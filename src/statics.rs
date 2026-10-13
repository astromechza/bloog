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
        let etag = format!("\"{}\"", BASE64_STANDARD.encode(content.metadata.sha256_hash()));
        let mut hm = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(etag.as_str()) {
            hm.insert("ETag", hv);
        }
        hm.insert(
            "Cache-Control",
            HeaderValue::from_static("public, max-age=86400, stale-while-revalidate=300"),
        );
        if if_none_match(&headers, &etag) {
            return (StatusCode::NOT_MODIFIED, hm).into_response();
        }
        if let Ok(hv) = HeaderValue::from_str(content.metadata.mimetype()) {
            hm.insert("Content-Type", hv);
        }
        (StatusCode::OK, hm, content.data.clone()).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// Returns true when the request's If-None-Match header lists the given entity tag (or `*`), using the weak
/// comparison required for GET requests.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all("If-None-Match")
        .iter()
        .filter_map(|hv| hv.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim())
        .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(if_none_match: Option<&str>) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(v) = if_none_match {
            headers.insert("If-None-Match", HeaderValue::from_str(v).unwrap_or(HeaderValue::from_static("")));
        }
        get_static_handler(headers, Path("link.svg".to_string())).await
    }

    #[tokio::test]
    async fn test_static_etag() {
        let first = get(None).await;
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(etag.starts_with('"') && etag.ends_with('"'));

        assert_eq!(get(Some(&etag)).await.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(get(Some(&format!("\"other\", W/{etag}"))).await.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(get(Some("\"other\"")).await.status(), StatusCode::OK);
    }
}
