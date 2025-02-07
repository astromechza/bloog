mod views;

use crate::conversion::convert;
use crate::htmx::HtmxContext;
use crate::store::{Image, Store};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::Datelike;
use itertools::Itertools;
use maud::PreEscaped;
use object_store::path::PathPart;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/favicon.ico", get(get_favicon_ico_handler))
        .route("/posts/{slug}", get(get_post_handler))
        .route("/images/{slug}", get(get_image_handler))
        .route("/livez", get(livez_handler))
        .route("/readyz", get(readyz_handler))
        .fallback(not_found_handler)
        .with_state(Arc::new(store));
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cfg.port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug)]
struct ResponseError(anyhow::Error, Option<HtmxContext>);

impl IntoResponse for ResponseError {
    fn into_response(self) -> Response {
        views::internal_error_page(self.0, self.1).into_response()
    }
}

/// This trait helps to attach the [HtmxContext] to the [Result] and convert any old error into
/// a [ResponseError]. We implement this internal trait for any [Result] type.
trait CanMapToRespErr<T> {
    fn map_resp_err(self, htmx: &Option<HtmxContext>) -> Result<T, ResponseError>;
}

impl<T, E> CanMapToRespErr<T> for Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn map_resp_err(self, htmx: &Option<HtmxContext>) -> Result<T, ResponseError> {
        self.map_err(|e| ResponseError(e.into(), htmx.clone()))
    }
}

async fn not_found_handler(uri: Uri, headers: HeaderMap) -> Response {
    views::not_found_page(uri, HtmxContext::try_from(&headers).ok()).into_response()
}

