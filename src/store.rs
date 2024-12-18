use crate::path_utils::path_tail;
use anyhow::Error;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use base64::Engine;
use bytes::Bytes;
use chrono::{Local, NaiveDate};
use futures::{StreamExt, TryStreamExt};
use image::codecs::webp::WebPEncoder;
use image::ImageReader;
use itertools::Itertools;
use object_store::local::LocalFileSystem;
use object_store::path::{Path, PathPart};
use object_store::{ObjectStore, PutPayload};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use url::Url;

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord,Default)]
pub enum PostContentType {
    #[default]
    Markdown,
    RestructuredText,
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord,Default)]
pub struct Post {
    date: NaiveDate,
    slug: String,
    title: String,
    content_type: PostContentType,
    labels: Vec<String>,
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord,Default)]
pub enum ImageVariant {
    #[default]
    Thumbnail,
    Medium,
    Original,
}

trait ReadOnlyStore {
    async fn list_posts(&self) -> Result<Vec<Post>, Error>;
    async fn get_raw_post(&self, slug: &str) -> Result<Option<(Post, String)>, Error>;
    async fn list_images(&self) -> Result<Vec<String>, Error>;

    async fn get_image_raw(&self, slug: &str, variant: ImageVariant) -> Result<Option<Bytes>, Error>;
}

trait WritableStore {

    const MEDIUM_VARIANT_WIDTH: u32 = 1000;
    const MEDIUM_VARIANT_HEIGHT: u32 = 1000;
    const THUMB_VARIANT_WIDTH: u32 = 200;
    const THUMB_VARIANT_HEIGHT: u32 = 200;

    async fn create_post(&mut self, post: &Post, raw: &[u8]) -> Result<(), Error>;
    async fn delete_post(&mut self, slug: &str) -> Result<(), Error>;
    async fn create_image(&mut self, pre_slug: &str, raw: &[u8]) -> Result<String, Error>;
    async fn delete_image(&mut self, slug: &str) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct Store {
    os: Box<dyn ObjectStore>,
    sub_path: Path,
}

impl Store {
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

}

impl Default for Store {
    fn default() -> Self {
        Self::new(Box::new(object_store::memory::InMemory::new()), Path::from("default"))
    }
}

impl WritableStore for Store {
    async fn create_post(&mut self, post: &Post, raw: &[u8]) -> Result<(), Error> {
        todo!()
    }

    async fn delete_post(&mut self, slug: &str) -> Result<(), Error> {
        todo!()
    }

    async fn create_image(&mut self, pre_slug: &str, raw: &[u8]) -> Result<String, Error> {
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

    async fn delete_image(&mut self, slug: &str) -> Result<(), Error> {
        let image_path = self.sub_path.child("images").child(slug);
        let variant_paths = self.os.list(Some(&image_path))
            .map_ok(|m| m.location)
            .boxed();
        if self.os.delete_stream(variant_paths)
            .try_collect::<Vec<Path>>()
            .await?.is_empty() {
            Err(Error::msg("not found"))
        } else {
            Ok(())
        }
    }
}

impl ReadOnlyStore for Store {
    async fn list_posts(&self) -> Result<Vec<Post>, Error> {
        todo!()
    }

    async fn get_raw_post(&self, slug: &str) -> Result<Option<(Post, String)>, Error> {
        todo!()
    }

    async fn list_images(&self) -> Result<Vec<String>, Error> {
        Ok(self.os.list_with_delimiter(Some(&self.sub_path.child("images")))
            .await?
            .common_prefixes.iter()
            .map(|m| path_tail(&Path::from(m.filename().unwrap()), &self.sub_path).to_string())
            .collect_vec())
    }

    async fn get_image_raw(&self, slug: &str, variant: ImageVariant) -> Result<Option<Bytes>, Error> {
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

    #[tokio::test]
    async fn test_store_images_empty() {
        let store = Store::default();
        assert!(store.list_images().await.unwrap().is_empty());
        assert_eq!(store.get_image_raw("fizz", ImageVariant::Medium).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_store_images() {
        let mut store = Store::default();

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