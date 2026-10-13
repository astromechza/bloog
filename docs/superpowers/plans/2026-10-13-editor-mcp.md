# Editor MCP Endpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only MCP endpoint at `/mcp` on the editor process exposing `list_posts` and `get_post` (line window + regex grep) so Claude Code can read posts and suggest edits the author applies by hand.

**Architecture:** A pure render function (`src/editor/mcp/render.rs`) owns all line windowing, grep grouping and output budgeting. A thin `EditorMcp` rmcp server (`src/editor/mcp.rs`) maps the two tools onto `Store::list_posts` / `Store::get_post_raw` and the render function. `editor::run` mounts it with `nest_service("/mcp", StreamableHttpService)` in stateless JSON mode, sharing the existing `Arc<Store>` and `TraceLayer`.

**Tech Stack:** Rust 2024, axum 0.8, rmcp 3.5.x (`server`, `macros`, `transport-streamable-http-server`), schemars 1.x, regex, serde_json, tokio tests.

**Spec:** `docs/superpowers/specs/2026-10-13-editor-mcp-design.md`

## Global Constraints

- Read-only: no tool writes to the store; `Store` is not modified.
- Editor process only; the viewer is untouched. No new CLI flags.
- Tools: exactly `list_posts` and `get_post`.
- `get_post` defaults: `offset` 1 (`0` → 1); `limit` 400 without `pattern`, unbounded with `pattern`, clamped to 1..=1000 when given; `context` 2, max 10.
- Lines are `str::lines()`; output format `<line number>\t<text>`; non-contiguous match groups separated by `--`.
- Output budget: 60,000 characters (measured as bytes, which is never fewer than characters).
- Unknown slug / invalid regex → tool error (`isError: true`); store I/O failure → protocol error (`ErrorData::internal_error`).
- `StreamableHttpServerConfig::default().with_legacy_session_mode(false).with_json_response(true).disable_allowed_hosts()`.
- Commit prefixes `feat:` / `fix:` / `chore:`. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must pass before pushing.

## Review Focus

1. A single line longer than the 60,000 budget (a huge pasted paragraph or base64 blob) → it is still emitted and paging advances; never an empty page with `next_offset` pointing at the same line (infinite loop for the agent).
2. A match whose context overlaps a group already emitted on the previous page → context lines may repeat, but the page's `next_offset` always moves strictly forward.
3. CRLF content saved from the browser textarea → no `\r` in output and `^...$` anchored patterns still match.
4. `limit: 0` or `offset: 0` sent by the agent → treated as 1, not a panic or an empty result.
5. Post with no props object (legacy/partial post; `get_post_raw` returns `Post::default()` fields) → header still renders (empty title, default date) rather than erroring.

Tests for 1–4 are in Task 1; 5 is in Task 2.

---

## File Structure

- Create `src/editor/mcp/render.rs` — pure `render(content, &RenderOptions) -> Rendered`. No I/O, no rmcp types.
- Create `src/editor/mcp.rs` — `EditorMcp` server, request structs, header formatting, `service()` constructor for mounting.
- Modify `src/editor.rs` — `mod mcp;`, hoist `Arc<Store>`, `nest_service("/mcp", ...)`.
- Modify `Cargo.toml` — add `regex`, `rmcp`, `schemars`, `serde_json`.
- Modify `README.md` — connecting section.

---

### Task 1: Pure render function

**Files:**
- Create: `src/editor/mcp/render.rs`
- Create: `src/editor/mcp.rs` (only `mod render;` for now)
- Modify: `src/editor.rs:1` (add `mod mcp;`)
- Modify: `Cargo.toml` (add `regex`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub const OUTPUT_BUDGET: usize = 60_000;
  pub struct RenderOptions<'a> { pub offset: usize, pub limit: Option<usize>, pub pattern: Option<&'a regex::Regex>, pub context: usize, pub budget: usize }
  pub struct Rendered { pub text: String, pub total_lines: usize, pub next_offset: Option<usize>, pub match_count: Option<usize> }
  pub fn render(content: &str, opts: &RenderOptions) -> Rendered
  ```
  `offset`/`limit`/`context` arrive already defaulted and clamped (Task 2 does that); `render` still treats `offset == 0` as 1 and `limit == Some(0)` as `Some(1)` defensively.

- [ ] **Step 1: Add the dependency and module skeleton**

```bash
cargo add regex@1
```

In `src/editor.rs`, below `mod views;`:

```rust
mod mcp;
```

Create `src/editor/mcp.rs`:

```rust
mod render;
```

- [ ] **Step 2: Write the failing tests**

Create `src/editor/mcp/render.rs` containing only the tests module and the type signatures with `todo!()` bodies:

```rust
use regex::Regex;

