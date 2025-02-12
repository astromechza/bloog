use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::prelude::*;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "statics/"]
pub struct Assets;

pub const ROUTE: &str = "/statics/{file}";
pub const FAVICON_ICO: &str = "/favicon.ico";
pub const FAVICON_SVG: &str = "/statics/favicon.svg";

pub async fn get_favicon_ico_handler() -> Response {
    let mut hm = HeaderMap::new();
    hm.insert("Location", HeaderValue::from_static(FAVICON_SVG));
    (StatusCode::TEMPORARY_REDIRECT, hm).into_response()
}

pub async fn get_static_handler(headers: HeaderMap, Path(file): Path<String>) -> Response {
    if let Some(content) = Assets::get(&file) {
        let encoded_hash = BASE64_STANDARD.encode(content.metadata.sha256_hash());
        let mut hm = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(content.metadata.mimetype()) {
            hm.insert("Content-Type", hv);
        }
        if let Ok(hv) = HeaderValue::from_str(encoded_hash.as_str()) {
            hm.insert("Etag", hv);
        }
        if let Some(hv) = headers.get("Etag") {
            if hv.as_bytes() == encoded_hash.as_bytes() {
                if let Ok(hv) = HeaderValue::from_str(content.data.len().to_string().as_str()) {
                    hm.insert("Content-Length", hv);
                }
                return (StatusCode::NOT_MODIFIED, hm).into_response();
            }
        }
        hm.insert(
            "Cache-Control",
            HeaderValue::from_static("public, max-age=86400, stale-while-revalidate=300"),
        );
        (StatusCode::OK, hm, content.data.clone()).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}
