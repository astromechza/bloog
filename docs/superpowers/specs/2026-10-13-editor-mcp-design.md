# Editor MCP endpoint — design

Date: 2026-10-13

## Goal

Let a local AI agent (Claude Code on the author's laptop) read blog posts so it can suggest edits and fixes. The
human stays in control: the agent has **read-only** access and every change is applied by the author in the existing
editor UI.

## Constraints

- Editor-only. The viewer process is untouched.
- The editor runs remotely and is reached over Tailscale. Tailscale is the auth boundary; no OAuth or tokens.
- Only used from Claude Code as a local MCP connection, not from cloud sessions.
- Read-only: no tool writes to the store, so no changes to `Store` and no concurrency concerns.
- The agent only sees **saved** content. Saving a published post makes it live, so reviewing unsaved edits to a
  published post means pasting the text into chat or working on a draft copy. No tool is added for this.
- Keep it small: two tools.

## Approach

Use the official Rust MCP SDK, `rmcp` 3.5.x, with its streamable HTTP server transport, mounted on the existing
editor axum router at `/mcp`.

Rejected alternatives:

- Hand-rolled JSON-RPC handler: avoids a dependency but means tracking MCP spec changes ourselves.
- REST API plus a Claude Code skill: clunkier for the agent and loses schema discovery.

## Tools

### `list_posts`

- Input: none.
- Output: a JSON text block, `[{slug, title, date, published, labels}]`, newest first (same ordering as the editor's
  posts page, `src/editor.rs:113`).

### `get_post`

Input (all integers unsigned):

| Field     | Type    | Default                     | Notes                                                    |
|-----------|---------|-----------------------------|----------------------------------------------------------|
| `slug`    | string  | —                           | Required.                                                |
| `offset`  | integer | 1                           | 1-based first line to scan. `0` is treated as `1`.       |
| `limit`   | integer | 400 without `pattern`; unbounded with `pattern` | Lines to scan from `offset`. Clamped to 1..=1000 when given. |
| `pattern` | string  | none                        | Optional regex (`regex` crate syntax), matched per line. |
| `context` | integer | 2                           | Lines of context either side of a match. Max 10. Only with `pattern`. |

Line semantics:

- Lines are `str::lines()` of the stored content: `\r\n` and `\n` both end a line, `\r` is stripped, and a trailing
  newline does not add an empty line. An empty post has `total_lines = 0`.
- Matching is per line, so patterns cannot span lines. `^#` also matches `#` comment lines inside code fences.

Selection:

- Without `pattern`: every line in the window `offset ..= offset + limit - 1`.
- With `pattern`: lines in the window that match, plus `context` lines either side. Context may extend outside the
  window (it is not clipped). Overlapping or adjacent groups merge; non-contiguous groups are separated by a `--` line
  (like `grep -C`).
- Output budget: rendering stops before the output would exceed 60,000 characters, checked line by line (a large
  merged match group can be cut mid-group). The response ends at the last complete line; `next_offset` points to the
  first line not emitted (or the group's first match, if the cut lands in its leading context). A page always reaches
  its first match, so a single oversized line is still returned and paging always advances.
- `next_offset` is present whenever scanning stopped before the last line of the post (window end or budget).
- `offset > total_lines` returns no lines and no `next_offset`.

Output is a single **plain text** content block (not JSON, to avoid escaping overhead): a small header, a `---`
line, then the numbered lines formatted `<line number>\t<text>`.

```
slug: my-post
title: My Post
date: 2026-10-01
published: false
labels: rust, mcp
total_lines: 212
next_offset: 181
match_count: 3
---
12	## Background
13
14	Some text...
--
40	## Design
```

`next_offset` and `match_count` lines are only included when applicable (`match_count` only with `pattern`; it counts
matching lines in the scanned range).

Errors:

- Unknown slug and invalid regex (including the parse message) are **tool errors** (`isError: true`) so the agent can
  recover. Implementation note: in rmcp, returning `Err(ErrorData)` becomes a JSON-RPC protocol error instead, so
  tool methods return `Result<String, String>` (or build `CallToolResult::error`).
- Store I/O failures are protocol errors (`ErrorData::internal_error`): they are not something the agent can fix.

Tool description should mention: `(?i)` for case-insensitive matching, `^#` to list headings with line numbers, and
the per-line matching caveat.

### Server instructions

The `ServerHandler` info states:

- The server is read-only; the agent proposes changes for the author to apply in the editor.
- The editor has no line numbers and soft-wraps paragraphs, so every suggestion must **quote a short unique snippet
  of the existing text** alongside the line number, then the replacement.
- Line numbers shift after the author saves an edit; re-fetch before giving further line-based suggestions.
- Only saved content is visible.

## Wiring

- New module `src/editor/mcp.rs` containing:
  - `EditorMcp { store: Arc<Store> }` with a `#[tool_router]` impl of the two tools and a `ServerHandler` impl.
  - Input structs deriving `Deserialize` + `JsonSchema`.
  - A pure function taking the content, `offset`, `limit`, optional compiled regex, `context`, and character budget,
    returning the rendered lines plus `total_lines`, `next_offset`, `match_count`. All non-trivial logic lives here.
  - `get_post` uses `Store::get_post_raw` (returns metadata and content in one call); it never calls `list_posts`.
  - Tool methods carry `#[instrument]` with `slug` / `pattern` fields, since every request otherwise traces as
    `POST /mcp`.
- `editor::run` hoists the `Arc<Store>` so the router state and `EditorMcp` share it, and mounts the service with
  `nest_service("/mcp", StreamableHttpService::new(..))` inside the existing `TraceLayer`, with the default
  `LocalSessionManager`.
- `StreamableHttpServerConfig` is `#[non_exhaustive]`, so build it from `Default::default()` with builder methods:
  `with_legacy_session_mode(false)` (stateless; tools hold no state), `with_json_response(true)` (plain JSON for
  request/response tools), `disable_allowed_hosts()`.
- No Host validation and no new CLI flags. rmcp's default allow-list is loopback only, which would reject the tailnet
  hostname. A Host check on read-only `/mcp` adds nothing while the read-write HTMX UI has none; the boundary is
  Tailscale (ideally an ACL limiting the editor port to the author's devices). If DNS rebinding ever matters, add a
  Host-check layer over the whole editor router as a separate change.
- Dependencies: `rmcp` (features `server`, `macros`, `transport-streamable-http-server`), `schemars` (version
  matching rmcp), `regex`, `serde_json` if not already direct.
- README: short section on connecting:
  `claude mcp add --transport http bloog http://<tailnet-host>:8081/mcp`.

## Testing

- Unit tests for the pure render function:
  - default window, `offset` / `limit` edges, `next_offset`, `offset` past end, empty post;
  - CRLF content and trailing newline;
  - match grouping: merge of overlapping/adjacent context, `--` separators, context extending past the window,
    zero matches, `match_count`;
  - character budget truncation sets `next_offset` and ends on a complete line/group.
- Integration test: `EditorMcp` over an in-memory `Store`, seeded with a couple of posts; call both tool methods
  directly and assert ordering, metadata header, unknown-slug and invalid-regex return `Err(String)` (which rmcp maps to `isError: true`).
- Manual check: run the editor locally, `claude mcp add` against it, call both tools once.
- `cargo clippy -- -D warnings` and `cargo fmt` clean.

## Out of scope

- Any write tools (drafts, edits, publish, delete).
- Markdown validation, rendered HTML, images, cross-post search, heading outline.
- Auth or Host validation beyond Tailscale.
