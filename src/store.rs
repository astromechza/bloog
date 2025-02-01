use crate::conversion;
use crate::path_utils::path_tail;
use anyhow::{anyhow, Context, Error};
use axum::http::HeaderValue;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use chrono::NaiveDate;
use futures::future::ready;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryStreamExt};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ImageReader};
use itertools::Itertools;
use object_store::local::LocalFileSystem;
use object_store::path::{Path, PathPart};
use object_store::{ObjectMeta, ObjectStore, PutOptions, PutPayload};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Cursor;
use std::slice::Iter;
use std::str::from_utf8;
use std::sync::Arc;
use url::Url;
use xmlparser::Token;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Post {
    pub date: NaiveDate,
    pub slug: String,
    pub title: String,
    pub published: bool,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Image {
    Svg { slug: Arc<str> },
    Webp { slug: Arc<str> },
    JpgMedium { slug: Arc<str> },
    JpgThumbnail { slug: Arc<str> },
}

impl AsRef<Image> for Image {
    fn as_ref(&self) -> &Image {
        self
    }
}

impl Image {
    pub fn to_original(&self) -> Image {
        match self {
            Image::Svg { slug } => Image::Svg { slug: slug.clone() },
            Image::Webp { slug } => Image::Webp { slug: slug.clone() },
            Image::JpgMedium { slug } => Image::Webp { slug: slug.clone() },
            Image::JpgThumbnail { slug } => Image::Webp { slug: slug.clone() },
        }
    }

    pub fn to_medium(&self) -> Image {
        match self {
            Image::Svg { slug } => Image::Svg { slug: slug.clone() },
            Image::Webp { slug } => Image::JpgMedium { slug: slug.clone() },
            Image::JpgMedium { slug } => Image::JpgMedium { slug: slug.clone() },
            Image::JpgThumbnail { slug } => Image::JpgMedium { slug: slug.clone() },
        }
    }

    pub fn to_thumbnail(&self) -> Image {
        match self {
            Image::Svg { slug } => Image::Svg { slug: slug.clone() },
            Image::Webp { slug } => Image::JpgThumbnail { slug: slug.clone() },
            Image::JpgMedium { slug } => Image::JpgThumbnail { slug: slug.clone() },
            Image::JpgThumbnail { slug } => Image::JpgThumbnail { slug: slug.clone() },
        }
    }

    pub fn to_content_type(&self) -> HeaderValue {
        match self {
            Image::Svg { .. } => HeaderValue::from_static("image/svg+xml"),
            Image::Webp { .. } => HeaderValue::from_static("image/webp"),
            Image::JpgMedium { .. } => HeaderValue::from_static("image/jpg"),
            Image::JpgThumbnail { .. } => HeaderValue::from_static("image/jpg"),
        }
    }

    pub fn to_path_part(&self) -> PathPart {
        match self {
            Image::Svg { slug } => PathPart::from(format!("{}.svg", slug)),
            Image::Webp { slug } => PathPart::from(format!("{}.webp", slug)),
            Image::JpgMedium { slug } => PathPart::from(format!("{}.medium.jpg", slug)),
            Image::JpgThumbnail { slug } => PathPart::from(format!("{}.thumb.jpg", slug)),
        }
    }

    pub fn try_from_path_part(p: PathPart) -> Result<Self, Error> {
        let mut parts = p.as_ref().split('.').rev();
        match parts.next() {
            Some("svg") => Ok(Image::Svg {
                slug: Arc::from(parts.rev().join(".")),
            }),
            Some("webp") => Ok(Image::Webp {
                slug: Arc::from(parts.rev().join(".")),
            }),
            Some("jpg") => {
                let variant = parts.next();
                let rem = parts.rev().join(".");
                match variant {
                    Some("medium") => Ok(Image::JpgMedium {
                        slug: Arc::from(rem),
                    }),
                    Some("thumb") => Ok(Image::JpgThumbnail {
                        slug: Arc::from(rem),
                    }),
                    _ => Err(anyhow!("invalid image variant")),
                }
            }
            _ => Err(anyhow!("invalid image variant")),
        }
    }

    pub fn resolve_full_path(&self, parent: &Path) -> Path {
        let original = self.to_original();
        parent.child("images").child(original.to_path_part()).child(self.to_path_part())
    }
}

impl Default for Image {
    fn default() -> Self {
        Image::Webp {
            slug: Arc::from(""),
        }
    }
}

/// The [Store] holds images and posts under a given sub path within a target object storage
/// provider. The schema looks like:
///
/// <pre>
/// (sub_path)/images/(slug).(svg|webp)/(slug).(svg|webp)
/// (sub_path)/images/(slug).(svg|webp)/(slug).(variant).(jpg)
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
    const MEDIUM_VARIANT_WIDTH: u32 = 800;
    const MEDIUM_VARIANT_HEIGHT: u32 = 550;
    const THUMB_VARIANT_WIDTH: u32 = 200;
    const THUMB_VARIANT_HEIGHT: u32 = 200;

    pub fn new(os: Box<dyn ObjectStore>, sub_path: Path) -> Self {
        Self { os, sub_path }
    }

    pub fn from_url(url: &Url) -> Result<Self, Error> {
        if url.scheme() != "file" {
            let opts = url
                .query_pairs()
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

    pub async fn convert_html_with_validation(&self, content: &str) -> Result<String, Error> {
        let valid_paths = self
            .list_images()
            .await?
            .iter()
            .flat_map(|i| {
                vec![
                    format!("/images/{}", i.to_original().to_path_part().as_ref()),
                    format!("/images/{}", i.to_medium().to_path_part().as_ref()),
                ]
                .into_iter()
            })
            .chain(
                self.list_posts()
                    .await?
                    .iter()
                    .map(|p| format!("/posts/{}", p.slug)),
            )
            .collect::<HashSet<String>>();
        conversion::convert(content, valid_paths)
    }

    pub async fn upsert_post(&self, post: &Post, content: &str) -> Result<String, Error> {
        let re = Regex::new(r"^[A-Za-z\-_0-9]{3,100}$")?;
        if !re.is_match(post.slug.as_str()) {
            return Err(Error::msg("invalid post slug"));
        }

        let html_content = self.convert_html_with_validation(content).await?;

        let post_path = self.sub_path.child("posts").child(post.slug.clone());
        let post_meta =
            PostMetadata::V1((post.date, post.title.clone(), IsPublished(post.published)));
        let post_meta_bytes = postcard::to_allocvec(&post_meta)?;
        let post_meta_raw = BASE64_STANDARD_NO_PAD.encode(&post_meta_bytes);

        self.os
            .put_opts(
                &post_path.child("content"),
                PutPayload::from(content.to_string()),
                PutOptions::default(),
            )
            .await?;
        self.os
            .put_opts(
                &post_path.child("props").child(post_meta_raw.clone()),
                PutPayload::default(),
                PutOptions::default(),
            )
            .await?;
        // write the tags concurrently
        FuturesUnordered::from_iter(post.labels.iter().map(|lbl| {
            let label_path = post_path.child("labels").child(lbl.clone()).to_owned();
            async move {
                self.os
                    .put_opts(&label_path, PutPayload::default(), PutOptions::default())
                    .await
            }
        }))
        .boxed()
        .try_collect::<Vec<_>>()
        .await?;
        // and now clean up anything extra that we don't need
        let cleanup_paths = self
            .os
            .list(Some(&post_path))
            .map_ok(|m| m.location)
            .boxed()
            .try_filter(|p| {
                let tail = path_tail(p, &post_path);
                if let Some((sec, k)) = tail.parts().next_tuple::<(PathPart, PathPart)>() {
                    return ready(
                        (sec.as_ref() == "props" && k.as_ref() != post_meta_raw.as_str())
                            || (sec.as_ref() == "labels"
                                && !post.labels.contains(&k.as_ref().to_string())),
                    );
                }
                ready(false)
            })
            .boxed();
        self.os
            .delete_stream(cleanup_paths)
            .try_collect::<Vec<Path>>()
            .await?;
        Ok(html_content)
    }

    async fn delete_paths_by_prefix(&self, prefix: &Path) -> Result<(), Error> {
        let matched_paths = self.os.list(Some(prefix)).map_ok(|m| m.location).boxed();
        if self
            .os
            .delete_stream(matched_paths)
            .try_collect::<Vec<Path>>()
            .await?
            .is_empty()
        {
            Err(Error::msg("not found"))
        } else {
            Ok(())
        }
    }

    pub async fn delete_post(&self, slug: &str) -> Result<(), Error> {
        self.delete_paths_by_prefix(&self.sub_path.child("posts").child(slug))
            .await
    }

    async fn create_webp_image(&self, slug: &str, image: DynamicImage) -> Result<Image, Error> {
        let original_image = Image::Webp {
            slug: Arc::from(slug),
        };
        if self.check_image_exists(&original_image).await? {
            return Err(Error::msg("image slug already exists"));
        }
        let medium = if image.width() > Self::MEDIUM_VARIANT_WIDTH
            || image.height() > Self::MEDIUM_VARIANT_HEIGHT
        {
            image
                .resize(
                    Self::MEDIUM_VARIANT_WIDTH,
                    Self::MEDIUM_VARIANT_HEIGHT,
                    image::imageops::FilterType::Triangle,
                )
                .into_rgb8()
        } else {
            image.clone().into_rgb8()
        };
        let thumbnail = image
            .thumbnail(Self::THUMB_VARIANT_WIDTH, Self::THUMB_VARIANT_HEIGHT)
            .into_rgb8();

        let mut original_data = vec![];
        image.write_with_encoder(WebPEncoder::new_lossless(&mut original_data))?;
        self.os
            .put(
                &original_image.resolve_full_path(&self.sub_path),
                PutPayload::from(original_data),
            )
            .await?;
        let mut medium_data = vec![];

        medium.write_with_encoder(JpegEncoder::new_with_quality(&mut medium_data, 90))?;
        self.os
            .put(
                &original_image.to_medium().resolve_full_path(&self.sub_path),
                PutPayload::from(medium_data),
            )
            .await?;

        let mut thumbnail_data = vec![];
        thumbnail.write_with_encoder(JpegEncoder::new_with_quality(&mut thumbnail_data, 85))?;
        self.os
            .put(
                &original_image
                    .to_thumbnail()
                    .resolve_full_path(&self.sub_path),
                PutPayload::from(thumbnail_data),
            )
            .await?;

        Ok(original_image)
    }

    async fn create_svg_image(&self, slug: &str, raw: &[u8]) -> Result<Image, Error> {
        let original_image = Image::Svg {
            slug: Arc::from(slug),
        };
        if self.check_image_exists(&original_image).await? {
            return Err(Error::msg("image slug already exists"));
        }
        let raw_str = from_utf8(raw)?;
        let first_element = xmlparser::Tokenizer::from(raw_str).find(|t| match t {
            Ok(Token::ElementStart { .. }) => true,
            Ok(_) => false,
            Err(_) => true,
        });
        match first_element {
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(anyhow!(e)).context("failed to read svg"),
            None => return Err(Error::msg("empty svg content")),
        }
        self.os
            .put(
                &original_image.resolve_full_path(&self.sub_path),
                PutPayload::from(raw.to_vec()),
            )
            .await?;
        Ok(original_image)
    }

    pub async fn create_image(&self, slug: &str, raw: &[u8]) -> Result<Image, Error> {
        let re = Regex::new(r"^[A-Za-z\-_0-9]{3,60}$")?;
        if !re.is_match(slug) {
            return Err(Error::msg("invalid image slug"));
        }

        match ImageReader::new(Cursor::new(raw))
            .with_guessed_format()?
            .decode()
        {
            Ok(dimg) => self
                .create_webp_image(slug, dimg)
                .await
                .context("failed to create webp image"),
            Err(_) => self
                .create_svg_image(slug, raw)
                .await
                .context("failed to create SVG"),
        }
    }

    pub async fn delete_image(&self, img: impl AsRef<Image>) -> Result<(), Error> {
        let candidate_paths = vec![
            Ok(img
                .as_ref()
                .to_thumbnail()
                .resolve_full_path(&self.sub_path)),
            Ok(img.as_ref().to_medium().resolve_full_path(&self.sub_path)),
            Ok(img.as_ref().to_original().resolve_full_path(&self.sub_path)),
        ];
        if self
            .os
            .delete_stream(futures::stream::iter(candidate_paths).boxed())
            .filter(|r| ready(!matches!(r, Err(object_store::Error::NotFound { .. }))))
            .try_collect::<Vec<Path>>()
            .await?
            .is_empty()
        {
            Err(Error::msg("not found"))
        } else {
            Ok(())
        }
    }

    fn labels_from_paths(i: Iter<&Path>, offset: usize) -> Vec<String> {
        i.into_iter()
            .filter_map(|p| {
                let mut iter = p.parts();
                if iter
                    .nth(offset + 2)
                    .filter(|pp| pp.as_ref() == "labels")
                    .is_some()
                {
                    iter.next().map(|pp| pp.as_ref().to_string())
                } else {
                    None
                }
            })
            .sorted()
            .collect()
    }

    fn props_part_from_paths(mut paths: Iter<&Path>, offset: usize) -> Option<PostMetadata> {
        paths
            .find(|path| {
                path.parts()
                    .nth(offset + 2)
                    .filter(|pp| pp.as_ref() == "props")
                    .is_some()
            })
            .and_then(|path| path.parts().nth(offset + 3))
            .and_then(|p| BASE64_STANDARD_NO_PAD.decode(p.as_ref().as_bytes()).ok())
            .and_then(|b| postcard::from_bytes(&b).ok())
    }

    pub async fn list_posts(&self) -> Result<Vec<Post>, Error> {
        let objects_paths: Vec<Path> = self
            .os
            .list(Some(&self.sub_path.child("posts")))
            .map_ok(|i| path_tail(&i.location, &self.sub_path))
            .boxed()
            .try_collect::<Vec<Path>>()
            .await?;

        // each path looks like images/... since we've removed the prefix path already
        Ok(objects_paths
            .iter()
            .into_group_map_by(|f| f.parts().nth(1))
            .iter()
            .flat_map(|(slug, paths)| slug.as_ref().map(|p| (p.as_ref(), paths)))
            .map(|(slug, paths)| {
                let slug = slug.to_string();
                let labels = Self::labels_from_paths(paths.iter(), 0);
                match Self::props_part_from_paths(paths.iter(), 0) {
                    Some(PostMetadata::V1((date, title, published))) => Post {
                        date,
                        slug,
                        title,
                        published: published.into(),
                        labels,
                    },
                    None => Post {
                        slug,
                        labels,
                        ..Post::default()
                    },
                }
            })
            .collect())
    }

    pub async fn get_post_raw(&self, slug: &str) -> Result<Option<(Post, String)>, Error> {
        let post_path = self.sub_path.child("posts").child(slug);
        let content_bytes = match self.os.get(&post_path.child("content")).await {
            Ok(gr) => gr.bytes().await?,
            Err(object_store::Error::NotFound { .. }) => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let content = String::from_utf8_lossy(content_bytes.as_ref()).to_string();
        let post_paths: Vec<Path> = self
            .os
            .list(Some(&post_path))
            .map_ok(|i| path_tail(&i.location, &self.sub_path))
            .boxed()
            .try_collect::<Vec<Path>>()
            .await?;
        let post_paths_refs: Vec<&Path> = post_paths.iter().collect();
        let labels = Self::labels_from_paths(post_paths_refs.iter(), 0);
        let post = match Self::props_part_from_paths(post_paths_refs.iter(), 0) {
            Some(PostMetadata::V1((date, title, published))) => Post {
                date,
                slug: slug.to_string(),
                title,
                published: published.into(),
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

    pub async fn list_images(&self) -> Result<Vec<Image>, Error> {
        Ok(self
            .os
            .list_with_delimiter(Some(&self.sub_path.child("images")))
            .await?
            .common_prefixes
            .iter()
            .map(|om| Image::try_from_path_part(om.parts().last().unwrap_or_default()))
            .filter_map(Result::ok)
            .collect_vec())
    }

    pub async fn check_image_exists(&self, img: impl AsRef<Image>) -> Result<bool, Error> {
        let p = &self.sub_path;
        match self.os.head(&img.as_ref().resolve_full_path(p)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn get_image_raw(&self, img: impl AsRef<Image>) -> Result<Option<Bytes>, Error> {
        let p = &self.sub_path;
        match self.os.get(&img.as_ref().resolve_full_path(p)).await {
            Ok(gr) => Ok(Some(gr.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn list_object_meta(&self) -> Result<Vec<ObjectMeta>, Error> {
        Ok(self
            .os
            .list(None)
            .boxed()
            .try_collect::<Vec<ObjectMeta>>()
            .await?)
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new(
            Box::new(object_store::memory::InMemory::new()),
            Path::default(),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IsPublished(bool);

impl From<IsPublished> for bool {
    fn from(is_published: IsPublished) -> Self {
        is_published.0
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PostMetadata {
    V1((NaiveDate, String, IsPublished)),
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
        let p = PostMetadata::V1((
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap_or_default(),
            "fizz".to_string(),
            IsPublished(false),
        ));
        let b = postcard::to_allocvec(&p)?;
        assert_eq!(b.len(), 18);
        assert_eq!(
            b,
            vec![
                // enum 0
                0,
                // Note: this just falls back to RFC3339 string encoding (4 + 1 + 2 + 1 + 2 == 10) for the date. Postcard doesn't seem to
                // have a binary representation but I'm not sure I care.
                10, 50, 48, 50, 52, 45, 48, 49, 45, 48, 50, // String encoding.
                4, 102, 105, 122, 122, // boolean 0
                0,
            ]
        );
        let p2 = postcard::from_bytes(b.as_slice())?;
        assert_eq!(p, p2);
        Ok(())
    }

    #[tokio::test]
    async fn test_store_images_empty() -> Result<(), Error> {
        let store = Store::default();
        assert!(store.list_images().await?.is_empty());
        assert_eq!(
            store
                .get_image_raw(Image::Webp {
                    slug: Arc::from("fizz")
                })
                .await?,
            None
        );
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

        let img = store.create_image("test", eg_data.deref()).await?;
        assert_eq!(store.list_images().await?, vec![img.clone()]);
        assert_ne!(store.get_image_raw(img.to_thumbnail()).await?, None);
        assert_ne!(store.get_image_raw(img.to_medium()).await?, None);
        assert_ne!(store.get_image_raw(img.to_original()).await?, None);

        store.delete_image(&img).await?;
        assert_eq!(store.list_images().await?, Vec::<Image>::new());
        assert_eq!(store.get_image_raw(img.to_thumbnail()).await?, None);
        assert_eq!(store.get_image_raw(img.to_medium()).await?, None);
        assert_eq!(store.get_image_raw(img.to_original()).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_store_posts() -> Result<(), Error> {
        let store = Store {
            sub_path: Path::from("default"),
            ..Store::default()
        };
        store
            .upsert_post(
                &Post {
                    date: NaiveDate::from_ymd_opt(2020, 1, 1).ok_or(anyhow!("invalid date"))?,
                    slug: "my-first-post".to_string(),
                    title: "My first post".to_string(),
                    published: true,
                    labels: vec!["blue".to_string(), "green".to_string()],
                },
                "my-content",
            )
            .await?;

        println!("{:#?}", store.list_object_meta().await?);

        let (post, content) = store
            .get_post_raw("my-first-post")
            .await?
            .unwrap_or_default();
        assert_eq!(
            post.date,
            NaiveDate::from_ymd_opt(2020, 1, 1).ok_or(anyhow!("invalid date"))?
        );
        assert_eq!(post.slug, "my-first-post");
        assert_eq!(post.title, "My first post");
        assert!(post.published);
        assert_eq!(post.labels, vec!["blue".to_string(), "green".to_string()]);
        assert_eq!(content, "my-content".to_string());
        assert_eq!(store.list_object_meta().await?.len(), 4);
        store
            .upsert_post(
                &Post {
                    date: NaiveDate::from_ymd_opt(2020, 1, 2).ok_or(anyhow!("invalid date"))?,
                    slug: "my-first-post".to_string(),
                    title: "My updated first post".to_string(),
                    published: false,
                    labels: vec!["red".to_string(), "green".to_string()],
                },
                "my-updated-content",
            )
            .await?;

        let (post, content) = store
            .get_post_raw("my-first-post")
            .await?
            .unwrap_or_default();
        assert_eq!(
            post.date,
            NaiveDate::from_ymd_opt(2020, 1, 2).ok_or(anyhow!("invalid date"))?
        );
        assert_eq!(post.slug, "my-first-post");
        assert_eq!(post.title, "My updated first post");
        assert!(!post.published);
        assert_eq!(post.labels, vec!["green".to_string(), "red".to_string()]);
        assert_eq!(content, "my-updated-content".to_string());
        assert_eq!(store.list_object_meta().await?.len(), 4);

        Ok(())
    }
}
