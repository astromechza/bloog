use crate::htmx::HtmxContext;
use crate::store::Post;
use crate::viewhelpers::{render_body_html_or_htmx, COMMON_CSS};
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use chrono::{Datelike, Local};
use clap::crate_version;
use lazy_static::lazy_static;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use std::ops::Deref;

const RFC3339_DATE_FORMAT: &str = "%Y-%m-%dT00:00:00Z";

fn render_body_html(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                title { (title) }
                meta charset="utf-8";
                meta name="author" content="Ben Meier";
                meta name="keywords" content="golang, rust, distributed systems, programming, security";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                link rel="shortcut icon" href="/statics/favicon.svg" type="image/svg+xml";
                link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/modern-normalize/3.0.1/modern-normalize.min.css" integrity="sha512-q6WgHqiHlKyOqslT/lgBgodhd03Wp4BEqKeW6nNtlOY4quzyG3VoQKFrieaCeSnuVseNKRGpGeDU3qPmabCANg==" crossorigin="anonymous" referrerpolicy="no-referrer";
                link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/milligram/1.4.1/milligram.min.css" integrity="sha512-xiunq9hpKsIcz42zt0o2vCo34xV0j6Ny8hgEylN3XBglZDtTZ2nwnqF/Z/TTCc18sGdvCjbFInNd++6q3J0N6g==" crossorigin="anonymous" referrerpolicy="no-referrer";
                link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/default.min.css" crossorigin="anonymous" referrerpolicy="no-referrer";
                style nonce="123456789" {
                    (PreEscaped(COMMON_CSS))
                    (PreEscaped(r#"
                    .index-nav-ul { margin: 0; list-style: circle outside; }
                    header.row { justify-content: space-between; align-items: center; }
                    header.row .column { max-width: fit-content; }
                    header.row nav.column { margin: 0; }
                    header img {
                      height: 1.3em;
                      vertical-align: middle;
                      margin-right: 0.5em;
                    }
                    .block { display: block; }
                    .m-b-05 { margin-bottom: 0.5em; }
                    .m-b-1 { margin-bottom: 1em; }
                    main.container {
                      margin: 2em auto 0;
                      flex-grow: 1;
                    }
                    footer.container {
                      margin: 2em auto;
                      text-align: center;
                      padding-bottom: 1em;
                    }
                    hr {
                        border: 0;
                        border-top: 0.1rem dotted var(--main-tx-colour);
                        margin: 3.0rem 0;
                    }
                    "#))
                }
                script src="https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js" integrity="sha512-2kIcAizYXhIn8TzUvqzEDZNuDZ+aW7yE/+f1HJHXFjQcGNfv1kqzJSTBRBSlOgp6B/KZsz1K0a3ZTqP9dnxioQ==" crossorigin="anonymous" referrerpolicy="no-referrer" {};
                script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js" crossorigin="anonymous" referrerpolicy="no-referrer" {};
            }
            body hx-boost="true" id="body" {
                (body)
            }
        }
    }
}

lazy_static! {
    static ref FOOTER: Markup = html! {
        footer.container {
            small {
                "© Ben Meier " (Local::now().year()) " - "
                a target="_blank" href={"https://github.com/astromechza/bloog/releases/tag/" (crate_version!()) } { "astromechza/bloog@" (crate_version!()) }
            }
        }
    };
}

pub(crate) fn internal_error_page(err: anyhow::Error, htmx_context: Option<HtmxContext>) -> impl IntoResponse {
    render_body_html_or_htmx(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Internal Error",
        html! {
            main.container {
                header.row.m-b-05 {
                    h1.column {
                        a href="/" title="Back to index" {
                            "/ "
                        }
                        "Error"
                    }
                }
                section {
                    details {
                        summary {
                            p {
                                "An internal error has occurred. Go back to the "
                                a href="/" {
                                    "index"
                                }
                                "."
                            }
                        }
                        code {
                            @for err in err.chain() {
                                (err)
                                br;
                            }
                        }
                    }
                }
            }
            (FOOTER.deref())
        },
        render_body_html,
        htmx_context,
    )
}

