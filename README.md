# bloog (new blog)

**NOTE**: while open-source, this binary is specific to me and not intended for general consumption (it has my name, bio, and themes embedded). Use this for inspiration or fork it and modify.

This is a rewrite of my previous [binary-blog](https://github.com/astromechza/binary-blog), this time, optimised for writing!

The previous iteration, embedded the posts and images into the binary itself, which was fun and allowed for some cool CI pipeline things, but it meant that writing new content was incredibly tedious and was stuck on slow Rust builds, slow github runners, and generally such slow iteration that it was easy to context switch away while writing.

This time, all content and images are stored in an object storage bucket, the binary for the blog changes very infrequently, and the binary hosts a mini markdown CMS system for authoring posts.

Features:

- Posts stored as markdown in object storage.
- Images stored in object storage and automatically resized and thumb-nailed on upload. SVGs are also supported.
- Automatic broken link detection.
- Automatic heading numbering, heading anchors, and table of contents generation.
- Validation of invalid markdown and invalid heading nesting.
- Validation of markdown conversion for all existing posts on startup.

```
Usage: bloog [OPTIONS] --store-url <STORE_URL> [HONEYCOMB_KEY] <COMMAND>

Commands:
  viewer  Launch the read-only viewer process
  editor  Launch the read-write editor process
  help    Print this message or the help of the given subcommand(s)

Arguments:
  [HONEYCOMB_KEY]  [env: BLOOG_HONEYCOMB_KEY=]

Options:
  -s, --store-url <STORE_URL>  The arrow/object_store url schema with config options as query args. [env: BLOOG_STORE_URL=]
  -p, --port <PORT>            The HTTP port to listen on. [env: BLOOG_PORT=] [default: 8080]
  -h, --help                   Print help
  -V, --version                Print version
```

Running against an S3-compatible bucket (this example uses Scaleway Object Storage in `fr-par`; any provider supported by
[object_store](https://docs.rs/object_store) works by changing the url and its options):

```
export BLOOG_STORE_URL='s3://<bucket>?access_key_id=<key id>&secret_access_key=<key>&region=fr-par&endpoint=https://s3.fr-par.scw.cloud'
bloog --port 8081 editor
bloog --port 8080 viewer
```

For local development, a directory works too: `--store-url file:///path/to/store`.

The editor also serves a read-only [MCP](https://modelcontextprotocol.io) endpoint at `/mcp` so an AI agent can read
posts and suggest edits (which you apply yourself in the editor). It has no auth of its own: keep the editor on a
private network such as a Tailscale tailnet. To connect Claude Code:

```
claude mcp add --transport http bloog http://<editor-host>:8081/mcp
```

Releasing a new version:

1. Update the version in [Cargo.toml](Cargo.toml).
2. Run `cargo build` or otherwise allow the Cargo.lock to be updated.
3. Push as a PR against main.

Best to do this as part of major PRs.
