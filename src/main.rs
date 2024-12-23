mod path_utils;
mod store;

use crate::store::{ImageVariant, Post, PostContentType, Store};
use anyhow::{anyhow, Error};
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use chrono::NaiveDate;
use clap::{arg, ArgMatches, Command};
use itertools::Itertools;
use maud::{html, Markup};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;
use tower_http::trace;
use tower_http::trace::TraceLayer;
use tracing::{error, instrument, Level};
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;
use validator::Validate;
use lazy_static::lazy_static;

const EDITOR_COMMAND: &str = "editor";
const VIEWER_COMMAND: &str = "viewer";

lazy_static! {
    static ref RE_ALLOWED_SLUG: Regex = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap();
    static ref RE_COMMA_SEP_LABELS: Regex = Regex::new(r"^([a-z0-9]+(-[a-z0-9]+)*(,[a-z0-9]+(-[a-z0-9]+)*)*)?$").unwrap();
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_span_events(FmtSpan::CLOSE)
        .init();
    match cli().get_matches().subcommand() {
        Some((EDITOR_COMMAND, matches)) => editor(matches).await,
        Some((VIEWER_COMMAND, matches)) => viewer(matches).await,
        _ => {}
    }
}

fn build_common_head() -> Markup {
    html! {
        script src="https://unpkg.com/htmx.org@2.0.3" integrity="sha384-0895/pl2MU10Hqc6jd4RvrthNlDiE9U1tWmX7WRESftEDRosgxNsQG/Ze9YMRzHq" crossorigin="anonymous" {};
    }
}

struct SharedState {
    store: Store,
}

#[instrument(skip(state,multipart))]
async fn editor_image_upload(State(state): State<Arc<SharedState>>, mut multipart: Multipart) -> Response {
    while let Some(field) = multipart.next_field().await.unwrap() {
        match field.name() {
            Some("image") => {
                if field.file_name().is_none() {
                    return (StatusCode::BAD_REQUEST, "missing file name").into_response();
                }
                let name = match std::path::Path::new(field.file_name().unwrap()).file_stem() {
                    Some(file_stem) if !file_stem.is_empty() => file_stem.to_string_lossy().to_string(),
                    _ => {
                        return (StatusCode::BAD_REQUEST, "invalid file name").into_response();
                    }
                };
                let data = match field.bytes().await {
                    Ok(data) => data,
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, format!("failed to read body: {}", e)).into_response();
                    }
                };

                if let Err(e) = state.store.create_image(&name, data.as_ref()).await {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to create: {}", e)).into_response();
                }
            }
            _ => {
                return (StatusCode::BAD_REQUEST, "unsupported form field name").into_response()
            }
        }
    }
    StatusCode::OK.into_response()
}

#[instrument(skip(state))]
async fn editor_image_delete(State(state): State<Arc<SharedState>>, Path(id): Path<String>) -> Response {
    match state.store.delete_image(&id).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to create: {}", e)).into_response(),
    }
}

#[instrument(skip(state))]
async fn editor_posts_delete(state: State<Arc<SharedState>>, Path(id): Path<String>) -> Response {
    match state.store.delete_post(&id).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to delete: {}", e)).into_response(),
    }
}



#[instrument(skip(state))]
async fn editor_image_browse(State(state): State<Arc<SharedState>>) -> Response {
    let image_ids = state.store.list_images().await.unwrap();
    (html! {
        head {
            (build_common_head())
        }
        main {
            header {
                h1 { "Images" }
            }

            section {
                h2 { "Upload an image" }
                form id="upload-form" hx-encoding="multipart/form-data" hx-post="/images/" hx-swap="outerHTML" {
                    input form="upload-form" type="file" name="image";
                    button { "Upload "}
                    span id="upload-form-spinner" { "Uploading..." }
                }
            }

            section {
                h2 { "Thumbnails" }
                @for id in image_ids {
                    a href={ "/images/" (id) ".medium.webp" } {
                        img src={ "/images/" (id) ".thumbnail.webp" };
                        figcaption { (id) }
                    }
                }
            }
        }
    }).into_response()
}

fn id_and_variant_from_path(path: impl AsRef<str>) -> Result<(String, ImageVariant), Error> {
    let parts = path.as_ref().rsplitn(3, ".").collect_vec();
    if parts[0] != "webp" {
        Err(anyhow!("invalid extension '.{}'", parts[0]))
    } else if parts.len() == 2 {
        Ok((parts[1].to_string(), ImageVariant::Original))
    } else if parts.len() == 3 {
        Ok((parts[2].to_string(), ImageVariant::try_from(parts[1])?))
    } else {
        Err(anyhow!("invalid path"))
    }
}

