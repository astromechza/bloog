use std::fmt::Display;
use crate::path_utils::path_tail;
use anyhow::{anyhow, Error};
use base64::prelude::BASE64_STANDARD_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use chrono::{Local, NaiveDate};
use futures::future::ready;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use image::codecs::webp::WebPEncoder;
use image::ImageReader;
use itertools::Itertools;
use object_store::local::LocalFileSystem;
use object_store::path::{Path, PathPart};
use object_store::{ObjectStore, PutOptions, PutPayload};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::slice::Iter;
use url::Url;

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord,Default)]
pub enum PostContentType {
    #[default]
    Markdown,
    RestructuredText,
}

impl Display for PostContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord,Default)]
pub struct Post {
    pub date: NaiveDate,
    pub slug: String,
    pub title: String,
    pub content_type: PostContentType,
    pub labels: Vec<String>,
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord,Default)]
pub enum ImageVariant {
    #[default]
    Thumbnail,
    Medium,
    Original,
}

impl From<ImageVariant> for String {
    fn from(v: ImageVariant) -> String {
        match v {
            ImageVariant::Thumbnail => "thumbnail".to_string(),
            ImageVariant::Medium => "medium".to_string(),
            ImageVariant::Original => "original".to_string(),
        }
    }
}

impl TryFrom<&str> for ImageVariant {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value == "thumbnail" {
            Ok(ImageVariant::Thumbnail)
        } else if value == "medium" {
            Ok(ImageVariant::Medium)
        } else if value == "original" {
            Ok(ImageVariant::Original)
        } else {
            Err(anyhow!("Unknown image variant: {}", value))
        }
    }
}

/// The [Store] holds images and posts under a given sub path within a target object storage
/// provider. The schema looks like:
///
/// <pre>
/// (sub_path)/images/(slug)/(variant).webp
/// (sub_path)/posts/(slug)/props/(encoded props)
/// (sub_path)/posts/(slug)/content
/// (sub_path)/posts/(slug)/label/(key)
/// <pre>
///
/// Therefore, we use apis to list by delimiter and prefix where possible to reduce traversals.
#[derive(Debug)]
pub struct Store {
    os: Box<dyn ObjectStore>,
    sub_path: Path,
}

impl Store {
    const MEDIUM_VARIANT_WIDTH: u32 = 1000;
    const MEDIUM_VARIANT_HEIGHT: u32 = 1000;
    const THUMB_VARIANT_WIDTH: u32 = 200;
    const THUMB_VARIANT_HEIGHT: u32 = 200;

    pub fn new(os: Box<dyn ObjectStore>, sub_path: Path) -> Self {
        Self { os, sub_path }
    }

    pub fn from_url(url: &Url) -> Result<Self, Error> {
        if url.scheme() != "file" {
            let opts = url.query_pairs()
                .map(|i| (i.0.to_string(), i.1.to_string()))
                .collect_vec();
            let (inner, path) = object_store::parse_url_opts(url, opts)?;
            Ok(Store::new(Box::new(inner), path))
        } else {
            let mut los = LocalFileSystem::new();
            los = los.with_automatic_cleanup(true);
            Ok(Store::new(Box::new(los), Path::from_url_path(url.path())?))
        }
    }

    pub async fn upsert_post(&self, post: &Post, raw: &[u8]) -> Result<(), Error> {
        let post_path = self.sub_path.child("posts").child(post.slug.clone());

        let post_meta = PostMetadata::V1((post.date, post.title.clone(), post.content_type.clone()));
        let post_meta_bytes = postcard::to_allocvec(&post_meta)?;
        let post_meta_raw = BASE64_STANDARD_NO_PAD.encode(&post_meta_bytes);

        self.os.put_opts(&post_path.child("content"), PutPayload::from(raw.to_vec()), PutOptions::default()).await?;
        self.os.put_opts(&post_path.child("props").child(post_meta_raw.clone()), PutPayload::default(), PutOptions::default()).await?;
        // write the tags concurrently
        FuturesUnordered::from_iter(post.labels.iter().map(| lbl| {
                let label_path = post_path.child("labels").child(lbl.clone()).to_owned();
                async move {
                    self.os.put_opts(&label_path, PutPayload::default(), PutOptions::default()).await
                }
            }))
            .boxed()
            .try_collect::<Vec<_>>().await?;
        // and now clean up anything extra that we don't need
        let cleanup_paths = self.os.list(Some(&post_path))
            .map_ok(|m| m.location)
            .try_filter(|p| {
                let mut parts = p.parts();
                if let Some((sec, k)) = parts.next_tuple::<(PathPart, PathPart)>() {
                    return ready((sec.as_ref() == "props" && k.as_ref() != post_meta_raw) ||
                        (sec.as_ref() == "labels" && !post.labels.contains(&k.as_ref().to_string())))
                }
                ready(false)
            })
            .boxed();
        if self.os.delete_stream(cleanup_paths).try_collect::<Vec<Path>>().await?.is_empty() {
            Err(Error::msg("cleanup failed"))
        } else {
            Ok(())
        }
    }

