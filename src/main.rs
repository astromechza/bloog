mod pathutils;

use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, head};
use axum::{Form, Router};
use chrono::prelude::*;
use validator::{Validate, ValidationError, ValidationErrors};
use chrono::NaiveDate;
use clap::{arg, ArgMatches, Command};
use futures::{stream, StreamExt, TryStreamExt};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::ImageReader;
use itertools::Itertools;
use maud::{html, Markup, PreEscaped, Render};
use object_store::local::LocalFileSystem;
use object_store::{parse_url_opts, ObjectStore, PutOptions, PutPayload};
use std::io::Cursor;
use std::sync::{Arc, RwLock};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use object_store::path::PathPart;
use tokio::sync::Mutex;
use tower_http::trace;
use tracing::{debug_span, error, info, info_span, instrument, Instrument, Level};
use tower_http::trace::{TraceLayer};
use tracing_subscriber::filter::FilterExt;
use tracing_subscriber::fmt::format::FmtSpan;
use url::Url;
use base64::prelude::*;
use futures::future::Lazy;
use futures::stream::FuturesUnordered;
use regex::Regex;
use serde::{Deserialize, Serialize};

const EDITOR_COMMAND: &str = "editor";
const VIEWER_COMMAND: &str = "viewer";
const MEDIUM_VARIANT_WIDTH: u32 = 1000;
const MEDIUM_VARIANT_HEIGHT: u32 = 1000;
const THUMB_VARIANT_WIDTH: u32 = 200;
const THUMB_VARIANT_HEIGHT: u32 = 200;

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

struct SharedState {
    object_store: Box<dyn ObjectStore>,
    object_store_path: object_store::path::Path,
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
                let image_path = state.object_store_path.child("images").child(format!("{}-{}", Local::now().format("%Y%m%dT%H%M%S"), name));

                let data = match field.bytes().await {
                    Ok(data) => data,
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, format!("failed to read body: {}", e)).into_response();
                    }
                };

                let image_reader = ImageReader::new(Cursor::new(data))
                    .with_guessed_format()
                    .expect("reader should not fail on buffered data");

                let span = info_span!("decode");
                let image = match image_reader.decode() {
                    Ok(image) => image,
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, format!("failed to decode image: {}", e)).into_response();
                    }
                };
                drop(span);

                let span = info_span!("resize_medium");
                let medium = if image.width() > MEDIUM_VARIANT_WIDTH || image.height() > MEDIUM_VARIANT_HEIGHT {
                    image.resize(MEDIUM_VARIANT_WIDTH, MEDIUM_VARIANT_HEIGHT, image::imageops::FilterType::Lanczos3)
                } else {
                    image.clone()
                };
                drop(span);

                let span = info_span!("resize_thumbnail");
                let thumbnail = image.thumbnail(THUMB_VARIANT_WIDTH, THUMB_VARIANT_HEIGHT);
                drop(span);

                let mut original_data = vec![];
                image.write_with_encoder(WebPEncoder::new_lossless(&mut original_data)).expect("image translation should never fail");
                if let Err(e) = state.object_store.put(&image_path.child("original.webp"), PutPayload::from(original_data))
                    .instrument(info_span!("object_store_put")).await {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to write original: {}", e)).into_response();
                }

                let mut medium_data = vec![];
                medium.write_with_encoder(WebPEncoder::new_lossless(&mut medium_data)).expect("image translation should never fail");
                if let Err(e) = state.object_store.put(&image_path.child("medium.webp"), PutPayload::from(medium_data))
                    .instrument(info_span!("object_store_put")).await {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to write medium variant: {}", e)).into_response();
                }

                let mut thumbnail_data = vec![];
                thumbnail.write_with_encoder(WebPEncoder::new_lossless(&mut thumbnail_data)).expect("image translation should never fail");
                if let Err(e) = state.object_store.put(&image_path.child("thumb.webp"), PutPayload::from(thumbnail_data))
                    .instrument(info_span!("object_store_put")).await {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to write thumbnail variant: {}", e)).into_response();
                }
            }
            _ => {
                return (StatusCode::BAD_REQUEST, "unsupported form field name").into_response();
            }
        }
    }
    StatusCode::OK.into_response()
}

#[instrument(skip(state))]
async fn editor_image_delete(State(state): State<Arc<SharedState>>, Path(id): Path<String>) -> Response {
    let image_path = state.object_store_path.child("images").child(id);
    let variant_paths = state.object_store.list(Some(&image_path))
        .map_ok(|m| m.location)
        .boxed();
    let deleted_paths = state.object_store.delete_stream(variant_paths)
        .try_collect::<Vec<object_store::path::Path>>()
        .instrument(info_span!("object_store_delete_stream"))
        .await
        .unwrap();
    if deleted_paths.is_empty() {
        StatusCode::NOT_FOUND.into_response()
    } else {
        StatusCode::OK.into_response()
    }
}

