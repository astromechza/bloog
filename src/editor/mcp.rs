mod render;

use crate::store::{Post, Store};
use regex::RegexBuilder;
use render::{OUTPUT_BUDGET, RenderOptions, render};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use std::sync::Arc;
use tracing::instrument;

const DEFAULT_LIMIT: usize = 400;
const MAX_LIMIT: usize = 1000;
const DEFAULT_CONTEXT: usize = 2;
const MAX_CONTEXT: usize = 10;
/// Compiled size limit for agent supplied patterns; far above any sensible grep, well below the regex default.
const MAX_PATTERN_SIZE: usize = 1 << 20;

const INSTRUCTIONS: &str = "Read-only access to the author's blog posts in the bloog editor. \
You cannot change anything: propose edits for the author to apply by hand in the editor. \
The editor has no line numbers and soft-wraps paragraphs, so every suggestion must quote a short, unique snippet of \
the existing text together with its line number, then give the replacement. \
Line numbers shift after the author saves an edit; re-fetch with get_post before giving further line-based \
suggestions. Only saved content is visible.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetPostRequest {
    #[schemars(description = "The post slug, as returned by list_posts.")]
    pub slug: String,
    #[schemars(description = "1-based first line to scan. Default 1.")]
    pub offset: Option<usize>,
    #[schemars(description = "Number of lines to scan from offset (1-1000). Default 400 without pattern; the whole post with pattern.")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Optional regex (Rust regex syntax) matched against each line separately; patterns cannot span lines. \
Use (?i) for case-insensitive matching. '^#' lists headings with their line numbers (it also matches '#' lines inside code fences)."
    )]
    pub pattern: Option<String>,
    #[schemars(description = "Lines of context either side of each match (0-10). Default 2. Only used with pattern.")]
    pub context: Option<usize>,
}

#[derive(Clone)]
pub struct EditorMcp {
    store: Arc<Store>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl EditorMcp {
    #[tool(description = "List all posts (drafts and published), newest first, as JSON: slug, title, date, published, labels.")]
    async fn list_posts(&self) -> Result<CallToolResult, ErrorData> {
        let mut posts = self.store.list_posts().await.map_err(internal)?;
        posts.sort();
        posts.reverse();
        let json = serde_json::to_string(&posts).map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "Read a post's markdown as numbered lines ('<line>\\t<text>'), preceded by a metadata header and a '---' line. \
Large posts are paged: when the header has next_offset, call again with offset set to it. \
With pattern, only matching lines plus context are returned, like grep -C, with '--' between non-adjacent groups."
    )]
    async fn get_post(&self, Parameters(req): Parameters<GetPostRequest>) -> Result<CallToolResult, ErrorData> {
        self.get_post_inner(req).await
    }
}

impl EditorMcp {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            tool_router: Self::tool_router(),
        }
    }

    #[instrument(name = "mcp.get_post", skip_all, fields(slug = %req.slug, pattern = ?req.pattern))]
    async fn get_post_inner(&self, req: GetPostRequest) -> Result<CallToolResult, ErrorData> {
        let pattern = match req
            .pattern
            .as_deref()
            .map(|p| RegexBuilder::new(p).size_limit(MAX_PATTERN_SIZE).build())
            .transpose()
        {
            Ok(p) => p,
            Err(e) => return Ok(tool_error(format!("invalid pattern: {e}"))),
        };
        let Some((post, content)) = self.store.get_post_raw(&req.slug).await.map_err(internal)? else {
            return Ok(tool_error(format!("post '{}' not found", req.slug)));
        };
        let limit = match (req.limit, &pattern) {
            (Some(l), _) => Some(l.clamp(1, MAX_LIMIT)),
            (None, None) => Some(DEFAULT_LIMIT),
            (None, Some(_)) => None,
        };
        let rendered = render(
            &content,
            &RenderOptions {
                offset: req.offset.unwrap_or(1).max(1),
                limit,
                pattern: pattern.as_ref(),
                context: req.context.unwrap_or(DEFAULT_CONTEXT).min(MAX_CONTEXT),
                budget: OUTPUT_BUDGET,
            },
        );
        let mut out = header(&post, &rendered);
        out.push_str(&rendered.text);
        Ok(CallToolResult::success(vec![ContentBlock::text(out)]))
    }
}

fn header(post: &Post, rendered: &render::Rendered) -> String {
    let mut h = format!(
        "slug: {}\ntitle: {}\ndate: {}\npublished: {}\nlabels: {}\ntotal_lines: {}\n",
        post.slug,
        post.title,
        post.date,
        post.published,
        post.labels.join(", "),
        rendered.total_lines
    );
    if let Some(n) = rendered.next_offset {
        h.push_str(&format!("next_offset: {n}\n"));
    }
    if let Some(n) = rendered.match_count {
        h.push_str(&format!("match_count: {n}\n"));
    }
    h.push_str("---\n");
    h
}

