use pulldown_cmark::{html, BrokenLink, BrokenLinkCallback, CowStr, Event, Parser, Tag};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

struct BrokenLinkTracker {
    tracker: Arc<Mutex<Option<String>>>,
}

impl<'input> BrokenLinkCallback<'input> for BrokenLinkTracker {
    fn handle_broken_link(&mut self, link: BrokenLink<'input>) -> Option<(CowStr<'input>, CowStr<'input>)> {
        if let Ok(mut locked) = self.tracker.lock() {
            locked.replace(format!("bad link '{}'", link.reference));
        }
        None
    }
}

fn pulldown_parser(content: &str) -> (Arc<Mutex<Option<String>>>, Parser<BrokenLinkTracker>) {
    let error_capture = Arc::new(Mutex::new(None::<String>));
    let parser = Parser::new_with_broken_link_callback(
        content,
        pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_DEFINITION_LIST
            | pulldown_cmark::Options::ENABLE_FOOTNOTES
            | pulldown_cmark::Options::ENABLE_TABLES,
        Some(BrokenLinkTracker {
            tracker: error_capture.clone(),
        }),
    );
    (error_capture, parser)
}

pub fn convert(content: &str, valid_links: HashSet<String>) -> Result<String, anyhow::Error> {
    let (error_capture, parser) = pulldown_parser(content);
    let mapped_parser = parser.inspect(|event| {
        if !valid_links.is_empty() {
            if let Some((link_type, dest_url)) = match &event {
                Event::Start(Tag::Image { dest_url, .. }) => Some(("image", dest_url)),
                Event::Start(Tag::Link { dest_url, .. }) => Some(("link", dest_url)),
                _ => None,
            }
            .filter(|(_, dl)| !dl.starts_with("http://") && !dl.starts_with("https://") && !valid_links.contains(&dl.to_string()))
            {
                if let Ok(mut locked) = error_capture.lock() {
                    locked.replace(format!(
                        "{} '{}' references a relative path which does not exist",
                        link_type, dest_url
                    ));
                }
            }
        }
    });
    let mut output = String::new();
    html::push_html(&mut output, mapped_parser);

    if let Ok(locked) = error_capture.lock() {
        if let Some(e) = locked.as_ref() {
            return Err(anyhow::anyhow!("{}", e));
        }
    }
    Ok(output)
}