pub(crate) fn not_found_page(uri: Uri, htmx_context: Option<HtmxContext>) -> impl IntoResponse {
    render_body_html_or_htmx(
        StatusCode::NOT_FOUND,
        "Not Found",
        html! {
            main.container {
                header.row.m-b-05 {
                    h1.column {
                        a href="/" title="Back to index" {
                            "/ "
                        }
                        "Not Found"
                    }
                }
                section {
                    p {
                        "Page " (uri) " not found. Go back to the "
                        a href="/" {
                            "index"
                        }
                        "."
                    }
                }
            }
            (FOOTER.deref())
        },
        render_body_html,
        htmx_context,
    )
}

pub(crate) fn get_index_page(
    label_filter: Option<String>,
    year_groups: Vec<(&i32, &Vec<&Post>)>,
    htmx_context: Option<HtmxContext>,
) -> impl IntoResponse {
    render_body_html_or_htmx(
        StatusCode::OK,
        "Ben's Blog",
        html! {
            main.container {
                header.row.m-b-05 {
                    h1.column style="max-width: none" {
                        a href="/" title="Back to index" {
                            "/ "
                        }
                        "Ben's Blog"
                    }
                    div.column style="flex: 0 0 auto" {
                        img src="/statics/bluesky.svg" alt="Bluesky logo";
                        a href="https://bsky.app/profile/ben.bsky.meierhost.com" target="_blank" {
                            "@ben.bsky.meierhost.com"
                        }
                    }
                    div.column style="flex: 0 0 auto" {
                        img src="/statics/github.svg" alt="Github logo";
                        a href="https://github.com/astromechza" target="_blank" {
                            "github/astromechza"
                        }
                    }
                }
                section {
                    p.block style="font-size: smaller" {
                        r#"
                        I'm a software engineer working mostly on distributed systems with an interest in security, networking, correctness, and chaos.
                        All opinions expressed here are my own.
                        This blog contains a wide range of content accrued over time and from multiple previous attempts at technical blogging over the course of my career.
                        I intentionally don't go back and improve or rewrite old posts, so please take old content with a pinch of salt, and I apologise for any broken links!
                        "#
                    }
                    hr;
                    @if let Some(l) = label_filter {
                        p {
                            "(Showing posts labeled '" (l) "'. "
                            a href="/" title="Back to index" {
                                "Click here to go back to all posts."
                            }
                            ")"
                        }
                    }
                    nav {
                        @for (y, g) in year_groups {
                            h3 { (y) }
                            ul.index-nav-ul {
                                @for p in g {
                                    li {
                                        a href={ "/posts/" (&p.slug) } {
                                            time datetime=(&p.date.format(RFC3339_DATE_FORMAT).to_string()) {
                                                (&p.date.format("%d %b").to_string())
                                            }
                                            ": "
                                            (&p.title)
                                        }
                                        @if !p.labels.is_empty() {
                                            small {
                                                " ("
                                                @for (i, l) in p.labels.iter().enumerate() {
                                                    @if i > 0 {
                                                        " | "
                                                    }
                                                    a href={"/?label=" (l)} { "#" (l) }
                                                }
                                                ")"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (FOOTER.deref())
        },
        render_body_html,
        htmx_context,
    ).into_response()
}

pub(crate) fn get_post_page(post: Post, content_html: Markup, toc: Markup, htmx_context: Option<HtmxContext>) -> impl IntoResponse {
    render_body_html_or_htmx(
        StatusCode::OK,
        post.title.as_str(),
        html! {
            main.container {
                header.row.m-b-05 {
                    h1.column {
                        a href="/" title="Back to index" {
                            "/ "
                        }
                        (post.title)
                    }
                }
                section {
                    p.block.m-b-1 {
                        time datetime=(post.date.format(RFC3339_DATE_FORMAT).to_string()) { (post.date.format("%e %B %Y").to_string()) }
                        @if !post.labels.is_empty() {
                            @for l in post.labels {
                                " | "
                                a href={"/?label=" (l)} title={"Filter by label " (l) } { "#" (l) }
                            }
                        }
                    }
                    hr;
                    article {
                        nav.toc { ul { (toc) } }
                        (content_html)
                    }
                    script {
                        (PreEscaped(r"hljs.highlightAll();"))
                    }
                }
            }
            (FOOTER.deref())
        },
        render_body_html,
        htmx_context,
    )
    .into_response()
}