fn tool_error(msg: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(msg)])
}

fn internal(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EditorMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(INSTRUCTIONS)
    }
}

/// Builds the stateless streamable HTTP service mounted at `/mcp` on the editor router.
pub fn service(store: Arc<Store>) -> StreamableHttpService<EditorMcp, LocalSessionManager> {
    // Tailscale is the auth boundary and the read-write editor UI has no Host check either, so rmcp's loopback-only
    // Host allow-list would only reject the tailnet hostname without protecting anything.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts();
    StreamableHttpService::new(move || Ok(EditorMcp::new(store.clone())), Default::default(), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn text(r: &CallToolResult) -> &str {
        r.content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or_default()
    }

    async fn seeded() -> Result<EditorMcp, anyhow::Error> {
        let store = Store::default();
        for (slug, date, title, published, content) in [
            ("older-post", (2024, 1, 2), "Older", true, "# Hello\n\nSome text.\r\n"),
            ("newer-post", (2025, 6, 7), "Newer", false, "# Draft\n\nAlpha\nBeta\nGamma\n"),
        ] {
            let post = Post {
                date: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap_or_default(),
                slug: slug.to_string(),
                title: title.to_string(),
                published,
                labels: vec!["rust".to_string()],
            };
            store.upsert_post(&post, content).await?;
        }
        Ok(EditorMcp::new(Arc::new(store)))
    }

    fn req(slug: &str) -> GetPostRequest {
        GetPostRequest {
            slug: slug.to_string(),
            offset: None,
            limit: None,
            pattern: None,
            context: None,
        }
    }

    #[tokio::test]
    async fn test_list_posts_newest_first() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let r = mcp.list_posts().await?;
        assert_ne!(r.is_error, Some(true));
        let posts: Vec<serde_json::Value> = serde_json::from_str(text(&r))?;
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0]["slug"], "newer-post");
        assert_eq!(posts[0]["date"], "2025-06-07");
        assert_eq!(posts[0]["published"], false);
        assert_eq!(posts[1]["slug"], "older-post");
        Ok(())
    }

    #[tokio::test]
    async fn test_get_post_header_and_lines() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let r = mcp.get_post_inner(req("older-post")).await?;
        assert_ne!(r.is_error, Some(true));
        assert_eq!(
            text(&r),
            "slug: older-post\ntitle: Older\ndate: 2024-01-02\npublished: true\nlabels: rust\ntotal_lines: 3\n---\n\
1\t# Hello\n2\t\n3\tSome text.\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_get_post_pattern_and_paging_header() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let mut q = req("newer-post");
        q.pattern = Some("(?i)^beta$".to_string());
        q.context = Some(0);
        let r = mcp.get_post_inner(q).await?;
        assert!(text(&r).contains("match_count: 1\n---\n4\tBeta\n"));

        let mut q = req("newer-post");
        q.limit = Some(2);
        let r = mcp.get_post_inner(q).await?;
        assert!(text(&r).contains("total_lines: 5\nnext_offset: 3\n---\n1\t# Draft\n2\t\n"));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_post_limit_clamped() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let mut q = req("newer-post");
        q.limit = Some(0);
        q.offset = Some(0);
        let r = mcp.get_post_inner(q).await?;
        assert!(text(&r).ends_with("---\n1\t# Draft\n"));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_post_unknown_slug_is_tool_error() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let r = mcp.get_post_inner(req("nope")).await?;
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("nope"));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_post_invalid_regex_is_tool_error() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let mut q = req("newer-post");
        q.pattern = Some("(".to_string());
        let r = mcp.get_post_inner(q).await?;
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("invalid pattern"));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_post_oversized_regex_is_tool_error() -> Result<(), anyhow::Error> {
        let mcp = seeded().await?;
        let mut q = req("newer-post");
        q.pattern = Some(r"\w{80}".to_string());
        let r = mcp.get_post_inner(q).await?;
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("invalid pattern"));
        Ok(())
    }

    #[test]
    fn test_header_for_post_without_props() -> Result<(), anyhow::Error> {
        let post = Post {
            slug: "legacy".to_string(),
            ..Post::default()
        };
        let rendered = render::Rendered {
            text: String::new(),
            total_lines: 0,
            next_offset: None,
            match_count: None,
        };
        assert_eq!(
            header(&post, &rendered),
            "slug: legacy\ntitle: \ndate: 1970-01-01\npublished: false\nlabels: \ntotal_lines: 0\n---\n"
        );
        Ok(())
    }
}
