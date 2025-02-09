use anyhow::anyhow;
use pulldown_cmark::{html, BrokenLink, BrokenLinkCallback, CowStr, Event, HeadingLevel, Parser, Tag};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

struct BrokenLinkTracker {
    tracker: Arc<Mutex<Option<anyhow::Error>>>,
}

impl<'input> BrokenLinkCallback<'input> for BrokenLinkTracker {
    fn handle_broken_link(&mut self, link: BrokenLink<'input>) -> Option<(CowStr<'input>, CowStr<'input>)> {
        if let Ok(mut locked) = self.tracker.lock() {
            locked.replace(anyhow!("bad link '{}'", link.reference));
        }
        None
    }
}

fn pulldown_parser(content: &str) -> (Arc<Mutex<Option<anyhow::Error>>>, Parser<BrokenLinkTracker>) {
    let error_capture = Arc::new(Mutex::new(None::<anyhow::Error>));
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
    let mut hn = HeadingChecker {
        level: 0,
        expected_prefix: None,
        expected_number: vec![],
    };
    let lc = RelativeLinkChecker {
        links: valid_links.into_iter().collect(),
    };
    let mapped_parser = parser.map(|evt| {
        lc.observe(&evt).and_then(|_| hn.observe(&evt)).unwrap_or_else(|e| {
            if let Ok(mut l) = error_capture.as_ref().lock() {
                l.replace(e);
            }
            evt.clone()
        })
    });
    let mut output = String::new();
    html::push_html(&mut output, mapped_parser);

    if let Ok(locked) = error_capture.lock() {
        if let Some(e) = locked.as_ref() {
            return Err(anyhow::format_err!("{}", e));
        }
    }
    Ok(output)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelativeLinkChecker {
    links: HashSet<String>,
}

impl RelativeLinkChecker {
    fn observe<'a>(&self, event: &Event<'a>) -> Result<Event<'a>, anyhow::Error> {
        let capture = match &event {
            Event::Start(Tag::Image { dest_url, .. }) => Some(("image", dest_url)),
            Event::Start(Tag::Link { dest_url, .. }) => Some(("link", dest_url)),
            _ => None,
        };
        if let Some((link_type, dest_url)) = capture
            .filter(|_| !self.links.is_empty())
            .filter(|(_, dl)| !dl.starts_with("http://") && !dl.starts_with("https://") && !self.links.contains(&dl.to_string()))
        {
            return Err(anyhow!(
                "{} '{}' references a relative path which does not exist",
                link_type,
                dest_url
            ));
        }
        Ok(event.clone())
    }
}

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
struct HeadingChecker {
    level: i16,
    expected_number: Vec<usize>,
    expected_prefix: Option<String>,
}
impl HeadingChecker {
    fn hl_to_i16(h: HeadingLevel) -> i16 {
        match h {
            HeadingLevel::H1 => 1,
            HeadingLevel::H2 => 2,
            HeadingLevel::H3 => 3,
            HeadingLevel::H4 => 4,
            HeadingLevel::H5 => 5,
            HeadingLevel::H6 => 6,
        }
    }

    pub(crate) fn observe<'a>(&mut self, evt: &Event<'a>) -> Result<Event<'a>, anyhow::Error> {
        match evt {
            Event::Start(Tag::Heading { level, .. }) => {
                let num_level = Self::hl_to_i16(*level);
                if num_level < self.level - 1 || num_level > self.level + 1 {
                    return Err(anyhow::anyhow!(
                        "bad heading with level h{}: heading level should be h{}, h{}, or h{}",
                        num_level,
                        self.level,
                        self.level - 1,
                        self.level + 1
                    ));
                }
                if num_level == self.level && !self.expected_number.is_empty() {
                    if let Some(l) = self.expected_number.pop() {
                        self.expected_number.push(l + 1);
                    }
                } else if num_level > self.level {
                    self.expected_number.push(1)
                } else if num_level < self.level {
                    self.expected_number.pop();
                    if let Some(l) = self.expected_number.pop() {
                        self.expected_number.push(l + 1);
                    }
                }
                self.level = num_level;
                let mut out = String::with_capacity(self.expected_number.len() * 2 + 16);
                out.push_str("<small>");
                for (i, x) in self.expected_number.iter().enumerate() {
                    out.push_str(x.to_string().as_str());
                    if self.expected_number.len() == 1 || i < self.expected_number.len() - 1 {
                        out.push('.');
                    }
                }
                out.push_str("</small>");
                self.expected_prefix = Some(out);
            }
            Event::Text(s) => {
                if let Some(pref) = &self.expected_prefix {
                    return Ok(Event::InlineHtml(CowStr::from(format!("{} {}", pref, s))));
                }
                self.expected_prefix = None
            }
            _ => self.expected_prefix = None,
        }
        Ok(evt.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bad_links() {
        assert_eq!(
            convert(
                r"
[external](http://example.com)
[external](https://example.com)
[internal](/some-link)
![internal](/does-not-exist)
",
                HashSet::from(["/some-link".to_string()])
            )
            .unwrap_or_else(|e| e.to_string()),
            "image '/does-not-exist' references a relative path which does not exist",
        );
        assert_eq!(
            convert(r"![internal](/does-not-exist)", HashSet::new()).unwrap_or_else(|e| e.to_string()),
            "<p><img src=\"/does-not-exist\" alt=\"internal\" /></p>\n",
        );
    }

    #[test]
    fn test_bad_heading() {
        assert_eq!(
            convert(
                r"
# fine
# also fine
## indented
# unindented
### not fine
",
                HashSet::new()
            )
            .unwrap_or_else(|e| e.to_string()),
            "bad heading with level h3: heading level should be h1, h0, or h2",
        )
    }

    #[test]
    fn test_number_headings() {
        assert_eq!(
            convert(
                r"
# fine
# also fine
## indented
# unindented
",
                HashSet::new()
            )
            .unwrap_or_else(|e| e.to_string()),
            r##"<h1><small>1.</small> fine</h1>
<h1><small>2.</small> also fine</h1>
<h2><small>2.1</small> indented</h2>
<h1><small>3.</small> unindented</h1>
"##,
        )
    }
}
