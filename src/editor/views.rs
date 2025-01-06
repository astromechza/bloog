use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use maud::{html, Markup, DOCTYPE};
use crate::htmx::HtmxContext;
use crate::store::Post;
use crate::store::PostContentType::{Markdown, RestructuredText};

fn render_body_html(title: impl AsRef<str>, inner: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                title { (title.as_ref()) }
                style {
                    r#"
                        /**
                         * Minified by jsDelivr using clean-css v5.3.2.
                         * Original file: /npm/modern-normalize@3.0.1/modern-normalize.css
                         *
                         * Do NOT use SRI with dynamically generated files! More information: https://www.jsdelivr.com/using-sri-with-dynamic-files
                         */
                        /*! modern-normalize v3.0.1 | MIT License | https://github.com/sindresorhus/modern-normalize */
                        *,::after,::before{box-sizing:border-box}html{font-family:system-ui,'Segoe UI',Roboto,Helvetica,Arial,sans-serif,'Apple Color Emoji','Segoe UI Emoji';line-height:1.15;-webkit-text-size-adjust:100%;tab-size:4}body{margin:0}b,strong{font-weight:bolder}code,kbd,pre,samp{font-family:ui-monospace,SFMono-Regular,Consolas,'Liberation Mono',Menlo,monospace;font-size:1em}small{font-size:80%}sub,sup{font-size:75%;line-height:0;position:relative;vertical-align:baseline}sub{bottom:-.25em}sup{top:-.5em}table{border-color:currentcolor}button,input,optgroup,select,textarea{font-family:inherit;font-size:100%;line-height:1.15;margin:0}[type=button],[type=reset],[type=submit],button{-webkit-appearance:button}legend{padding:0}progress{vertical-align:baseline}::-webkit-inner-spin-button,::-webkit-outer-spin-button{height:auto}[type=search]{-webkit-appearance:textfield;outline-offset:-2px}::-webkit-search-decoration{-webkit-appearance:none}::-webkit-file-upload-button{-webkit-appearance:button;font:inherit}summary{display:list-item}
                        /*# sourceMappingURL=/sm/d2d8cd206fb9f42f071e97460f3ad9c875edb5e7a4b10f900a83cdf8401c53a9.map */
                    "#
                }
                link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/milligram/1.4.1/milligram.min.css" integrity="sha512-xiunq9hpKsIcz42zt0o2vCo34xV0j6Ny8hgEylN3XBglZDtTZ2nwnqF/Z/TTCc18sGdvCjbFInNd++6q3J0N6g==" crossorigin="anonymous" referrerpolicy="no-referrer";
                link rel="shortcut icon" type="image/svg" href="/favicon.svg";
                script src="https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js" integrity="sha512-2kIcAizYXhIn8TzUvqzEDZNuDZ+aW7yE/+f1HJHXFjQcGNfv1kqzJSTBRBSlOgp6B/KZsz1K0a3ZTqP9dnxioQ==" crossorigin="anonymous" referrerpolicy="no-referrer" {};
            }
            body hx-boost="true" id="body" {
                (inner)
            }
        }
    }
}

fn render_body_semantics(header: &str, sections: Vec<Markup>) -> Markup {
    html! {
        main class="container" {
            header {
                nav {
                    a href="/" { "home "}
                    " | "
                    a href="/posts" { "posts" }
                    " | "
                    a href="/images" { "images" }
                }
                h2 { (header) }
            }
            @for section in sections {
                section { (section) }
            }
        }
    }
}

/// Renders either the whole main html, or returns just the content suitable for swapping into the main element.
pub(crate) fn render_body_html_or_htmx(code: StatusCode, title: impl AsRef<str>, inner: Markup, htmx_context: Option<HtmxContext>) -> Response {
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
        (StatusCode::OK, hm, html! {
            title { (title.as_ref()) }
            (inner)
        }.0).into_response()
    } else {
        (code, hm, render_body_html(title, inner).0).into_response()
    }
}

pub(crate) fn internal_error_page(err: anyhow::Error, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(StatusCode::INTERNAL_SERVER_ERROR, "Internal Error", render_body_semantics("Internal Error", vec![html! {
        p {
            "An internal error has occurred. Please navigate back using the links above."
        }
        code {
            (err)
        }
    }]), htmx_context)
}

pub(crate) fn not_found_page(method: Method, uri: Uri, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(StatusCode::NOT_FOUND, "Not Found", render_body_semantics("Not Found", vec![html! {
        p {
            code { (method.as_str()) }
            " "
            code { (uri.path()) }
            " not found"
        }
    }]), htmx_context)
}

pub(crate) fn list_posts_page(posts: Vec<Post>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(StatusCode::OK, "Posts", render_body_semantics("Posts", vec![html! {

        a href="/posts/new" class="button" {
            "New Post"
        }

        table {
            thead {
                tr {
                    th { "Date" }
                    th { "Slug" }
                    th { "Title" }
                    th { "Labels" }
                }
            }
            tbody {
                @if posts.is_empty() {
                    tr {
                        td colspan="4" { "No posts, please create one" }
                    }
                } else {
                    @for post in posts {
                        tr {
                            td {
                                a href={"/posts/" (post.slug)} {
                                    (post.date)
                                }
                            }
                            td { (post.slug) }
                            td { (post.title) }
                            td { (post.labels.join(", ")) }
                        }
                    }
                }
            }
        }
    }]), htmx_context)
}

fn render_post_form(current: Option<(Post, String)>, is_new: bool) -> Markup {
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
        }
        div.row {
            div.column {
                label for="date" { "Post Date" }
                input type="date" name="date" required="true" value=[current.as_ref().map(|x| &x.0.date)];
            }
            div.column {
                label for="content_type" { "Content Type" }
                select name="content_type" required="true" {
                    option value=(Markdown) selected[current.as_ref().map(|x| x.0.content_type == Markdown).unwrap_or_default()] { (Markdown) }
                    option value=(RestructuredText) selected[current.as_ref().map(|x| x.0.content_type == RestructuredText).unwrap_or_default()] { (RestructuredText)}
                }
            }
        }
        div.row {
            div.column {
                label for="raw_content" { "Raw Content" }
                textarea name="raw_content" spellcheck="true" wrap="soft" placeholder="Your post content here.." {
                    @if let Some((_, c)) = current {
                        (c)
                    }
                }
                button type="submit" { "Submit" }
            }
        }
    }
}

pub(crate) fn new_posts_page(post: Option<(Post, String)>, error: Option<String>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(StatusCode::OK, "New post", render_body_semantics("New Post", vec![html! {
        @if let Some(e) = error {
            div {
                (e)
            }
        }
        form action="/posts/new" method="post" {
            (render_post_form(post, true))
        }
    }]), htmx_context)
}

pub(crate) fn edit_posts_page(post: Post, content: String, error: Option<String>, htmx_context: Option<HtmxContext>) -> Response {
    render_body_html_or_htmx(StatusCode::OK, "Edit post", render_body_semantics("Edit Post", vec![html! {
        @if let Some(e) = error {
            div {
                (e)
            }
        }
        form action={ "/posts/" (post.slug) } method="post" {
            (render_post_form(Some((post, content)), true))
        }
    }]), htmx_context)
}
