# bloog (new blog)

**NOTE**: while open-source, this blog binary is not 

This is a rewrite of my previous [binary-blog](https://github.com/astromechza/binary-blog), this time, optimised for writing!

The previous iteration, embedded the posts and images into the binary itself, which was fun and allowed for some cool CI pipeline things, but it meant that writing new content was incredibly tedious and was stuck on slow Rust builds, slow github runners, and generally such slow iteration that it was easy to context switch away while writing.

This time, all content and images are stored in an object storage bucket, the binary for the blog changes very infrequently, and the binary hosts a mini markdown CMS system for authoring posts.

Features:

- Posts stored as markdown in object storage.
- Images stored in object storage and automatically resized and thumb-nailed on upload. SVGs are also supported.
- Automatic broken link detection.

```
Usage: bloog [OPTIONS] --store-url <STORE_URL> <COMMAND>

Commands:
  viewer  Launch the read-only viewer process
  editor  Launch the read-write editor process
  help    Print this message or the help of the given subcommand(s)

Options:
  -s, --store-url <STORE_URL>  The arrow/object_store url schema with config options as query args. [env: BLOOG_STORE_URL=]
  -p, --port <PORT>            The HTTP port to listen on. [env: BLOOG_PORT=] [default: 8080]
  -h, --help                   Print help
  -V, --version                Print version
```

Running against my Backblaze B2 bucket:

```
export BLOOG_STORE_URL='--store-url s3://<bucket>?access_key_id=<key id>&secret_access_key=<key>&endpoint=https://s3.us-east-005.backblazeb2.com'
bloog --port 8081 editor
bloog --port 8080 viewer
```
