# bloog (new blog)

The idea is a stateless blog binary that pulls all content from an object storage bucket. 

## bloog-editor

Point it at an arrow-rs/object storage source (s3-style object store or file system). In general this is just a system that supports list, put, delete, etc.

It has basically 3 pages:

GET / - the landing page, it has a list of posts, followed by a list of image thumbnails
GET /posts/{id}/edit - displays a post in editing mode with [check], [save] and [(un)publish] buttons
	[check] is the same as [save]--dry-run and returns a set of errors from the post
	[publish] is the same as [save]
	the content is writtein in restructured text
POST /posts/{id}/check
POST /posts/{id}/save
GET /images/{id} - displays an image
POST /images/ - uploads an image, strips any metadata, stores the original, stores an 1000x1000 max size, stores a 150px thumbnail
DELETE /images/{id} - deletes an image

## bloog-server

Point it at the same object storage source. Load the parquet file.

GET /
GET /posts/
GET /posts/{id}
GET /images/{id}

## What's in the posts DB...
What do we need?

- date
- title
- content_type
- content
- labels
- image_ids
- bsky_post_url

THe duckdb thingy for rust doesn't seem very fully featured... quite disappointing...

## Could we just do this on object store too?

operations to supporT:

LIST and return the date, title, and labels of all posts.
We can do this one by just listing all objects under /posts/ and parsing them as we know how to do

/posts/<unique slug>/props/<base64-encoded props>
/posts/<unique slug>/content/raw
/posts/<unique slug>/labels/x/true
/posts/<unique slug>/labels/y/true


So to show the index page:

- list all objects, grab the posts, decode the props, annotate with labels, build a vec<posts> representation, sort, render.

To show a particular post by slug:

- list all objects under the slug, decode the props, annotate with labels.
- make a second request to grab the document content itself

To show all posts with a particular label, search by delimeter labels/x/ and grab the slugs from the common prefixes list.





