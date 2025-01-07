use std::fmt::Display;
use crate::path_utils::path_tail;
use anyhow::{anyhow, Context, Error};
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
use object_store::{ObjectMeta, ObjectStore, PutOptions, PutPayload};
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
    pub published: bool,
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

#[allow(dead_code)]
impl Store {
    const MEDIUM_VARIANT_WIDTH: u32 = 1000;
    const MEDIUM_VARIANT_HEIGHT: u32 = 1000;
    const THUMB_VARIANT_WIDTH: u32 = 200;
    const THUMB_VARIANT_HEIGHT: u32 = 200;

    pub fn new(os: Box<dyn ObjectStore>, sub_path: Path) -> Self {
        Self { os, sub_path }
    }

    #[allow(dead_code)]
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

    pub async fn upsert_post(&self, post: &Post, content: &str) -> Result<(), Error> {
        let post_path = self.sub_path.child("posts").child(post.slug.clone());

        let post_meta = PostMetadata::V1((post.date, post.title.clone(), post.content_type.clone(), post.published));
        let post_meta_bytes = postcard::to_allocvec(&post_meta)?;
        let post_meta_raw = BASE64_STANDARD_NO_PAD.encode(&post_meta_bytes);

        self.os.put_opts(&post_path.child("content"), PutPayload::from(content.to_string()), PutOptions::default()).await?;
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
            .boxed()
            .try_filter(|p| {
                let tail = path_tail(p, &post_path);
                if let Some((sec, k)) = tail.parts().next_tuple::<(PathPart, PathPart)>() {
                    return ready((sec.as_ref() == "props" && k.as_ref() != post_meta_raw.as_str()) ||
                        (sec.as_ref() == "labels" && !post.labels.contains(&k.as_ref().to_string())))
                }
                ready(false)
            })
            .boxed();
        self.os.delete_stream(cleanup_paths).try_collect::<Vec<Path>>().await?;
        Ok(())
    }

    async fn delete_paths_by_prefix(&self, prefix: &Path) -> Result<(), Error> {
        let matched_paths = self.os.list(Some(prefix))
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
            .with_guessed_format().context("failed to guess format")?;
        let image = image_reader.decode()?;
        let medium = if image.width() > Self::MEDIUM_VARIANT_WIDTH || image.height() > Self::MEDIUM_VARIANT_HEIGHT {
            image.resize(Self::MEDIUM_VARIANT_WIDTH, Self::MEDIUM_VARIANT_HEIGHT, image::imageops::FilterType::Lanczos3)
        } else {
            image.clone()
        };
        let thumbnail = image.thumbnail(Self::THUMB_VARIANT_WIDTH, Self::THUMB_VARIANT_HEIGHT);

        let mut original_data = vec![];
        image.write_with_encoder(WebPEncoder::new_lossless(&mut original_data))?;
        self.os.put(&image_path.child("original.webp"), PutPayload::from(original_data)).await?;

        let mut medium_data = vec![];
        medium.write_with_encoder(WebPEncoder::new_lossless(&mut medium_data))?;
        self.os.put(&image_path.child("medium.webp"), PutPayload::from(medium_data)).await?;

        let mut thumbnail_data = vec![];
        thumbnail.write_with_encoder(WebPEncoder::new_lossless(&mut thumbnail_data))?;
        self.os.put(&image_path.child("thumbnail.webp"), PutPayload::from(thumbnail_data)).await?;

        Ok(slug)
    }

    pub async fn delete_image(&self, slug: &str) -> Result<(), Error> {
        self.delete_paths_by_prefix(&self.sub_path.child("images").child(slug)).await
    }