async fn get_image_handler(
    State(store): State<Arc<Store>>,
    headers: HeaderMap,
    uri: Uri,
    Path(slug): Path<String>,
) -> Result<Response, ResponseError> {
    if let Some(content) = match slug.as_str() {
        "favicon.svg" => Some("<svg version='1.0' xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' viewBox='0 0 64 64' enable-background='new 0 0 64 64' xml:space='preserve'><g><g><polygon fill='#F9EBB2' points='46,3.414 46,14 56.586,14 '/><path fill='#F9EBB2' d='M45,16c-0.553,0-1-0.447-1-1V2H8C6.896,2,6,2.896,6,4v56c0,1.104,0.896,2,2,2h48c1.104,0,2-0.896,2-2V16 H45z'/></g><path fill='#394240' d='M14,26c0,0.553,0.447,1,1,1h34c0.553,0,1-0.447,1-1s-0.447-1-1-1H15C14.447,25,14,25.447,14,26z'/><path fill='#394240' d='M49,37H15c-0.553,0-1,0.447-1,1s0.447,1,1,1h34c0.553,0,1-0.447,1-1S49.553,37,49,37z'/><path fill='#394240' d='M49,43H15c-0.553,0-1,0.447-1,1s0.447,1,1,1h34c0.553,0,1-0.447,1-1S49.553,43,49,43z'/><path fill='#394240' d='M49,49H15c-0.553,0-1,0.447-1,1s0.447,1,1,1h34c0.553,0,1-0.447,1-1S49.553,49,49,49z'/><path fill='#394240' d='M49,31H15c-0.553,0-1,0.447-1,1s0.447,1,1,1h34c0.553,0,1-0.447,1-1S49.553,31,49,31z'/><path fill='#394240' d='M15,20h16c0.553,0,1-0.447,1-1s-0.447-1-1-1H15c-0.553,0-1,0.447-1,1S14.447,20,15,20z'/><path fill='#394240' d='M59.706,14.292L45.708,0.294C45.527,0.112,45.277,0,45,0H8C5.789,0,4,1.789,4,4v56c0,2.211,1.789,4,4,4h48 c2.211,0,4-1.789,4-4V15C60,14.723,59.888,14.473,59.706,14.292z M46,3.414L56.586,14H46V3.414z M58,60c0,1.104-0.896,2-2,2H8 c-1.104,0-2-0.896-2-2V4c0-1.104,0.896-2,2-2h36v13c0,0.553,0.447,1,1,1h13V60z'/><polygon opacity='0.15' fill='#231F20' points='46,3.414 56.586,14 46,14 '/></g></svg>"),
        "bluesky.svg" => Some(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 -3.268 64 68.414"><path fill="darkslategray" d="M13.873 3.805C21.21 9.332 29.103 20.537 32 26.55v15.882c0-.338-.13.044-.41.867-1.512 4.456-7.418 21.847-20.923 7.944-7.111-7.32-3.819-14.64 9.125-16.85-7.405 1.264-15.73-.825-18.014-9.015C1.12 23.022 0 8.51 0 6.55 0-3.268 8.579-.182 13.873 3.805zm36.254 0C42.79 9.332 34.897 20.537 32 26.55v15.882c0-.338.13.044.41.867 1.512 4.456 7.418 21.847 20.923 7.944 7.111-7.32 3.819-14.64-9.125-16.85 7.405 1.264 15.73-.825 18.014-9.015C62.88 23.022 64 8.51 64 6.55c0-9.818-8.578-6.732-13.873-2.745z"/></svg>"##),
        "github.svg" => Some(r##"<svg width="98" height="96" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M48.854 0C21.839 0 0 22 0 49.217c0 21.756 13.993 40.172 33.405 46.69 2.427.49 3.316-1.059 3.316-2.362 0-1.141-.08-5.052-.08-9.127-13.59 2.934-16.42-5.867-16.42-5.867-2.184-5.704-5.42-7.17-5.42-7.17-4.448-3.015.324-3.015.324-3.015 4.934.326 7.523 5.052 7.523 5.052 4.367 7.496 11.404 5.378 14.235 4.074.404-3.178 1.699-5.378 3.074-6.6-10.839-1.141-22.243-5.378-22.243-24.283 0-5.378 1.94-9.778 5.014-13.2-.485-1.222-2.184-6.275.486-13.038 0 0 4.125-1.304 13.426 5.052a46.97 46.97 0 0 1 12.214-1.63c4.125 0 8.33.571 12.213 1.63 9.302-6.356 13.427-5.052 13.427-5.052 2.67 6.763.97 11.816.485 13.038 3.155 3.422 5.015 7.822 5.015 13.2 0 18.905-11.404 23.06-22.324 24.283 1.78 1.548 3.316 4.481 3.316 9.126 0 6.6-.08 11.897-.08 13.526 0 1.304.89 2.853 3.316 2.364 19.412-6.52 33.405-24.935 33.405-46.691C97.707 22 75.788 0 48.854 0z" fill="darkslategray"/></svg>"##),
        "link.svg" => Some(r##"<svg fill="#9b4dca" viewBox="0 0 8 8" xmlns="http://www.w3.org/2000/svg"><path d="M0 0v8h8v-2h-1v1h-6v-6h1v-1h-2zm4 0l1.5 1.5-2.5 2.5 1 1 2.5-2.5 1.5 1.5v-4h-4z" /></svg>"##),
        _ => None,
    } {
        let mut hm = HeaderMap::new();
        hm.insert("Content-Type", HeaderValue::from_static("image/svg+xml"));
        return Ok((StatusCode::OK, hm, content).into_response())
    };

    let htmx_context = HtmxContext::try_from(&headers).ok();

    let img = Image::try_from_path_part(PathPart::from(slug)).unwrap_or_default();

    let can_html = htmx_context.is_some()
        || headers
            .get("Accept")
            .is_some_and(|hv| hv.to_str().is_ok_and(|t| t.contains("text/html")));
    if can_html {
        if !store.check_image_exists(&img).await.map_resp_err(&htmx_context)? {
            return Ok(views::not_found_page(uri, htmx_context).into_response());
        }
        return Ok(views::get_image_page(img.to_original(), htmx_context).into_response());
    }

    if let Some(image) = store.get_image_raw(&img).await.map_resp_err(&htmx_context)? {
        let mut hm = HeaderMap::new();
        hm.insert("Content-Type", img.to_content_type());
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
    let htmx_context = HtmxContext::try_from(&headers).ok();
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
    let htmx_context = HtmxContext::try_from(&headers).ok();
    if let Some((post, content)) = store.get_post_raw(&slug).await.map_resp_err(&htmx_context)? {
        let content_html = convert(content.as_str(), HashSet::default()).map_resp_err(&htmx_context)?;
        Ok(views::get_post_page(post, PreEscaped(content_html), htmx_context).into_response())
    } else {
        Ok(views::not_found_page(uri, htmx_context).into_response())
    }
}

async fn get_favicon_ico_handler() -> Response {
    let mut hm = HeaderMap::new();
    hm.insert("Location", HeaderValue::from_static("/images/favicon.svg"));
    (StatusCode::TEMPORARY_REDIRECT, hm).into_response()
}

async fn livez_handler() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn readyz_handler(State(store): State<Arc<Store>>) -> Result<Response, ResponseError> {
    store.readyz().await.map_resp_err(&None)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