    async fn delete_paths_by_prefix(&self, prefix: &Path) -> Result<(), Error> {
        let matched_paths = self.os.list(Some(&prefix))
            .map_ok(|m| m.location)
            .boxed();
        if self.os.delete_stream(matched_paths).try_collect::<Vec<Path>>().await?.is_empty() {
            Err(Error::msg("not found"))
        } else {
            Ok(())
        }
    }

    pub async fn delete_post(&self, slug: &str) -> Result<(), Error> {
        self.delete_paths_by_prefix(&self.sub_path.child("posts").child(slug)).await
    }

    pub async fn create_image(&self, pre_slug: &str, raw: &[u8]) -> Result<String, Error> {
        let slug = format!("{}-{}", Local::now().format("%Y%m%dT%H%M%S"), pre_slug);
        let image_path = self.sub_path.child("images").child(slug.clone());
        let image_reader = ImageReader::new(Cursor::new(raw))
            .with_guessed_format()
            .expect("reader should not fail on buffered data");
        let image = image_reader.decode()?;
        let medium = if image.width() > Self::MEDIUM_VARIANT_WIDTH || image.height() > Self::MEDIUM_VARIANT_HEIGHT {
            image.resize(Self::MEDIUM_VARIANT_WIDTH, Self::MEDIUM_VARIANT_HEIGHT, image::imageops::FilterType::Lanczos3)
        } else {
            image.clone()
        };
        let thumbnail = image.thumbnail(Self::THUMB_VARIANT_WIDTH, Self::THUMB_VARIANT_HEIGHT);

        let mut original_data = vec![];
        image.write_with_encoder(WebPEncoder::new_lossless(&mut original_data)).expect("image translation should never fail");
        self.os.put(&image_path.child("original.webp"), PutPayload::from(original_data)).await?;

        let mut medium_data = vec![];
        medium.write_with_encoder(WebPEncoder::new_lossless(&mut medium_data)).expect("image translation should never fail");
        self.os.put(&image_path.child("medium.webp"), PutPayload::from(medium_data)).await?;

        let mut thumbnail_data = vec![];
        thumbnail.write_with_encoder(WebPEncoder::new_lossless(&mut thumbnail_data)).expect("image translation should never fail");
        self.os.put(&image_path.child("thumbnail.webp"), PutPayload::from(thumbnail_data)).await?;

        Ok(slug)
    }

    pub async fn delete_image(&self, slug: &str) -> Result<(), Error> {
        self.delete_paths_by_prefix(&self.sub_path.child("images").child(slug)).await
    }

    fn labels_from_paths(i: Iter<&Path>, offset: usize) -> Vec<String> {
        i.into_iter().filter_map(|p| {
            let mut iter = p.parts();
            if iter.nth(offset + 2).map(|pp| pp.as_ref() == "labels").is_some() {
                iter.next().map(|pp| pp.as_ref().to_string())
            } else {
                None
            }
        }).sorted().collect()
    }

    fn props_part_from_paths(mut paths: Iter<&Path>, offset: usize) -> Option<PostMetadata> {
        paths.find(|path|  path.parts().nth(offset + 2).filter(|pp| pp.as_ref() == "props").is_some())
            .and_then(|path|  path.parts().nth(offset + 3))
            .and_then(|p| BASE64_STANDARD_NO_PAD.decode(p.as_ref().as_bytes()).ok())
            .and_then(|b| postcard::from_bytes(&b).ok())
    }

    pub async fn list_posts(&self) -> Result<Vec<Post>, Error> {
        let objects_paths: Vec<Path> = self.os
            .list(Some(&self.sub_path.child("posts")))
            .map_ok(|i| path_tail(&i.location, &self.sub_path))
            .boxed()
            .try_collect::<Vec<Path>>()
            .await?;

        // each path looks like images/... since we've removed the prefix path already
        Ok(objects_paths.iter()
            .into_group_map_by(|f| f.parts().nth(1)).iter()
            .filter(|(slug, _)| slug.is_some())
            .map(|(slug, paths)| {
                let slug = slug.as_ref().unwrap().as_ref().to_string();
                let labels = Self::labels_from_paths(paths.iter(), 0);
                match Self::props_part_from_paths(paths.iter(), 0) {
                    Some(PostMetadata::V1((date, title, content_type))) => Post {
                        date,
                        slug,
                        title,
                        content_type,
                        labels,
                    },
                    None => Post {
                        slug,
                        labels,
                        ..Post::default()
                    },
                }
            }).collect())
    }