#[instrument(skip(state))]
async fn editor_image_browse(State(state): State<Arc<SharedState>>) -> Response {
    let image_ids = state.object_store.list_with_delimiter(Some(&state.object_store_path.child("images"))).instrument(info_span!("object_store_list"))
        .await.unwrap()
        .common_prefixes.iter()
        .map(|m| m.filename().unwrap().to_string())
        .collect_vec();

    html! {
        head {
            script src="https://unpkg.com/htmx.org@2.0.3" integrity="sha384-0895/pl2MU10Hqc6jd4RvrthNlDiE9U1tWmX7WRESftEDRosgxNsQG/Ze9YMRzHq" crossorigin="anonymous" {};
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
                    a href={ "/images/" (id) "medium.webp" } {
                        img src={ "/images/" (id) ".thumb.webp" };
                        figcaption { (id) }
                    }
                }
            }
        }
    }.into_response()
}


fn id_and_variant_from_path(path: impl AsRef<str>) -> Option<(String, String)> {
    let parts = path.as_ref().rsplitn(3, ".").collect_vec();
    match parts.len() {
        2 => Some((parts[1].to_string(), format!("original.{}", parts[0]))),
        3 => Some((parts[2].to_string(), format!("{}.{}", parts[1], parts[0]))),
        _ => None
    }
}

fn content_type_header(variant: impl AsRef<str>) -> HeaderValue {
    if variant.as_ref().ends_with(".jpg") {
        HeaderValue::from_str("image/jpg").unwrap()
    } else if variant.as_ref().ends_with(".webp") {
        HeaderValue::from_str("image/webp").unwrap()
    } else {
        HeaderValue::from_str("application/octet-stream").unwrap()
    }
}

