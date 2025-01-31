use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse};
use maud::{html, Markup};
use crate::htmx::HtmxContext;


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