    pub async fn get_post_raw(&self, slug: &str) -> Result<Option<(Post, String)>, Error> {
        let post_path = self.sub_path.child("posts").child(slug);
        let content_bytes = match self.os.get(&post_path.child("content")).await {
            Ok(gr) => gr.bytes().await?,
            Err(object_store::Error::NotFound{..}) => {
                return Ok(None);
            },
            Err(e) => return Err(e.into()),
        };
        let content = String::from_utf8_lossy(content_bytes.as_ref()).to_string();
        let post_paths: Vec<Path> = self.os
            .list(Some(&post_path))
            .map_ok(|i| path_tail(&i.location, &self.sub_path))
            .boxed()
            .try_collect::<Vec<Path>>()
            .await?;
        let post_paths_refs: Vec<&Path> = post_paths.iter().collect();
        let labels = Self::labels_from_paths(post_paths_refs.iter(), 0);
        let post = match Self::props_part_from_paths(post_paths_refs.iter(), 0) {
            Some(PostMetadata::V1((date, title, content_type))) => Post {
                date,
                slug: slug.to_string(),
                title,
                content_type,
                labels,
            },
            None => Post {
                slug: slug.to_string(),
                labels,
                ..Post::default()
            },
        };
        Ok(Some((post, content)))
    }

    pub async fn list_images(&self) -> Result<Vec<String>, Error> {
        Ok(self.os.list_with_delimiter(Some(&self.sub_path.child("images")))
            .await?
            .common_prefixes.iter()
            .map(|m| path_tail(&Path::from(m.filename().unwrap()), &self.sub_path).to_string())
            .collect_vec())
    }

    pub async fn get_image_raw(&self, slug: &str, variant: ImageVariant) -> Result<Option<Bytes>, Error> {
        let variant_slug = match variant {
            ImageVariant::Original => "original.webp",
            ImageVariant::Thumbnail => "thumbnail.webp",
            ImageVariant::Medium => "medium.webp"
        };
        match self.os.get(&self.sub_path.child("images").child(slug).child(variant_slug)).await {
            Ok(gr) => Ok(Some(gr.bytes().await?)),
            Err(object_store::Error::NotFound{..}) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new(Box::new(object_store::memory::InMemory::new()), Path::default())
    }
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord)]
enum PostMetadata {
    V1((NaiveDate, String, PostContentType)),
}

impl TryFrom<PathPart<'_>> for PostMetadata {
    type Error = Error;
    fn try_from(part: PathPart) -> Result<Self, Self::Error> {
        let props_bytes = BASE64_STANDARD_NO_PAD.decode(part.as_ref().as_bytes())?;
        let meta = postcard::from_bytes(&props_bytes)?;
        Ok(meta)
    }
}

impl From<PostMetadata> for PathPart<'_> {
    fn from(meta: PostMetadata) -> Self {
        let raw = postcard::to_allocvec(&meta).expect("Failed to serialize PostMetadata");
        PathPart::from(BASE64_STANDARD_NO_PAD.encode(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ColorType, DynamicImage};
    use std::ops::Deref;

    #[test]
    fn test_ser_der() {
        let p = PostMetadata::V1((NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), "fizz".to_string(), PostContentType::Markdown));
        let b = postcard::to_allocvec(&p).unwrap();
        assert_eq!(b.len(), 18);
        assert_eq!(b, vec![
            // enum 0
            0,
            // Note: this just falls back to RFC3339 string encoding (4 + 1 + 2 + 1 + 2 == 10) for the date. Postcard doesn't seem to
            // have a binary representation but I'm not sure I care.
            10, 50, 48, 50, 52, 45, 48, 49, 45, 48, 50,
            // String encoding.
            4, 102, 105, 122, 122,
            // enum 0
            0,
        ]);
        let p2 = postcard::from_bytes(b.as_slice()).unwrap();
        assert_eq!(p, p2);
    }

    #[tokio::test]
    async fn test_store_images_empty() {
        let store = Store::default();
        assert!(store.list_images().await.unwrap().is_empty());
        assert_eq!(store.get_image_raw("fizz", ImageVariant::Medium).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_store_posts_empty() {
        let store = Store::default();
        assert!(store.list_posts().await.unwrap().is_empty());
        assert_eq!(store.get_post_raw("fizz").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_store_images() {
        let store = Store {
            sub_path: Path::from("default"),
            ..Store::default()
        };

        let eg_image = DynamicImage::new(100, 100, ColorType::Rgb8);
        let mut eg_data: Vec<u8> = vec![];
        eg_image.write_with_encoder(JpegEncoder::new(&mut eg_data)).unwrap();

        let slug = store.create_image("test", eg_data.deref()).await.unwrap();
        assert_eq!(store.list_images().await.unwrap(), vec![slug.clone()]);
        assert_ne!(store.get_image_raw(&slug, ImageVariant::Thumbnail).await.unwrap(), None);
        assert_ne!(store.get_image_raw(&slug, ImageVariant::Medium).await.unwrap(), None);
        assert_ne!(store.get_image_raw(&slug, ImageVariant::Original).await.unwrap(), None);

        store.delete_image(&slug).await.unwrap();
        assert_eq!(store.list_images().await.unwrap(), Vec::<String>::new());
        assert_eq!(store.get_image_raw(&slug, ImageVariant::Thumbnail).await.unwrap(), None);
        assert_eq!(store.get_image_raw(&slug, ImageVariant::Medium).await.unwrap(), None);
        assert_eq!(store.get_image_raw(&slug, ImageVariant::Original).await.unwrap(), None);
    }

}