/// Upper bound on the rendered text so a single tool result stays well under MCP client output caps.
/// Measured in bytes, which is never fewer than characters.
pub const OUTPUT_BUDGET: usize = 60_000;

pub struct RenderOptions<'a> {
    /// 1-based first line to scan.
    pub offset: usize,
    /// Number of lines to scan from `offset`. `None` scans to the end.
    pub limit: Option<usize>,
    pub pattern: Option<&'a Regex>,
    /// Lines of context either side of each match. Only used with `pattern`.
    pub context: usize,
    pub budget: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub total_lines: usize,
    pub next_offset: Option<usize>,
    pub match_count: Option<usize>,
}

pub fn render(content: &str, opts: &RenderOptions) -> Rendered {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(offset: usize, limit: Option<usize>) -> RenderOptions<'static> {
        RenderOptions {
            offset,
            limit,
            pattern: None,
            context: 2,
            budget: OUTPUT_BUDGET,
        }
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn test_window_whole_post() {
        let r = render("a\nb\nc\n", &opts(1, Some(400)));
        assert_eq!(r.text, "1\ta\n2\tb\n3\tc\n");
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.next_offset, None);
        assert_eq!(r.match_count, None);
    }

    #[test]
    fn test_window_paging_sets_next_offset() {
        let content = numbered(10);
        let r = render(&content, &opts(4, Some(3)));
        assert_eq!(r.text, "4\tline 4\n5\tline 5\n6\tline 6\n");
        assert_eq!(r.next_offset, Some(7));
        let r = render(&content, &opts(8, Some(3)));
        assert_eq!(r.text, "8\tline 8\n9\tline 9\n10\tline 10\n");
        assert_eq!(r.next_offset, None);
    }

    #[test]
    fn test_offset_past_end_and_empty_post() {
        let r = render("a\nb\n", &opts(5, Some(10)));
        assert_eq!(r.text, "");
        assert_eq!(r.total_lines, 2);
        assert_eq!(r.next_offset, None);
        let r = render("", &opts(1, Some(10)));
        assert_eq!(r.text, "");
        assert_eq!(r.total_lines, 0);
        assert_eq!(r.next_offset, None);
    }

    #[test]
    fn test_zero_offset_and_limit_treated_as_one() {
        let r = render("a\nb\n", &opts(0, Some(0)));
        assert_eq!(r.text, "1\ta\n");
        assert_eq!(r.next_offset, Some(2));
    }

    #[test]
    fn test_crlf_and_trailing_newline() {
        let re = Regex::new("^b$").unwrap();
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 0;
        let r = render("a\r\nb\r\nc\r\n", &o);
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.text, "2\tb\n");
        assert_eq!(r.match_count, Some(1));
    }

    #[test]
    fn test_pattern_groups_merge_and_separate() {
        let content = numbered(20);
        // matches on 3, 5 (overlapping context -> merged), and 15 (separate group)
        let re = Regex::new(r"^line (3|5|15)$").unwrap();
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 1;
        let r = render(&content, &o);
        assert_eq!(
            r.text,
            "2\tline 2\n3\tline 3\n4\tline 4\n5\tline 5\n6\tline 6\n--\n14\tline 14\n15\tline 15\n16\tline 16\n"
        );
        assert_eq!(r.match_count, Some(3));
        assert_eq!(r.next_offset, None);
    }

    #[test]
    fn test_pattern_adjacent_groups_merge() {
        let content = numbered(10);
        // context 1: group 1-3 for match 2, group 4-6 for match 5 -> adjacent, merged without separator
        let re = Regex::new(r"^line (2|5)$").unwrap();
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 1;
        let r = render(&content, &o);
        assert!(!r.text.contains("--"));
        assert!(r.text.starts_with("1\tline 1\n"));
        assert!(r.text.ends_with("6\tline 6\n"));
    }

    #[test]
    fn test_pattern_context_extends_past_window() {
        let content = numbered(10);
        let re = Regex::new(r"^line 5$").unwrap();
        let mut o = opts(5, Some(1));
        o.pattern = Some(&re);
        o.context = 2;
        let r = render(&content, &o);
        assert_eq!(r.text, "3\tline 3\n4\tline 4\n5\tline 5\n6\tline 6\n7\tline 7\n");
        assert_eq!(r.next_offset, Some(6));
    }

    #[test]
    fn test_pattern_no_matches() {
        let re = Regex::new("zzz").unwrap();
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        let r = render(&numbered(5), &o);
        assert_eq!(r.text, "");
        assert_eq!(r.match_count, Some(0));
        assert_eq!(r.next_offset, None);
    }

    #[test]
    fn test_budget_truncates_on_complete_line() {
        let content = numbered(100);
        let mut o = opts(1, None);
        o.budget = 30; // "1\tline 1\n" is 9 bytes; three lines fit (9+9+9=27), the fourth doesn't
        let r = render(&content, &o);
        assert_eq!(r.text, "1\tline 1\n2\tline 2\n3\tline 3\n");
        assert_eq!(r.next_offset, Some(4));
    }

    #[test]
    fn test_budget_truncates_on_complete_group() {
        let content = numbered(100);
        let re = Regex::new(r"^line (10|50)$").unwrap();
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 0;
        o.budget = 15; // first group "10\tline 10\n" (11 bytes) fits; "--\n50\tline 50\n" doesn't
        let r = render(&content, &o);
        assert_eq!(r.text, "10\tline 10\n");
        assert_eq!(r.match_count, Some(1));
        assert_eq!(r.next_offset, Some(50));
    }

    #[test]
    fn test_oversized_single_line_still_advances() {
        let content = format!("{}\nshort\n", "x".repeat(100));
        let mut o = opts(1, None);
        o.budget = 10;
        let r = render(&content, &o);
        assert!(r.text.starts_with("1\txxx"));
        assert_eq!(r.next_offset, Some(2));
        o.offset = 2;
        let r = render(&content, &o);
        assert_eq!(r.text, "2\tshort\n");
        assert_eq!(r.next_offset, None);
    }

    #[test]
    fn test_next_page_after_group_truncation_moves_forward() {
        let content = numbered(100);
        let re = Regex::new(r"^line (10|11|80)$").unwrap();
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 5;
        o.budget = 1; // nothing fits, so the first group (matches 10 and 11) is still emitted alone
        let r = render(&content, &o);
        assert_eq!(r.match_count, Some(2));
        // next page must start after the last match already shown, never at or before it
        assert_eq!(r.next_offset, Some(12));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test render::tests`
Expected: FAIL — tests panic with `not yet implemented`.

- [ ] **Step 4: Implement `render`**

Replace the `todo!()` body (keep the types and tests):

```rust
pub fn render(content: &str, opts: &RenderOptions) -> Rendered {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = opts.offset.max(1);
    let end = opts
        .limit
        .map(|l| (start + l.max(1) - 1).min(total_lines))
        .unwrap_or(total_lines);
    let line = |n: usize| format!("{n}\t{}\n", lines[n - 1]);

    let mut text = String::new();
    let mut next_offset = None;

    let Some(re) = opts.pattern else {
        for n in start..=end {
            let l = line(n);
            if !text.is_empty() && text.len() + l.len() > opts.budget {
                next_offset = Some(n);
                break;
            }
            text.push_str(&l);
        }
        if next_offset.is_none() && end < total_lines && start <= total_lines {
            next_offset = Some(end + 1);
        }
        return Rendered {
            text,
            total_lines,
            next_offset,
            match_count: None,
        };
    };

    // Each group is (first line, last line, first match, last match, match count), all 1-based.
    let mut groups: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
    for n in (start..=end).filter(|n| re.is_match(lines[n - 1])) {
        let lo = n.saturating_sub(opts.context).max(1);
        let hi = (n + opts.context).min(total_lines);
        match groups.last_mut() {
            Some(g) if lo <= g.1 + 1 => {
                g.1 = hi;
                g.3 = n;
                g.4 += 1;
            }
            _ => groups.push((lo, hi, n, n, 1)),
        }
    }

    let mut match_count = 0;
    for (i, &(lo, hi, first_match, last_match, count)) in groups.iter().enumerate() {
        let mut block = String::new();
        if i > 0 {
            block.push_str("--\n");
        }
        for n in lo..=hi {
            block.push_str(&line(n));
        }
        if !text.is_empty() && text.len() + block.len() > opts.budget {
            next_offset = Some(first_match);
            break;
        }
        text.push_str(&block);
        match_count += count;
        if text.len() > opts.budget && i + 1 < groups.len() {
            next_offset = Some(last_match + 1);
            break;
        }
    }
    if next_offset.is_none() && end < total_lines && start <= total_lines {
        next_offset = Some(end + 1);
    }
    Rendered {
        text,
        total_lines,
        next_offset,
        match_count: Some(match_count),
    }
}
```

How the budget rules map to the tests: the first line/group is always emitted even if oversized (so paging always
advances); a later line/group that would overflow is dropped and `next_offset` points at its first line / first match;
if the first group alone overflowed and more groups remain, `next_offset` is the line after its last match.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test render::tests`
Expected: PASS (13 tests).

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add Cargo.toml Cargo.lock src/editor.rs src/editor/mcp.rs src/editor/mcp/render.rs
git commit -m "feat: add line window and grep renderer for editor MCP"
```

`render` is not yet called from non-test code, so clippy will report `dead_code`. Add `#![allow(dead_code)]` at the top of `src/editor/mcp.rs` for this commit; Task 2 removes it.

---

### Task 2: `EditorMcp` server with `list_posts` and `get_post`

**Files:**
- Modify: `src/editor/mcp.rs`
- Modify: `Cargo.toml` (add `rmcp`, `schemars`, `serde_json`)

**Interfaces:**
- Consumes: `render::{render, RenderOptions, Rendered, OUTPUT_BUDGET}` from Task 1; `Store::list_posts() -> Result<Vec<Post>, anyhow::Error>`, `Store::get_post_raw(&str) -> Result<Option<(Post, String)>, anyhow::Error>` (`src/store.rs:433`, `:471`); `Post { date: NaiveDate, slug, title, published: bool, labels: Vec<String> }` (derives `Serialize`).
- Produces:
  ```rust
  #[derive(Clone)] pub struct EditorMcp { store: Arc<Store>, tool_router: ToolRouter<Self> }
  impl EditorMcp { pub fn new(store: Arc<Store>) -> Self }
  pub fn service(store: Arc<Store>) -> StreamableHttpService<EditorMcp, LocalSessionManager>
  ```

- [ ] **Step 1: Add dependencies**

```bash
cargo add rmcp@3.5 --features server,macros,transport-streamable-http-server
cargo add schemars@1
cargo add serde_json@1
```

- [ ] **Step 2: Write the failing tests**

Replace `src/editor/mcp.rs` with the module below. Bodies of `list_posts`, `get_post` and `header` are `todo!()` for now; the tests are final.

```rust
mod render;

use crate::store::{Post, Store};
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
    #[schemars(
        description = "Number of lines to scan from offset (1-1000). Default 400 without pattern; the whole post with pattern."
    )]
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
        todo!()
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
        todo!()
    }
}

fn header(post: &Post, rendered: &render::Rendered) -> String {
    todo!()
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
        r.content[0].as_text().expect("text content").text.as_str()
    }

    async fn seeded() -> EditorMcp {
        let store = Store::default();
        for (slug, date, title, published, content) in [
            ("older-post", (2024, 1, 2), "Older", true, "# Hello\n\nSome text.\r\n"),
            ("newer-post", (2025, 6, 7), "Newer", false, "# Draft\n\nAlpha\nBeta\nGamma\n"),
        ] {
            let post = Post {
                date: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap(),
                slug: slug.to_string(),
                title: title.to_string(),
                published,
                labels: vec!["rust".to_string()],
            };
            store.upsert_post(&post, content).await.unwrap();
        }
        EditorMcp::new(Arc::new(store))
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
    async fn test_list_posts_newest_first() {
        let mcp = seeded().await;
        let r = mcp.list_posts().await.unwrap();
        assert_ne!(r.is_error, Some(true));
        let posts: Vec<serde_json::Value> = serde_json::from_str(text(&r)).unwrap();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0]["slug"], "newer-post");
        assert_eq!(posts[0]["date"], "2025-06-07");
        assert_eq!(posts[0]["published"], false);
        assert_eq!(posts[1]["slug"], "older-post");
    }

    #[tokio::test]
    async fn test_get_post_header_and_lines() {
        let mcp = seeded().await;
        let r = mcp.get_post_inner(req("older-post")).await.unwrap();
        assert_ne!(r.is_error, Some(true));
        assert_eq!(
            text(&r),
            "slug: older-post\ntitle: Older\ndate: 2024-01-02\npublished: true\nlabels: rust\ntotal_lines: 3\n---\n\
1\t# Hello\n2\t\n3\tSome text.\n"
        );
    }

    #[tokio::test]
    async fn test_get_post_pattern_and_paging_header() {
        let mcp = seeded().await;
        let mut q = req("newer-post");
        q.pattern = Some("(?i)^beta$".to_string());
        q.context = Some(0);
        let r = mcp.get_post_inner(q).await.unwrap();
        assert!(text(&r).contains("match_count: 1\n---\n4\tBeta\n"));

        let mut q = req("newer-post");
        q.limit = Some(2);
        let r = mcp.get_post_inner(q).await.unwrap();
        assert!(text(&r).contains("total_lines: 5\nnext_offset: 3\n---\n1\t# Draft\n2\t\n"));
    }

    #[tokio::test]
    async fn test_get_post_limit_clamped() {
        let mcp = seeded().await;
        let mut q = req("newer-post");
        q.limit = Some(0);
        q.offset = Some(0);
        let r = mcp.get_post_inner(q).await.unwrap();
        assert!(text(&r).ends_with("---\n1\t# Draft\n"));
    }

    #[tokio::test]
    async fn test_get_post_unknown_slug_is_tool_error() {
        let mcp = seeded().await;
        let r = mcp.get_post_inner(req("nope")).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("nope"));
    }

    #[tokio::test]
    async fn test_get_post_invalid_regex_is_tool_error() {
        let mcp = seeded().await;
        let mut q = req("newer-post");
        q.pattern = Some("(".to_string());
        let r = mcp.get_post_inner(q).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("invalid pattern"));
    }

    #[test]
    fn test_header_for_post_without_props() {
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
    }
}
```

Remove the `#![allow(dead_code)]` added in Task 1.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test editor::mcp::tests`
Expected: FAIL — tests panic with `not yet implemented`.

- [ ] **Step 4: Implement the tool bodies**

```rust
    async fn list_posts(&self) -> Result<CallToolResult, ErrorData> {
        let mut posts = self.store.list_posts().await.map_err(internal)?;
        posts.sort();
        posts.reverse();
        let json = serde_json::to_string(&posts).map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }
```

```rust
    async fn get_post_inner(&self, req: GetPostRequest) -> Result<CallToolResult, ErrorData> {
        let pattern = match req.pattern.as_deref().map(regex::Regex::new).transpose() {
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
```

```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test editor::mcp`
Expected: PASS (13 render tests + 7 server tests). `NaiveDate::default()` is 1970-01-01 in chrono 0.4.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add Cargo.toml Cargo.lock src/editor/mcp.rs
git commit -m "feat: add read-only list_posts and get_post MCP tools"
```

`service()` is unused until Task 3; if clippy reports `dead_code` on it, add `#[allow(dead_code)]` on `service` only and remove it in Task 3.

---

### Task 3: Mount `/mcp` on the editor and document it

**Files:**
- Modify: `src/editor.rs:37-60` (`run`)
- Modify: `README.md`

**Interfaces:**
- Consumes: `mcp::service(store: Arc<Store>) -> StreamableHttpService<EditorMcp, LocalSessionManager>` from Task 2.
- Produces: `/mcp` route on the editor.

- [ ] **Step 1: Mount the service**

In `src/editor.rs` `run`, hoist the store and add the nested service before `.fallback(...)`:

```rust
pub async fn run(cfg: Config, store: Store) -> Result<(), anyhow::Error> {
    let store = Arc::new(store);
    let app = Router::new()
        .route("/", get(home_handler))
        // ... existing routes unchanged ...
        .route("/readyz", get(readyz_handler))
        .nest_service("/mcp", mcp::service(store.clone()))
        .fallback(not_found_handler)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(store)
        .layer(
            // ... existing TraceLayer unchanged ...
        );
```

Remove any `#[allow(dead_code)]` left on `service` from Task 2.

- [ ] **Step 2: Build and run the full test suite**

Run: `cargo test`
Expected: PASS, all existing tests plus the new ones.

- [ ] **Step 3: Manual protocol check against a local editor**

```bash
mkdir -p /tmp/bloog-mcp-store
cargo run -- --store-url file:///tmp/bloog-mcp-store --port 8081 editor
```

Create one post in the UI at `http://localhost:8081/posts/new`, then in another terminal:

```bash
curl -s -X POST http://localhost:8081/mcp -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"curl","version":"1"}}}'
```

Expected: a JSON `result` with `serverInfo` and the instructions text.

```bash
claude mcp add --transport http bloog-local http://localhost:8081/mcp
```

In a Claude Code session, ask it to list posts and read the one created. Expected: both tools work; the `get_post` output has the header, `---`, numbered lines. Then `claude mcp remove bloog-local`.

- [ ] **Step 4: README section**

Add after the "For local development" paragraph in `README.md`:

```markdown
The editor also serves a read-only [MCP](https://modelcontextprotocol.io) endpoint at `/mcp` so an AI agent can read
posts and suggest edits (which you apply yourself in the editor). It has no auth of its own: keep the editor on a
private network such as a Tailscale tailnet. To connect Claude Code:

```
claude mcp add --transport http bloog http://<editor-host>:8081/mcp
```
```

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add src/editor.rs README.md
git commit -m "feat: mount read-only MCP endpoint on the editor"
```
