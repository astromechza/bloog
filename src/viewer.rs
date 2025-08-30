mod views;

use crate::conversion::convert;
use crate::htmx::HtmxContext;
use crate::statics::{get_favicon_ico_handler, get_static_handler};
use crate::store::{Image, Store};
use crate::{conversion, customhttptrace, statics};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::Datelike;
use itertools::Itertools;
use log::info;
use maud::PreEscaped;
use object_store::path::PathPart;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::instrument;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct Config {
    pub port: u16,
}

impl Default for Config {
    fn default() -> Config {
        Config { port: 8080 }
    }
}

pub async fn run(cfg: Config, store: Store) -> Result<(), anyhow::Error> {
    validate(&store).await?;
    let app = Router::new()
        .route("/", get(index_handler))
        .route(statics::FAVICON_ICO, get(get_favicon_ico_handler))
        .route(statics::ROUTE, get(get_static_handler))
        .route("/posts/{slug}", get(get_post_handler))
        .route("/images/{slug}", get(get_image_handler))
        .route("/livez", get(livez_handler))
        .route("/readyz", get(readyz_handler))
        .fallback(not_found_handler)
        .with_state(Arc::new(store))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(customhttptrace::HttpTraceLayerHooks)
                .on_request(customhttptrace::HttpTraceLayerHooks)
                .on_response(customhttptrace::HttpTraceLayerHooks)
                .on_failure(customhttptrace::HttpTraceLayerHooks),
        );
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cfg.port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[instrument(skip_all, err)]
async fn validate(store: &Store) -> Result<(), anyhow::Error> {
    tracing::event!(tracing::Level::DEBUG, "starting post conversion validation");
    let images = store.list_images().await?;
    let posts = store.list_posts().await?;
    let valid_links = conversion::build_valid_links(&posts, &images);
    for (i, p) in posts.iter().enumerate() {
        info!("Validating  {}/{} ({})", i + 1, posts.len(), p.slug);
        if let Some((_, raw)) = store.get_post_raw(p.slug.as_ref()).await? {
            convert(raw.as_ref(), &valid_links)?;
        }
    }
    tracing::event!(tracing::Level::INFO, "post conversion validation complete");
    Ok(())
}

#[derive(Debug)]
struct ResponseError(anyhow::Error, Option<Box<HtmxContext>>);

impl IntoResponse for ResponseError {
    fn into_response(self) -> Response {
        views::internal_error_page(self.0, self.1).into_response()
    }
}

/// This trait helps to attach the [HtmxContext] to the [Result] and convert any old error into
/// a [ResponseError]. We implement this internal trait for any [Result] type.
trait CanMapToRespErr<T> {
    fn map_resp_err(self, htmx: &Option<Box<HtmxContext>>) -> Result<T, ResponseError>;
}

impl<T, E> CanMapToRespErr<T> for Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn map_resp_err(self, htmx: &Option<Box<HtmxContext>>) -> Result<T, ResponseError> {
        self.map_err(|e| ResponseError(e.into(), htmx.clone()))
    }
}

async fn not_found_handler(uri: Uri, headers: HeaderMap) -> Response {
    views::not_found_page(uri, HtmxContext::try_from(&headers).map(Box::new).ok()).into_response()
}

async fn get_image_handler(
    State(store): State<Arc<Store>>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ResponseError> {
    if HtmxContext::try_from(&headers).is_ok() {
        let mut hm = HeaderMap::new();
        if let Ok(hv) = HeaderValue::try_from(uri.path()) {
            hm.insert("HX-Redirect", hv);
        }
        return Ok((StatusCode::OK, hm).into_response());
    }
    let img = Image::try_from_path_part(PathPart::from(slug)).unwrap_or_default();
    if let Some(image) = store.get_image_raw(&img).await.map_resp_err(&None)? {
        let mut hm = HeaderMap::new();
        hm.insert("Content-Type", img.to_content_type());
        hm.insert(
            "Cache-Control",
            HeaderValue::from_static("public, max-age=86400, stale-while-revalidate=300"),
        );
        Ok((StatusCode::OK, hm, image).into_response())
    } else {
        Ok(StatusCode::NOT_FOUND.into_response())
    }
}

async fn index_handler(
    State(store): State<Arc<Store>>,
    query: Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).map(Box::new).ok();
    let label_filter = query.get("label");
    let mut posts = store.list_posts().await.map_resp_err(&htmx_context)?;
    posts.retain_mut(|p| p.published && label_filter.as_ref().is_none_or(|l| p.labels.contains(l)));
    posts.sort();
    posts.reverse();
    let group_map = posts.iter().into_group_map_by(|p| p.date.year());
    let year_groups = group_map.iter().sorted().rev().collect_vec();
    Ok(views::get_index_page(label_filter.map(|s| s.to_string()), year_groups, htmx_context).into_response())
}

async fn get_post_handler(
    State(store): State<Arc<Store>>,
    headers: HeaderMap,
    uri: Uri,
    Path(slug): Path<String>,
) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).map(Box::new).ok();
    if let Some((post, content)) = store.get_post_raw(&slug).await.map_resp_err(&htmx_context)? {
        let (content_html, toc) = convert(content.as_str(), &HashSet::default()).map_resp_err(&htmx_context)?;
        Ok(views::get_post_page(post, PreEscaped(content_html), PreEscaped(toc), htmx_context).into_response())
    } else {
        Ok(views::not_found_page(uri, htmx_context).into_response())
    }
}

async fn livez_handler() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn readyz_handler(State(store): State<Arc<Store>>) -> Result<Response, ResponseError> {
    store.readyz().await.map_resp_err(&None)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
