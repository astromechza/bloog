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
    htmx_context: Option<HtmxContext>,
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

pub(crate) const COMMON_CSS: &str = r###"
:root {
--main-bg-colour: rgb(255, 252, 240);
--main-tx-colour: rgb(16, 15, 15);
--main-a-colour: rgb(67, 133, 190);
--main-font: 'Verdana', sans-serif;
}

html, body { height: 100% }
body { display: flex; flex-direction: column; font-family: var(--main-font); }
pre code { display: block; white-space: pre-wrap; }
ul { list-style: circle outside; }
ul li { margin-left: 1em; }
body { background-color: var(--main-bg-colour); }
.footnote-definition { margin-bottom: 2em; }
.footnote-definition p { display: inline; }
.container {
  color: var(--main-tx-colour);
  font-family: var(--main-font);
  font-size: 1em;
  font-weight: 300;
  letter-spacing: .01em;
  line-height: 1.6;
}

header h1 { font-size: 3.6rem; }

a {
  color: var(--main-a-colour);
}
article a {
  text-decoration-line: underline;
  text-decoration-style: dotted;
}
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
article a.hlink { color: var(--main-tx-colour); text-decoration: none; }
article a.hlink:hover { text-decoration-line: underline; text-decoration-style: dotted; }

article img:not([src$=".svg"]) {
  border-radius: 0.3em;
}
article h1 { font-size: 3.2rem; }
article h2 { font-size: 2.7rem; }
article h3 { font-size: 2.2rem; }
article h4 { font-size: 1.8rem; }
article h5 { font-size: 1.6rem; }

nav.toc ul { font-size: 1.4rem; list-style: none; }
nav.toc .toc-l1 { margin-left: 0; }
nav.toc .toc-l2 { margin-left: 2rem; }
nav.toc .toc-l3 { margin-left: 4rem; }
nav.toc .toc-l4 { margin-left: 6rem; }
nav.toc .toc-l5 { margin-left: 8rem; }
"###;
