use crate::htmx::HtmxContext;
use crate::store::{Image, Post};
use crate::viewhelpers::COMMON_CSS;
use anyhow::Error;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use chrono::Local;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use object_store::ObjectMeta;

fn render_body_html(title: impl AsRef<str>, inner: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                title { (title.as_ref()) }
                meta charset="utf-8";
                link rel="shortcut icon" type="image/svg" href="/favicon.svg";
                link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/modern-normalize/3.0.1/modern-normalize.min.css" integrity="sha512-q6WgHqiHlKyOqslT/lgBgodhd03Wp4BEqKeW6nNtlOY4quzyG3VoQKFrieaCeSnuVseNKRGpGeDU3qPmabCANg==" crossorigin="anonymous" referrerpolicy="no-referrer";
                link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/milligram/1.4.1/milligram.min.css" integrity="sha512-xiunq9hpKsIcz42zt0o2vCo34xV0j6Ny8hgEylN3XBglZDtTZ2nwnqF/Z/TTCc18sGdvCjbFInNd++6q3J0N6g==" crossorigin="anonymous" referrerpolicy="no-referrer";
                style {
                    (PreEscaped(COMMON_CSS))
                    r##"
                    textarea {
                      min-height: 50rem;
                      background: white;
                      font-family: monospace;
                    }
                    "##
                }
                script src="https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js" integrity="sha512-2kIcAizYXhIn8TzUvqzEDZNuDZ+aW7yE/+f1HJHXFjQcGNfv1kqzJSTBRBSlOgp6B/KZsz1K0a3ZTqP9dnxioQ==" crossorigin="anonymous" referrerpolicy="no-referrer" {};
            }
            body hx-boost="true" id="body" {
                (inner)
            }
        }
    }
}

pub(crate) fn render_body_semantics(header: &str, sections: Vec<Markup>) -> Markup {
    html! {
        main class="container" {
            header {
                nav.row {
                    a.button.button-clear.column href="/posts" { "Posts" }
                    a.button.button-clear.column href="/images" { "Images" }
                    a.button.button-clear.column href="/debug" { "Debug" }
                }
                h1 { (header) }
            }
            @for section in sections {
                section { (section) }
            }
        }
    }
}

/// Renders either the whole main html, or returns just the content suitable for swapping into the main element.
pub(crate) fn render_body_html_or_htmx(
    code: StatusCode,
    title: impl AsRef<str>,
    inner: Markup,
    htmx_context: Option<HtmxContext>,
) -> Response {
    let mut hm = HeaderMap::new();
    hm.insert("Content-Type", HeaderValue::from_static("text/html"));
    hm.insert("Vary", HeaderValue::from_static("HX-Request"));
    if let Some(hc) = htmx_context {
        // Ensure that we retarget the request if it's attempting to swap to the wrong place.
        if hc.target.is_some_and(|x| x.ne("#body")) {
            hm.insert("HX-Retarget", HeaderValue::from_static("#body"));
            hm.insert("HX-Reswap", HeaderValue::from_static("innerHTML"));
        }
        // HTMX requires HTTP 200 responses by default.
        (
            StatusCode::OK,
            hm,
            html! {
                title { (title.as_ref()) }
                (inner)
            }
            .0,
        )
            .into_response()
    } else {
        (code, hm, render_body_html(title, inner).0).into_response()
    }
}

pub(crate) fn internal_error_page(err: anyhow::Error, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal Error",
        render_body_semantics(
            "Internal Error",
            vec![html! {
                p {
                    "An internal error has occurred. Please navigate back using the links above."
                }
                code {
                    @for err in err.chain() {
                        (err)
                        br;
                    }
                }
            }],
        ),
        htmx_context,
    )
}

pub(crate) fn not_found_page(method: Method, uri: Uri, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(
        StatusCode::NOT_FOUND,
        "Not Found",
        render_body_semantics(
            "Not Found",
            vec![html! {
                p {
                    code { (method.as_str()) }
                    " "
                    code { (uri.path()) }
                    " not found"
                }
            }],
        ),
        htmx_context,
    )
}

