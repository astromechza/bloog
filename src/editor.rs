mod views;

use super::store::{Post, PostContentType, Store};
use crate::htmx::HtmxContext;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Router};
use std::sync::Arc;
use anyhow::anyhow;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::routing::{delete, get, post};
use chrono::NaiveDate;
use serde::Deserialize;

#[derive(Debug,Eq,PartialEq,Ord, PartialOrd,Clone)]
pub struct Config {
    pub port: u16,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            port: 8080,
        }
    }
}

pub async fn run(cfg: Config, store: Store) -> Result<(), anyhow::Error> {
    let app = Router::new()
        .route("/", get(home_handler))
        .route("/images", get(|| async { Result::<Response, ResponseError>::Err(ResponseError(anyhow!("not implemented"), None)) }))
        .route("/images", post(|| async { Result::<Response, ResponseError>::Err(ResponseError(anyhow!("not implemented"), None)) }))
        .route("/posts", get(posts_handler))
        .route("/posts/new", get(new_post_handler))
        .route("/posts/new", post(submit_new_post_handler))
        .route("/posts/{id}", get(edit_post_handler))
        .route("/posts/{id}", post(submit_edit_post_handler))
        .route("/posts/{id}", delete(submit_delete_post_handler))
        .route("/debug", get(debug_handler))
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
        views::internal_error_page(self.0, self.1)
    }
}

/// This trait helps to attach the [HtmxContext] to the [Result] and convert any old error into
/// a [ResponseError]. We implement this internal trait for any [Result] type.
trait CanMapToRespErr<T> {
    fn map_resp_err(self, htmx: &Option<HtmxContext>) -> Result<T, ResponseError>;
}

impl<T, E> CanMapToRespErr<T> for Result<T, E> where E: Into<anyhow::Error> {
    fn map_resp_err(self, htmx: &Option<HtmxContext>) -> Result<T, ResponseError> {
        self.map_err(|e| ResponseError(e.into(), htmx.clone()))
    }
}

async fn not_found_handler(method: Method, uri: Uri, headers: HeaderMap) -> Result<Response, ResponseError> {
    Ok(views::not_found_page(method, uri, HtmxContext::try_from(&headers).ok()))
}

async fn home_handler(headers: HeaderMap) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    if htmx_context.is_none() {
        Ok(Redirect::to("/posts").into_response())
    } else {
        let mut hm = HeaderMap::new();
        hm.insert("HX-Location", HeaderValue::from_static("/posts"));
        Ok((StatusCode::OK, hm).into_response())
    }
}

async fn posts_handler(headers: HeaderMap, State(store): State<Arc<Store>>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let posts = store.list_posts().await.map_resp_err(&htmx_context)?;
    Ok(views::list_posts_page(posts, htmx_context))
}


async fn new_post_handler(headers: HeaderMap) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    Ok(views::new_posts_page(None, None, htmx_context))
}

#[derive(Debug,Default,Deserialize)]
struct NewPostForm {
    slug: String,
    title: String,
    date: NaiveDate,
    content_type: PostContentType,
    published: Option<bool>,
    raw_content: String,
    labels: String,
}

async fn submit_new_post_handler(State(store): State<Arc<Store>>, headers: HeaderMap, Form(form): Form<NewPostForm>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let temporary_post = Post{
        date: form.date,
        slug: form.slug.clone(),
        title: form.title,
        content_type: form.content_type,
        published: form.published.unwrap_or_default(),
        labels: form.labels.split(",")
            .filter_map(|s| Some(s.to_string()).filter(|s| !s.is_empty()))
            .collect(),
    };
    if store.get_post_raw(form.slug.as_str()).await.map_resp_err(&htmx_context)?.is_some() {
        return Ok(views::new_posts_page(Some((temporary_post, form.raw_content)), Some("slug already exists".to_string()), htmx_context));
    }
    if let Err(e) = store.upsert_post(&temporary_post, form.raw_content.as_str()).await {
        return Ok(views::new_posts_page(Some((temporary_post, form.raw_content)), Some(e.to_string()), htmx_context));
    }
    let redirect_to = format!("/posts/{}", form.slug);
    match htmx_context {
        None => Ok(Redirect::to(redirect_to.as_str()).into_response()),
        Some(_) => {
            let mut hm = HeaderMap::new();
            hm.insert("HX-Location", HeaderValue::from_str(redirect_to.as_str()).map_resp_err(&htmx_context)?);
            Ok((StatusCode::CREATED, hm).into_response())
        }
    }
}

async fn edit_post_handler(uri: Uri, Path(id): Path<String>, headers: HeaderMap, State(store): State<Arc<Store>>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    match store.get_post_raw(&id).await.map_resp_err(&htmx_context)? {
        Some((post, raw_content)) => Ok(views::edit_posts_page(post, raw_content, None, htmx_context)),
        None => Ok(views::not_found_page(Method::GET, uri, HtmxContext::try_from(&headers).ok()))
    }
}

#[derive(Debug,Default,Deserialize)]
struct EditPostForm {
    title: String,
    date: NaiveDate,
    content_type: PostContentType,
    published: Option<bool>,
    raw_content: String,
    labels: String,
}

async fn submit_edit_post_handler(State(store): State<Arc<Store>>, headers: HeaderMap, Path(slug): Path<String>, Form(form): Form<EditPostForm>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let temporary_post = Post{
        date: form.date,
        slug: slug.clone(),
        title: form.title,
        content_type: form.content_type,
        published: form.published.unwrap_or_default(),
        labels: form.labels.split(",")
            .filter_map(|s| Some(s.to_string()).filter(|s| !s.is_empty()))
            .collect(),
    };
    if let Err(e) = store.upsert_post(&temporary_post, form.raw_content.as_str()).await {
        Ok(views::edit_posts_page(temporary_post, form.raw_content, Some(e.to_string()), htmx_context))
    } else {
        Ok(views::edit_posts_page(temporary_post, form.raw_content, None, htmx_context))
    }
}

async fn submit_delete_post_handler(State(store): State<Arc<Store>>, headers: HeaderMap, Path(slug): Path<String>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    store.delete_post(slug.as_str()).await.map_resp_err(&htmx_context)?;
    let redirect_to = "/posts";
    match htmx_context {
        None => Ok(Redirect::to(redirect_to).into_response()),
        Some(_) => {
            let mut hm = HeaderMap::new();
            hm.insert("HX-Location", HeaderValue::from_str(redirect_to).map_resp_err(&htmx_context)?);
            Ok((StatusCode::NO_CONTENT, hm).into_response())
        }
    }
}

async fn debug_handler(State(store): State<Arc<Store>>, headers: HeaderMap) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let objects = store.list_object_meta().await.map_resp_err(&htmx_context)?;
    Ok(views::debug_objects_page(objects, htmx_context).into_response())
}