#[instrument(skip(state))]
async fn editor_image_head(State(state): State<Arc<SharedState>>, Path(variant): Path<String>) -> Response {
    let (id, variant) = match id_and_variant_from_path(&variant) {
        Some(x) => x,
        None => {
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    match state.object_store.head(&state.object_store_path.child("images").child(id).child(variant.clone())).instrument(info_span!("object_store_head")).await {
        Ok(gr) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", content_type_header(&variant));
            headers.insert("Content-Length", HeaderValue::from(gr.size));
            (StatusCode::OK, headers).into_response()
        }
        Err(object_store::Error::NotFound{..}) =>  StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[instrument(skip(state))]
async fn editor_image_get(State(state): State<Arc<SharedState>>, Path(variant): Path<String>) -> Response {
    let (id, variant) = match id_and_variant_from_path(&variant) {
        Some(x) => x,
        None => {
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    match state.object_store.get(&state.object_store_path.child("images").child(id).child(variant.clone())).instrument(info_span!("object_store_get")).await {
        Ok(gr) => {
            match gr.bytes().await {
                Ok(bytes) => {
                    let mut headers = HeaderMap::new();
                    headers.insert("Content-Type", content_type_header(&variant));
                    (StatusCode::OK, headers, bytes).into_response()
                }
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
        Err(object_store::Error::NotFound{..}) =>  StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}


#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord)]
enum PostContentType {
    Markdown,
    RestructuredText,
}

#[derive(Debug,Serialize,Deserialize,Clone,PartialEq,Eq,PartialOrd,Ord)]
enum PostMetadata {
    V1((NaiveDate, String, PostContentType)),
}

#[derive(Debug)]
struct Post {
    date: NaiveDate,
    slug: String,
    title: String,
    content_type: PostContentType,
    labels: Vec<String>,
}

#[instrument(skip(state))]
async fn editor_posts_browse(State(state): State<Arc<SharedState>>) -> Response {
    let objects_paths = state.object_store
        .list(Some(&state.object_store_path.child("posts")))
        .map_ok(|i| object_store::path::Path::from() i.location.as_ref())
        .boxed()
        .try_collect::<Vec<object_store::path::Path>>()
        .instrument(info_span!("object_store_list"))
        .await
        .unwrap();

    let posts = objects_paths.iter()
        .into_group_map_by(|f| f.parts().nth(1))
        .iter()
        .filter_map(|e| {
            info!("a");
            if e.0.is_none() {
                return None;
            }
            info!("x");
            let slug = e.0.clone().unwrap().as_ref().to_string();
            let props_part = match e.1.iter().find(|path| {
                info!("piter {:?} {:?} {:?}", path.parts().nth(2), path, state.object_store_path);
                path.parts().nth(2).filter(|pp| pp.as_ref() == "props").is_some()
            }) {
                Some(p) => match p.parts().nth(3) {
                    Some(p) => p,
                    None => {
                        error!("A");
                        return None
                    },
                },
                None => {
                    return None
                },
            };
            info!("b");
            let props_bytes = BASE64_STANDARD_NO_PAD.decode(props_part.as_ref().as_bytes()).unwrap();
            let props: PostMetadata = postcard::from_bytes(&props_bytes).unwrap();
            info!("c");
            let labels = e.1.iter().filter_map(|p| {
                let mut iter = p.parts();
                if iter.nth(2).map(|pp| pp.as_ref() == "labels").is_some() {
                    iter.next().map(|pp| pp.as_ref().to_string())
                } else {
                    None
                }
            }).sorted().collect_vec();
            info!("d labels {:?}", labels);

            info!("props: {:?}", props);
            match props { 
                PostMetadata::V1((nd, ttl, cs)) => {
                    info!("d");
                    Some(Post{
                        date: nd,
                        slug,
                        title: ttl,
                        content_type: cs,
                        labels,
                    })
                } 
            }
        }).collect::<Vec<Post>>();
    info!("posts: {:?}", posts);

    html! {
        head {
            script src="https://unpkg.com/htmx.org@2.0.3" integrity="sha384-0895/pl2MU10Hqc6jd4RvrthNlDiE9U1tWmX7WRESftEDRosgxNsQG/Ze9YMRzHq" crossorigin="anonymous" {};
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
    }.into_response()
}

use lazy_static::lazy_static;

lazy_static! {
    static ref RE_ALLOWED_SLUG: Regex = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap();
    static ref RE_COMMA_SEP_LABELS: Regex = Regex::new(r"^([a-z0-9]+(-[a-z0-9]+)*(,[a-z0-9]+(-[a-z0-9]+)*)*)?$").unwrap();
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

    let props = PostMetadata::V1((form.date, form.title, form.content_type));
    let raw_props = STANDARD_NO_PAD.encode(postcard::to_stdvec(&props).unwrap());
    let slug_path = state.object_store_path.child("posts").child(form.slug);

    state.object_store.put_opts(&slug_path.child(raw_props), PutPayload::default(), PutOptions::default()).await.unwrap();
    state.object_store.put_opts(&slug_path.child("content"), PutPayload::from(form.content), PutOptions::default()).await.unwrap();
    FuturesUnordered::from_iter(form.labels.split(',')
        .map(| lbl| {
            let label_path = slug_path.child("labels").child(lbl).to_owned();
            let cloned_state = state.clone();
            async move {
                cloned_state.object_store.put_opts(&label_path, PutPayload::default(), PutOptions::default()).await
            }
        })).boxed()
        .try_collect::<Vec<_>>().await.unwrap();

    StatusCode::OK.into_response()
}

async fn editor(am: &ArgMatches) {
    println!("{:?}", am);
    let su = am.get_one::<Url>("uri").unwrap();
    let (os, path) = if su.scheme() != "file" {
        parse_url_opts(su, su.query_pairs().map(|i| (i.0.to_string(), i.1.to_string())).collect_vec()).unwrap()
    } else {
        let mut los = LocalFileSystem::new();
        los = los.with_automatic_cleanup(true);
        let bos: Box<dyn ObjectStore> = Box::new(los);
        (bos, object_store::path::Path::from_url_path(su.path()).unwrap())
    };

    // build our application with a single route
    let app = Router::new()
        .route("/posts/", get(editor_posts_browse))
        .route("/posts/", post(editor_posts_create))
        .route("/images/", get(editor_image_browse))
        .route("/images/", post(editor_image_upload))
        .route("/images/:variant", get(editor_image_get))
        .route("/images/:variant", head(editor_image_head))
        .route("/images/:id/actions/delete", post(editor_image_delete))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(TraceLayer::new_for_http()
            .make_span_with(trace::DefaultMakeSpan::new()
                .level(Level::INFO))
            .on_response(trace::DefaultOnResponse::new()
                .level(Level::INFO)))
        .with_state(Arc::new(SharedState{
            object_store: os,
            object_store_path: path,
        }));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", am.get_one::<u16>("port").unwrap())).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn viewer(am: &ArgMatches) {
    println!("{:?}", am);
    let su = am.get_one::<Url>("uri").unwrap();
    println!("{:?}", su);
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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use serde::{Deserialize, Serialize};
    use crate::{NewPostForm, PostLabel, PostContentType, PostMetadata};

    #[test]
    fn test_ser_der() {
        let p = PostMetadata::V1((NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), "fizz".to_string(), PostContentType::Markdown));
        let b = postcard::to_allocvec(&p).unwrap();
        assert_eq!(b.len(), 18);
        assert_eq!(b, vec![
            // enum 0
            0,
            // Note: this just falls back to RFC3339 string encoding (4 + 1 + 2 + 1 + 2 == 10) for the date. Postcard doesn't seem to
            // have a binary representation but I'm not sure I care.
            10, 50, 48, 50, 52, 45, 48, 49, 45, 48, 50,
            // String encoding.
            4, 102, 105, 122, 122,
            // enum 0
            0,
        ]);
        let p2 = postcard::from_bytes(b.as_slice()).unwrap();
        assert_eq!(p, p2);
    }

}