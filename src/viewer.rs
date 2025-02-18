mod views;

use crate::conversion::convert;
use crate::htmx::HtmxContext;
use crate::statics::{get_favicon_ico_handler, get_static_handler};
use crate::store::{Image, Store};
use crate::{conversion, statics};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::Datelike;
use http::Request;
use itertools::Itertools;
use log::info;
use maud::PreEscaped;
use object_store::path::PathPart;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tower_http::trace::{MakeSpan, TraceLayer};
use tracing::{instrument, Span};

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
                .make_span_with(HttpTraceLayerHooks)
                .on_request(HttpTraceLayerHooks)
                .on_response(HttpTraceLayerHooks)
                .on_failure(HttpTraceLayerHooks),
        );
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cfg.port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Default, Clone)]
struct HttpTraceLayerHooks;

impl<B> MakeSpan<B> for HttpTraceLayerHooks {
    fn make_span(&mut self, req: &Request<B>) -> Span {
        // see https://github.com/open-telemetry/semantic-conventions/blob/main/docs/http/http-spans.md
        let span = tracing::info_span!(
            // span name here will be ignored by open telemetry and replaced with otel.name
            "request",
            // the span name
            otel.name = tracing::field::Empty,
            otel.kind = "server",
            otel.status_code = tracing::field::Empty,
            // common attributes
            http.response.status_code = tracing::field::Empty,
            http.request.body.size = tracing::field::Empty,
            http.response.body.size = tracing::field::Empty,
            http.request.method = req.method().as_str(),
            network.protocol.name = "http",
            network.protocol.version = format!("{:?}", req.version()).strip_prefix("HTTP/"),
            user_agent.original = tracing::field::Empty,
            // http request and response headers
            http.request.header.x_forwarded_for = tracing::field::Empty,
            http.request.header.cf_ipcountry = tracing::field::Empty,
            http.request.header.referer = tracing::field::Empty,
            // http server
            http.route = tracing::field::Empty,
            server.address = tracing::field::Empty,
            server.port = tracing::field::Empty,
            url.path = req.uri().path(),
            url.query = tracing::field::Empty,
            url.scheme = tracing::field::Empty,
            // set on the failure hook
            "error.type" = tracing::field::Empty,
            error = tracing::field::Empty,
        );

        req.uri().query().map(|v| span.record("url.query", v));
        req.uri().scheme().map(|v| span.record("url.scheme", v.as_str()));
        req.uri().host().map(|v| span.record("server.address", v));
        req.uri().port_u16().map(|v| span.record("server.port", v));

        req.headers()
            .get(http::header::CONTENT_LENGTH)
            .map(|v| v.to_str().map(|v| span.record("http.request.body.size", v)));
        req.headers()
            .get(http::header::USER_AGENT)
            .map(|v| v.to_str().map(|v| span.record("user_agent.original", v)));
        req.headers()
            .get("X-Forwarded-For")
            .map(|v| v.to_str().map(|v| span.record("http.request.header.x_forwarded_for", v)));
        req.headers()
            .get("CF-IPCountry")
            .map(|v| v.to_str().map(|v| span.record("http.request.header.cf_ipcountry", v)));
        req.headers()
            .get("Referer")
            .map(|v| v.to_str().map(|v| span.record("http.request.header.referer", v)));

        if let Some(path) = req.extensions().get::<axum::extract::MatchedPath>() {
            span.record("otel.name", format!("{} {}", req.method(), path.as_str()));
            span.record("http.route", path.as_str());
        } else {
            span.record("otel.name", format!("{} -", req.method()));
        };

        span
    }
}

impl<B> tower_http::trace::OnRequest<B> for HttpTraceLayerHooks {
    fn on_request(&mut self, _: &Request<B>, _: &Span) {
        tracing::event!(tracing::Level::DEBUG, "start processing request");
    }
}

impl<B> tower_http::trace::OnResponse<B> for HttpTraceLayerHooks {
    fn on_response(self, response: &Response<B>, _: std::time::Duration, span: &Span) {
        if let Some(size) = response.headers().get(http::header::CONTENT_LENGTH) {
            span.record("http.response.body.size", size.to_str().unwrap_or_default());
        }
        span.record("http.response.status_code", response.status().as_u16());

        // Server errors are handled by the OnFailure hook
        if !response.status().is_server_error() {
            tracing::event!(tracing::Level::INFO, "finished processing request");
        }
    }
}

impl<FailureClass> tower_http::trace::OnFailure<FailureClass> for HttpTraceLayerHooks
where
    FailureClass: std::fmt::Display,
{
    fn on_failure(&mut self, error: FailureClass, _: std::time::Duration, _: &Span) {
        tracing::event!(
            tracing::Level::ERROR,
            error = error.to_string(),
            "error.type" = "_OTHER",
            "response failed"
        );
    }
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
