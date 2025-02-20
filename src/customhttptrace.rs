use axum::response::Response;
use http::Request;
use tower_http::trace::MakeSpan;
use tracing::Span;

#[derive(Default, Clone)]
pub struct HttpTraceLayerHooks;

impl<B> MakeSpan<B> for HttpTraceLayerHooks {
    fn make_span(&mut self, req: &Request<B>) -> Span {
        // see https://github.com/open-telemetry/semantic-conventions/blob/main/docs/http/http-spans.md
        let span = tracing::info_span!(
            // span name here will be ignored by open telemetry and replaced with otel.name
            "request",
            // the span name
            otel.name = tracing::field::Empty,
            otel.kind = "server",
            otel.status_code = tracing::field::Empty,
            // common attributes
            http.response.status_code = tracing::field::Empty,
            http.request.body.size = tracing::field::Empty,
            http.response.body.size = tracing::field::Empty,
            http.request.method = req.method().as_str(),
            network.protocol.name = "http",
            network.protocol.version = format!("{:?}", req.version()).strip_prefix("HTTP/"),
            user_agent.original = tracing::field::Empty,
            // http request and response headers
            http.request.header.x_forwarded_for = tracing::field::Empty,
            http.request.header.cf_ipcountry = tracing::field::Empty,
            http.request.header.referer = tracing::field::Empty,
            // http server
            http.route = tracing::field::Empty,
            server.address = tracing::field::Empty,
            server.port = tracing::field::Empty,
            url.path = req.uri().path(),
            url.query = tracing::field::Empty,
            url.scheme = tracing::field::Empty,
            // set on the failure hook
            "error.type" = tracing::field::Empty,
            error = tracing::field::Empty,
        );

        req.uri().query().map(|v| span.record("url.query", v));
        req.uri().scheme().map(|v| span.record("url.scheme", v.as_str()));
        req.uri().host().map(|v| span.record("server.address", v));
        req.uri().port_u16().map(|v| span.record("server.port", v));

        req.headers()
            .get(http::header::CONTENT_LENGTH)
            .map(|v| v.to_str().map(|v| span.record("http.request.body.size", v)));
        req.headers()
            .get(http::header::USER_AGENT)
            .map(|v| v.to_str().map(|v| span.record("user_agent.original", v)));
        req.headers()
            .get("X-Forwarded-For")
            .map(|v| v.to_str().map(|v| span.record("http.request.header.x_forwarded_for", v)));
        req.headers()
            .get("CF-IPCountry")
            .map(|v| v.to_str().map(|v| span.record("http.request.header.cf_ipcountry", v)));
        req.headers()
            .get("Referer")
            .map(|v| v.to_str().map(|v| span.record("http.request.header.referer", v)));

        if let Some(path) = req.extensions().get::<axum::extract::MatchedPath>() {
            span.record("otel.name", format!("{} {}", req.method(), path.as_str()));
            span.record("http.route", path.as_str());
        } else {
            span.record("otel.name", format!("{} -", req.method()));
        };

        span
    }
}

impl<B> tower_http::trace::OnResponse<B> for HttpTraceLayerHooks {
    fn on_response(self, response: &Response<B>, _: std::time::Duration, span: &Span) {
        if let Some(size) = response.headers().get(http::header::CONTENT_LENGTH) {
            span.record("http.response.body.size", size.to_str().unwrap_or_default());
        }
        span.record("http.response.status_code", response.status().as_u16());

        // Server errors are handled by the OnFailure hook
        if !response.status().is_server_error() {
            tracing::event!(tracing::Level::INFO, "finished processing request");
        }
    }
}

impl<FailureClass> tower_http::trace::OnFailure<FailureClass> for HttpTraceLayerHooks
where
    FailureClass: std::fmt::Display,
{
    fn on_failure(&mut self, error: FailureClass, _: std::time::Duration, _: &Span) {
        tracing::event!(
            tracing::Level::ERROR,
            error = error.to_string(),
            "error.type" = "_OTHER",
            "response failed"
        );
    }
}
impl<B> tower_http::trace::OnRequest<B> for HttpTraceLayerHooks {
    fn on_request(&mut self, _: &Request<B>, _: &Span) {
        tracing::event!(tracing::Level::DEBUG, "start processing request");
    }
}