    fn labels_from_paths(i: Iter<&Path>, offset: usize) -> Vec<String> {
        i.into_iter().filter_map(|p| {
            let mut iter = p.parts();
            if iter.nth(offset + 2).filter(|pp| pp.as_ref() == "labels").is_some() {
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
            .into_group_map_by(|f| f.parts().nth(1))
            .iter()
            .flat_map(|(slug, paths)| slug.as_ref().map(|p| (p.as_ref(), paths))  )
            .map(|(slug, paths)| {
                let slug = slug.to_string();
                let labels = Self::labels_from_paths(paths.iter(), 0);
                match Self::props_part_from_paths(paths.iter(), 0) {
                    Some(PostMetadata::V1((date, title, content_type, published))) => Post {
                        date,
                        slug,
                        title,
                        content_type,
                        published,
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
            Some(PostMetadata::V1((date, title, content_type, published))) => Post {
                date,
                slug: slug.to_string(),
                title,
                content_type,
                published,
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
            .flat_map(|m| m.filename().map(Path::from))
            .map(|m| path_tail(&m, &self.sub_path).to_string())
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

    pub async fn list_object_meta(&self) -> Result<Vec<ObjectMeta>, Error> {
        Ok(self.os.list(None)
            .boxed()
            .try_collect::<Vec<ObjectMeta>>().await?)
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new(Box::new(object_store::memory::InMemory::new()), Path::default())
    }
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord)]
enum PostMetadata {
    V1((NaiveDate, String, PostContentType, bool)),
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
        if let Ok(raw) = postcard::to_allocvec(&meta) {
            PathPart::from(BASE64_STANDARD_NO_PAD.encode(&raw))
        } else {
            PathPart::default()            
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ColorType, DynamicImage};
    use std::ops::Deref;

    #[test]
    fn test_ser_der() -> Result<(), Error> {
        let p = PostMetadata::V1((NaiveDate::from_ymd_opt(2024, 1, 2).unwrap_or_default(), "fizz".to_string(), PostContentType::Markdown, false));
        let b = postcard::to_allocvec(&p)?;
        assert_eq!(b.len(), 19);
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
            // boolean 0
            0,
        ]);
        let p2 = postcard::from_bytes(b.as_slice())?;
        assert_eq!(p, p2);
        Ok(())
    }

    #[tokio::test]
    async fn test_store_images_empty() -> Result<(), Error> {
        let store = Store::default();
        assert!(store.list_images().await?.is_empty());
        assert_eq!(store.get_image_raw("fizz", ImageVariant::Medium).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_store_posts_empty() -> Result<(), Error> {
        let store = Store::default();
        assert!(store.list_posts().await?.is_empty());
        assert_eq!(store.get_post_raw("fizz").await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_store_images() -> Result<(), Error> {
        let store = Store {
            sub_path: Path::from("default"),
            ..Store::default()
        };

        let eg_image = DynamicImage::new(100, 100, ColorType::Rgb8);
        let mut eg_data: Vec<u8> = vec![];
        eg_image.write_with_encoder(JpegEncoder::new(&mut eg_data))?;

        let slug = store.create_image("test", eg_data.deref()).await?;
        assert_eq!(store.list_images().await?, vec![slug.clone()]);
        assert_ne!(store.get_image_raw(&slug, ImageVariant::Thumbnail).await?, None);
        assert_ne!(store.get_image_raw(&slug, ImageVariant::Medium).await?, None);
        assert_ne!(store.get_image_raw(&slug, ImageVariant::Original).await?, None);

        store.delete_image(&slug).await?;
        assert_eq!(store.list_images().await?, Vec::<String>::new());
        assert_eq!(store.get_image_raw(&slug, ImageVariant::Thumbnail).await?, None);
        assert_eq!(store.get_image_raw(&slug, ImageVariant::Medium).await?, None);
        assert_eq!(store.get_image_raw(&slug, ImageVariant::Original).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_store_posts() -> Result<(), Error> {
        let store = Store {
            sub_path: Path::from("default"),
            ..Store::default()
        };
        store.upsert_post(&Post{
            date: NaiveDate::from_ymd_opt(2020, 1, 1).ok_or(anyhow!("invalid date"))?,
            slug: "my-first-post".to_string(),
            title: "My first post".to_string(),
            content_type: PostContentType::Markdown,
            published: true,
            labels: vec!["blue".to_string(), "green".to_string()],
        }, "my-content").await?;

        println!("{:#?}", store.list_object_meta().await?);

        let (post, content) = store.get_post_raw("my-first-post").await?.unwrap_or_default();
        assert_eq!(post.date, NaiveDate::from_ymd_opt(2020, 1, 1).ok_or(anyhow!("invalid date"))?);
        assert_eq!(post.slug, "my-first-post");
        assert_eq!(post.title, "My first post");
        assert_eq!(post.content_type, PostContentType::Markdown);
        assert!(post.published);
        assert_eq!(post.labels, vec!["blue".to_string(), "green".to_string()]);
        assert_eq!(content, "my-content".to_string());
        assert_eq!(store.list_object_meta().await?.len(), 4);
        store.upsert_post(&Post{
            date: NaiveDate::from_ymd_opt(2020, 1, 2).ok_or(anyhow!("invalid date"))?,
            slug: "my-first-post".to_string(),
            title: "My updated first post".to_string(),
            content_type: PostContentType::Markdown,
            published: false,
            labels: vec!["red".to_string(), "green".to_string()],
        }, "my-updated-content").await?;

        let (post, content) = store.get_post_raw("my-first-post").await?.unwrap_or_default();
        assert_eq!(post.date, NaiveDate::from_ymd_opt(2020, 1, 2).ok_or(anyhow!("invalid date"))?);
        assert_eq!(post.slug, "my-first-post");
        assert_eq!(post.title, "My updated first post");
        assert_eq!(post.content_type, PostContentType::Markdown);
        assert!(!post.published);
        assert_eq!(post.labels, vec!["green".to_string(), "red".to_string()]);
        assert_eq!(content, "my-updated-content".to_string());
        assert_eq!(store.list_object_meta().await?.len(), 4);

        Ok(())
    }
}