mod views;

use super::store::{ImageVariant, Post, Store};
use crate::htmx::HtmxContext;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Router};
use std::sync::Arc;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::routing::{delete, get, post};
use chrono::NaiveDate;
use image::EncodableLayout;
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
        .route("/images", get(list_images_handler))
        .route("/images", post(submit_image_handler))
        .route("/images/{slug}", get(get_image_handler))
        .route("/images/{slug}", delete(submit_delete_image_handler))
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
        published: form.published.unwrap_or_default(),
        labels: form.labels.split(",")
            .filter_map(|s| Some(s.to_string()).filter(|s| !s.is_empty()))
            .collect(),
    };
    if store.get_post_raw(form.slug.as_str()).await.map_resp_err(&htmx_context)?.is_some() {
        return Ok(views::new_posts_page(Some((&temporary_post, form.raw_content.as_str())), Some("slug already exists".to_string()), htmx_context));
    }
    if let Err(e) = store.upsert_post(&temporary_post, form.raw_content.as_str()).await {
        return Ok(views::new_posts_page(Some((&temporary_post, form.raw_content.as_str())), Some(e.to_string()), htmx_context));
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
        Some((post, raw_content)) => {

            let parser = pulldown_cmark::Parser::new(raw_content.as_str());
            let mut html_output = String::new();
            pulldown_cmark::html::push_html(&mut html_output, parser);

            Ok(views::edit_posts_page(post, raw_content, html_output, None, htmx_context))
        },
        None => Ok(views::not_found_page(Method::GET, uri, HtmxContext::try_from(&headers).ok()))
    }
}

#[derive(Debug,Default,Deserialize)]
struct EditPostForm {
    title: String,
    date: NaiveDate,
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
        published: form.published.unwrap_or_default(),
        labels: form.labels.split(",")
            .filter_map(|s| Some(s.to_string()).filter(|s| !s.is_empty()))
            .collect(),
    };

    let parser = pulldown_cmark::Parser::new(form.raw_content.as_str());
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);

    if let Err(e) = store.upsert_post(&temporary_post, form.raw_content.as_str()).await {
        Ok(views::edit_posts_page(temporary_post, form.raw_content, html_output, Some(e.to_string()), htmx_context))
    } else {
        Ok(views::edit_posts_page(temporary_post, form.raw_content, html_output, None, htmx_context))
    }
}

fn redirect_response(to: &str, htmx_context: Option<HtmxContext>) -> Result<Response, ResponseError> {
    match htmx_context {
        None => Ok(Redirect::to(to).into_response()),
        Some(_) => {
            let mut hm = HeaderMap::new();
            hm.insert("HX-Location", HeaderValue::from_str(to).map_resp_err(&htmx_context)?);
            Ok((StatusCode::NO_CONTENT, hm).into_response())
        }
    }
}

async fn submit_delete_post_handler(State(store): State<Arc<Store>>, headers: HeaderMap, Path(slug): Path<String>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    store.delete_post(slug.as_str()).await.map_resp_err(&htmx_context)?;
    redirect_response("/posts", htmx_context)
}

async fn debug_handler(State(store): State<Arc<Store>>, headers: HeaderMap) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let objects = store.list_object_meta().await.map_resp_err(&htmx_context)?;
    Ok(views::debug_objects_page(objects, htmx_context).into_response())
}

async fn list_images_handler(State(store): State<Arc<Store>>, headers: HeaderMap) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let images = store.list_images().await.map_resp_err(&htmx_context)?;
    Ok(views::list_images_page(images, None, htmx_context).into_response())
}

async fn submit_image_handler(State(store): State<Arc<Store>>, headers: HeaderMap, mut multipart: Multipart) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let error: Option<anyhow::Error> = match multipart.next_field().await.map_resp_err(&htmx_context)? {
        Some(f) if f.name().is_some_and(|x| x == "slug") => {
            let pre_slug = f.text().await.map_resp_err(&htmx_context)?;
            match multipart.next_field().await.map_resp_err(&htmx_context)? {
                Some(f) if f.name().is_some_and(|x| x == "image") => {
                    let image_bytes = f.bytes().await.map_resp_err(&htmx_context)?;
                    store.create_image(pre_slug.as_str(), image_bytes.as_bytes()).await.err()
                },
                _ => Some(anyhow::anyhow!("Multipart missing image field"))
            }
        },
        _ => Some(anyhow::anyhow!("Multipart missing slug field")),
    };
    let images = store.list_images().await.map_resp_err(&htmx_context)?;
    Ok(views::list_images_page(images, error.map(|e| e.to_string()), htmx_context).into_response())
}

async fn get_image_handler(State(store): State<Arc<Store>>, headers: HeaderMap, Path(slug_variant): Path<String>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    let (slug, raw_variant) = slug_variant.split_once('.').unwrap_or((slug_variant.as_str(), ""));
    if raw_variant.is_empty() {
        Ok(views::get_image_page(slug.to_string(), htmx_context).into_response())
    } else {
        match ImageVariant::try_from(raw_variant) {
            Err(_) => Ok(StatusCode::NOT_FOUND.into_response()),
            Ok(variant) => {
                if let Some(image) = store.get_image_raw(slug, variant.clone()).await.map_resp_err(&htmx_context)? {
                    let mut hm = HeaderMap::new();
                    hm.insert("Content-Type", variant.into());
                    Ok((StatusCode::OK, hm, image).into_response())
                } else {
                    Ok(StatusCode::NOT_FOUND.into_response())
                }
            }
        }
    }
}

async fn submit_delete_image_handler(State(store): State<Arc<Store>>, headers: HeaderMap, Path(slug): Path<String>) -> Result<Response, ResponseError> {
    let htmx_context = HtmxContext::try_from(&headers).ok();
    store.delete_image(slug.as_str()).await.map_resp_err(&htmx_context)?;
    redirect_response("/images", htmx_context)
}