pub(crate) fn list_posts_page(posts: Vec<Post>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(
        StatusCode::OK,
        "Posts",
        render_body_semantics(
            "Posts",
            vec![html! {

                a href="/posts/new" class="button" {
                    "New Post"
                }

                table {
                    thead {
                        tr {
                            th { "Date" }
                            th { "Slug" }
                            th { "Title" }
                            th { "Published" }
                            th { "Labels" }
                        }
                    }
                    tbody {
                        @if posts.is_empty() {
                            tr {
                                td colspan="5" { "No posts, please create one" }
                            }
                        } @else {
                            @for post in posts {
                                tr {
                                    td {
                                        a href={"/posts/" (post.slug)} {
                                            (post.date)
                                        }
                                    }
                                    td { (post.slug) }
                                    td { (post.title) }
                                    td {
                                        @if post.published { "Yes" } @else { strong { "No" } }
                                    }
                                    td { (post.labels.join(", ")) }
                                }
                            }
                        }
                    }
                }
            }],
        ),
        htmx_context,
    )
}

fn render_post_form(current: Option<(&Post, &str)>, is_new: bool) -> Markup {
    html! {
        div.row {
            div.column {
                label for="slug" { "URL Slug (immutable)" }
                @if is_new {
                    input type="text" name="slug" spellcheck="true" required="true" placeholder="the-url-slug-of-this-post" value=[current.as_ref().map(|x| &x.0.slug)];
                } @else {
                    input type="text" disabled? value=[current.as_ref().map(|x| &x.0.slug)];
                }
            }
            div.column {
                label for="title" { "Post Title" }
                input type="text" name="title" spellcheck="true" required="true" placeholder="The title of this post" value=[current.as_ref().map(|x| &x.0.title)];
            }
            div.column {
                label for="published" { "Published" }
                input type="checkbox" name="published" value="true" checked[current.as_ref().map(|x| x.0.published).unwrap_or_default()];
            }
        }
        div.row {
            div.column {
                label for="date" { "Post Date" }
                input type="date" name="date" required="true" value=(current.as_ref().map(|x| &x.0.date).unwrap_or(&Local::now().date_naive()));
            }
            div.column {
                label for="labels" { "Labels" }
                input type="text" name="labels" placeholder="label,label,label" value=[current.as_ref().map(|x| x.0.labels.join(","))];
            }
        }
        div.row {
            div.column {
                label for="raw_content" { "Raw Content" }
                textarea name="raw_content" spellcheck="true" wrap="soft" placeholder="Your post content here.." {
                    @if let Some((_, c)) = current.as_ref() {
                        (c)
                    }
                }
                button type="submit" { "Submit" }
                a.button.button-clear href="/posts" { "Cancel" }
                @if let Some((c, _)) = current.as_ref() {
                    @if !is_new {
                        form action={"/posts/" (c.slug)} hx-confirm="Are you sure you want to delete this post?" method="delete" style="display: inline" hx-disabled-elt="find input, find button, find textarea" {
                            button.button-clear type="submit" { "Delete" }
                        }
                    }
                }
                details {
                    summary { "Markdown Hints" }
                    small { pre { r#"**bold** _italic_ ~strike~ ![alt](/link)

title 1
: definition 1

Footnote referenced [^1].

| Col  | Col  |
| ---- | ---- |
| Cell | Cell |

[^1]: footnote defined"# } }
                }
            }
        }
    }
}

pub(crate) fn new_posts_page(post: Option<(&Post, &str)>, error: Option<String>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(
        StatusCode::OK,
        "New post",
        render_body_semantics(
            "New Post",
            vec![html! {
                @if let Some(e) = error {
                    div {
                        (e)
                    }
                }
                form action="/posts/new" method="post" {
                    (render_post_form(post, true))
                }
            }],
        ),
        htmx_context,
    )
}

pub(crate) fn edit_posts_page(
    post: Post,
    content: String,
    html_content: Markup,
    toc_content: Markup,
    error: Option<String>,
    htmx_context: Option<HtmxContext>,
) -> Response {
    render_body_html_or_htmx(
        StatusCode::OK,
        "Edit post",
        render_body_semantics(
            "Edit Post",
            vec![html! {
                @if let Some(e) = error {
                    div {
                        (e)
                    }
                }
                form action={ "/posts/" (post.slug) } method="post" {
                    (render_post_form(Some((&post, content.as_ref())), false))
                }
                hr;
                hr;
                article hx-boost="false" {
                    h1 { (post.title) }
                    nav.toc { ul { (toc_content) } }
                    (html_content)
                }
            }],
        ),
        htmx_context,
    )
}

pub(crate) fn debug_objects_page(objects: Vec<ObjectMeta>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(
        StatusCode::OK,
        "Debug",
        render_body_semantics(
            "Debug",
            vec![html! {
                table {
                    thead {
                        tr {
                            th { "Location" }
                            th { "Size" }
                            th { "Last Modified" }
                        }
                    }
                    tbody {
                        @if objects.is_empty() {
                            tr {
                                td colspan="3" { "No objects" }
                            }
                        } @else {
                            @for object in objects {
                                tr {
                                    td { (object.location) }
                                    td { (object.size) }
                                    td { (object.last_modified) }
                                }
                            }
                        }
                    }
                }
            }],
        ),
        htmx_context,
    )
}

pub(crate) fn list_images_page(images: Vec<Image>, error: Option<Error>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(
        StatusCode::OK,
        "Images",
        render_body_semantics(
            "Images",
            vec![html! {
                @if let Some(e) = error {
                    div {
                        code {
                            @for err in e.chain() {
                                (err)
                                br;
                            }
                        }
                    }
                }
                form action="/images" method="post" enctype="multipart/form-data" hx-disabled-elt="find input[type='text'], find button" {
                    div.row {
                        div.column {
                            label for="slug" { "URL Slug" }
                            input type="text" name="slug" spellcheck="true" required="true" placeholder="the-url-slug-of-this-image";
                        }
                        div.column {
                            label for="images" { "Image" }
                            input type="file" name="image" required="true";
                        }
                        div.column {
                            button type="submit" { "Submit" }
                        }
                    }
                }
                table {
                    thead {
                        tr {
                            th { "Image" }
                            th { "Link" }
                            th { "Actions" }
                        }
                    }
                    tbody {
                        @if images.is_empty() {
                            tr {
                                td colspan="3" { "No images" }
                            }
                        } @else {
                            @for img in images {
                                tr {
                                    td {
                                        a href={ "/images/" (img.to_original().to_path_part().as_ref()) } {
                                            img src={ "/images/" (img.to_thumbnail().to_path_part().as_ref()) };
                                        }
                                    }
                                    td {
                                        code style="user-select: all" {
                                            "[![missing alt text](/images/" (img.to_medium().to_path_part().as_ref()) ")](/images/" (img.to_original().to_path_part().as_ref()) ")"
                                        }
                                    }
                                    td {
                                        form action={"/images/" (img.to_original().to_path_part().as_ref()) } hx-confirm="Are you sure you want to delete this image?" method="delete" hx-disabled-elt="find input[type='text'], find button" {
                                            button.button.button-clear type="submit" { "Delete" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }],
        ),
        htmx_context,
    )
}

pub(crate) fn get_image_page(image: impl AsRef<Image>, htmx_context: Option<HtmxContext>) -> Response {
    let original_path = image.as_ref().to_path_part();
    render_body_html_or_htmx(
        StatusCode::OK,
        "Image",
        render_body_semantics(
            "Image",
            vec![html! {
                img src={ "/images/" (original_path.as_ref()) };
                form action={"/images/" (original_path.as_ref()) } hx-confirm="Are you sure you want to delete this image?" method="delete" hx-disabled-elt="find input[type='text'], find button" {
                    button.button type="submit" { "Delete" }
                }
            }],
        ),
        htmx_context,
    )
}