#[instrument(skip(state))]
async fn editor_image_get(State(state): State<Arc<SharedState>>, Path(variant): Path<String>) -> Response {
    let (id, variant) = match id_and_variant_from_path(&variant) {
        Ok(x) => x,
        Err(e) => {
            error!("failed to get variant from path: {}", e);
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    match state.store.get_image_raw(&id, variant).await {
        Ok(Some(bytes)) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", HeaderValue::from_static("image/webp"));
            (StatusCode::OK, headers, bytes).into_response()
        }
        Ok(None) =>  StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[instrument(skip(state))]
async fn editor_posts_browse(State(state): State<Arc<SharedState>>) -> Response {
    let posts = state.store.list_posts().await.unwrap();
    (html! {
        head {
            (build_common_head())
        }
        main {
            header {
                h1 { "Posts" }
            }
            section {
                ul {
                    @for p in posts {
                        li {
                            span { (p.slug) }
                            span { (p.date) }
                            span { (p.title) }
                            span { ( format!("{:?}", p.content_type) ) }
                            span { ( format!("{:?}", p.labels) ) }
                        }
                    }
                }
            }
        }
    }).into_response()
}

#[derive(Debug,Serialize,Deserialize,Clone,Validate)]
struct NewPostForm {
    #[validate(length(min = 1, max = 50), regex(path = *RE_ALLOWED_SLUG))]
    slug: String,
    date: NaiveDate,
    #[validate(length(min = 1, max = 100), non_control_character)]
    title: String,
    content_type: PostContentType,
    #[validate(non_control_character)]
    content: String,
    #[validate(length(min = 1, max = 10), regex(path = *RE_COMMA_SEP_LABELS))]
    labels: String,
}

async fn editor_posts_create(State(state): State<Arc<SharedState>>, Form(form): Form<NewPostForm>) -> Response {
    if let Err(e) = form.validate() {
        return (StatusCode::BAD_REQUEST, format!("{}", e)).into_response();
    }
    state.store.upsert_post(&Post{
        date: form.date,
        slug: form.slug,
        title: form.title,
        content_type: form.content_type,
        labels: form.labels.split(',').map(&str::to_string).collect(),
    }, form.content.as_bytes()).await.unwrap();
    StatusCode::OK.into_response()
}

async fn editor_posts_get(State(state): State<Arc<SharedState>>, Path(id): Path<String>) -> Response {
    match state.store.get_post_raw(&id).await.unwrap() {
        Some((post, content)) => {
            (html!(
                head {
                    (build_common_head())
                }
                main {
                    header {
                        h1 { "Post " (post.title) }
                        span { (post.date) }
                        span { (post.labels.join(",")) }
                        span { (post.content_type) }
                    }
                    section {
                        pre {
                            code {
                                ( content )
                            }
                        }
                    }
                }
            )).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn editor(am: &ArgMatches) {
    let su = am.get_one::<Url>("uri").unwrap();
    let store = Store::from_url(su).unwrap();

    // build our application with a single route
    let app = Router::new()
        .route("/posts/", get(editor_posts_browse))
        .route("/posts/", post(editor_posts_create))
        .route("/posts/:id/actions/delete", post(editor_posts_delete))
        .route("/posts/:id", get(editor_posts_get))
        .route("/images/", get(editor_image_browse))
        .route("/images/", post(editor_image_upload))
        .route("/images/:variant", get(editor_image_get))
        .route("/images/:id/actions/delete", post(editor_image_delete))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(TraceLayer::new_for_http()
            .make_span_with(trace::DefaultMakeSpan::new()
                .level(Level::INFO))
            .on_response(trace::DefaultOnResponse::new()
                .level(Level::INFO)))
        .with_state(Arc::new(SharedState{store}));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", am.get_one::<u16>("port").unwrap())).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn viewer(am: &ArgMatches) {
    let _su = am.get_one::<Url>("uri").unwrap();
}

fn cli() -> Command {
    let uri_arg = arg!(<uri> "storage uri")
        .required(true)
        .value_parser(clap::value_parser!(Url))
        .env("BLOOG_URI");
    let port_arg = arg!(--port <port> "port number")
        .default_value("9000")
        .value_parser(clap::value_parser!(u16).range(1..65535))
        .env("BLOOG_PORT");
    Command::new("bloog")
        .subcommand_required(true)
        .subcommand(
            Command::new(EDITOR_COMMAND)
                .long_about("launch the web server with post editor and image uploader")
                .arg(uri_arg.clone())
                .arg(port_arg.clone())
                .arg_required_else_help(true))
        .subcommand(
            Command::new(VIEWER_COMMAND)
                .long_about("launch the web server in server mode")
                .arg(uri_arg.clone())
                .arg(port_arg.clone())
                .arg_required_else_help(true))
}
