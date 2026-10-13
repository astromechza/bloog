use crate::htmx::HtmxContext;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use maud::{html, Markup};

/// Renders either the whole main html, or returns just the content suitable for swapping into the main element.
pub(crate) fn render_body_html_or_htmx(
    code: StatusCode,
    title: impl AsRef<str>,
    inner: Markup,
    outer: fn(&str, Markup) -> Markup,
    htmx_context: Option<Box<HtmxContext>>,
) -> impl IntoResponse {
    let mut hm = HeaderMap::new();
    hm.insert("Content-Type", HeaderValue::from_static("text/html"));
    hm.insert("Vary", HeaderValue::from_static("HX-Request"));
    hm.insert(
        "Cache-Control",
        HeaderValue::from_static(match code {
            StatusCode::OK => "public, max-age=300, stale-while-revalidate=30",
            _ => "no-cache",
        }),
    );
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
    } else {
        (code, hm, outer(title.as_ref(), inner).0)
    }
}

pub(crate) const FONTS_CSS_URL: &str = "https://fonts.googleapis.com/css2?family=Atkinson+Hyperlegible+Mono:wght@400;700&family=Atkinson+Hyperlegible+Next:ital,wght@0,400;0,700;1,400;1,700&display=swap";

pub(crate) const COMMON_CSS: &str = r###"
:root {
--bg: #FFFCF0;
--tx: #343331;
--tx-muted: #6F6E69;
--link: #205EA6;
--border: #DAD8CE;
--border-input: #878580;
--code-tx: #AF3029;
--code-bg: #F2F0E5;
--code-block-bg: #282C34;
--main-font: 'Atkinson Hyperlegible Next', system-ui, sans-serif;
--mono-font: 'Atkinson Hyperlegible Mono', ui-monospace, monospace;
}

html, body { height: 100% }
body {
  display: flex;
  flex-direction: column;
  background-color: var(--bg);
  color: var(--tx);
  font-family: var(--main-font);
  font-size: 1.8rem;
  font-weight: 400;
  letter-spacing: normal;
  line-height: 1.7;
}
.container { max-width: 78rem; }

h1, h2, h3, h4, h5, h6 {
  color: var(--tx);
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.25;
}
header h1 { font-size: 3.6rem; }
article h1 { font-size: 3.2rem; }
article h2 { font-size: 2.7rem; }
article h3 { font-size: 2.2rem; }
article h4 { font-size: 1.9rem; }
article h5 { font-size: 1.8rem; }
small, .muted { color: var(--tx-muted); }

a {
  color: var(--link);
  text-decoration-line: underline;
  text-decoration-thickness: 0.06em;
  text-underline-offset: 0.18em;
}
a:hover, a:focus { color: var(--link); text-decoration-thickness: 0.12em; }
h1 a, nav a, a.button { text-decoration-line: none; }
h1 a:hover, nav a:hover { text-decoration-line: underline; }
article a[href^="http"]::after {
  content: "";
  display: inline-block;
  width: 0.6em;
  height: 0.6em;
  margin-bottom: 0.1em;
  margin-left: 0.25em;
  margin-right: 0.1em;
  background-size: 100%;
  background-image: url("/statics/link.svg");
}
article a.hlink { color: var(--tx); text-decoration-line: none; }
article a.hlink:hover { text-decoration-line: underline; }

code, pre, kbd, textarea { font-family: var(--mono-font); }
code {
  color: var(--code-tx);
  background: var(--code-bg);
  border: 1px solid var(--border);
  border-radius: 0.3em;
  font-size: 0.9em;
  padding: 0.1em 0.35em;
}
pre {
  background: var(--code-block-bg);
  border: 0;
  border-radius: 0.5em;
  overflow-x: auto;
}
pre > code {
  display: block;
  white-space: pre-wrap;
  color: #ABB2BF;
  background: transparent;
  border: 0;
  border-radius: 0;
  font-size: 0.85em;
  padding: 1.2em 1.5em;
}

blockquote { border-left-color: var(--border); color: var(--tx-muted); }
td, th { border-bottom-color: var(--border); }
input[type="text"], input[type="date"], input[type="file"], textarea, select { border-color: var(--border-input); }
input:focus, textarea:focus, select:focus { border-color: var(--link) !important; }
.button, button, input[type="submit"], input[type="button"] {
  background-color: var(--link);
  border-color: var(--link);
  font-family: var(--main-font);
  letter-spacing: 0.05em;
}
.button.button-outline, button.button-outline { background-color: transparent; color: var(--link); }
.button.button-clear, button.button-clear { background-color: transparent; border-color: transparent; color: var(--link); }

ul { list-style: circle outside; }
ul li { margin-left: 1em; }
.footnote-definition { margin-bottom: 2em; color: var(--tx-muted); font-size: 0.9em; }
.footnote-definition p { display: inline; }

article img:not([src$=".svg"]) { border-radius: 0.3em; }
article > ol > li > p { display: inline; }
article > ul > li > p { display: inline; }

nav.toc ul { font-size: 1.6rem; list-style: none; }
nav.toc .toc-l1 { margin-left: 0; }
nav.toc .toc-l2 { margin-left: 2rem; }
nav.toc .toc-l3 { margin-left: 4rem; }
nav.toc .toc-l4 { margin-left: 6rem; }
nav.toc .toc-l5 { margin-left: 8rem; }
"###;
