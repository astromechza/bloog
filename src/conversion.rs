use crate::store::{Image, Post};
use anyhow::anyhow;
use maud::html;
use pulldown_cmark::{html, BrokenLink, BrokenLinkCallback, CowStr, Event, HeadingLevel, Parser, Tag};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tracing::instrument;

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
            | pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_SMART_PUNCTUATION
            | pulldown_cmark::Options::ENABLE_SUBSCRIPT
            | pulldown_cmark::Options::ENABLE_SUPERSCRIPT,
        Some(BrokenLinkTracker {
            tracker: error_capture.clone(),
        }),
    );
    (error_capture, parser)
}

pub fn build_valid_links(ps: &[Post], is: &[Image]) -> HashSet<String> {
    is.iter()
        .flat_map(|i| {
            vec![
                format!("/images/{}", i.to_original().to_path_part().as_ref()),
                format!("/images/{}", i.to_medium().to_path_part().as_ref()),
            ]
            .into_iter()
        })
        .chain(ps.iter().map(|p| format!("/posts/{}", p.slug)))
        .collect::<HashSet<String>>()
}

#[instrument(skip_all, err)]
pub fn convert(content: &str, valid_links: &HashSet<String>) -> Result<(String, String), anyhow::Error> {
    let (error_capture, parser) = pulldown_parser(content);
    let mut hn = HeadingChecker {
        level: 0,
        expected_prefix: None,
        expected_number: vec![],
        toc: String::new(),
    };
    let lc = RelativeLinkChecker { links: valid_links };
    let mut output = String::new();
    {
        let mapped_parser = parser.map(|evt| {
            lc.observe(&evt).and_then(|_| hn.observe(&evt)).unwrap_or_else(|e| {
                if let Ok(mut l) = error_capture.as_ref().lock() {
                    l.replace(e);
                }
                evt.clone()
            })
        });
        html::push_html(&mut output, mapped_parser);
    };

    if let Ok(locked) = error_capture.lock() {
        if let Some(e) = locked.as_ref() {
            return Err(anyhow::format_err!("{}", e));
        }
    }
    Ok((output, hn.toc.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelativeLinkChecker<'a> {
    links: &'a HashSet<String>,
}

impl RelativeLinkChecker<'_> {
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
    toc: String,
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

    fn convert_to_valid_id(s: impl AsRef<str>) -> String {
        s.as_ref()
            .chars()
            .filter_map(|c| match c {
                'a'..='z' => Some(c),
                'A'..='Z' => Some(c.to_ascii_lowercase()),
                '0'..='9' => Some(c),
                '_' => Some(c),
                '-' => Some(c),
                ' ' => Some('-'),
                _ => None,
            })
            .collect()
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
                let mut out = String::with_capacity(self.expected_number.len() * 2);
                for (i, x) in self.expected_number.iter().enumerate() {
                    out.push_str(x.to_string().as_str());
                    if self.expected_number.len() == 1 || i < self.expected_number.len() - 1 {
                        out.push('.');
                    }
                }
                self.expected_prefix = Some(out);
            }
            Event::Text(s) => {
                if let Some(pref) = &self.expected_prefix {
                    let valid_id = Self::convert_to_valid_id(s);
                    self.toc.push_str(
                        html! {
                            li class=(format!("toc-l{}", self.expected_number.len())) {
                                a href={"#" (valid_id)} {
                                     (pref) " " (s)
                                }
                            }
                        }
                        .0
                        .as_str(),
                    );
                    return Ok(Event::InlineHtml(CowStr::from(
                        html! {
                            a class="hlink" href={"#" (valid_id) } {
                                small id=(valid_id) {
                                    (pref)
                                }
                                " "
                                (s)
                            }
                        }
                        .0,
                    )));
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
    fn test_typog() {
        let (content, _) = convert(
            r"
normal
_italic_
**bold**
~sub~
^sup^
~~strike~~
",
            &HashSet::new(),
        )
            .unwrap_or_else(|e| (e.to_string(), String::new()));
        assert_eq!(
            content,
            r##"<p>normal
<em>italic</em>
<strong>bold</strong>
<sub>sub</sub>
<sup>sup</sup>
<del>strike</del></p>
"##,
        );
    }

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
                &HashSet::from(["/some-link".to_string()])
            )
            .unwrap_or_else(|e| (e.to_string(), String::new()))
            .0,
            "image '/does-not-exist' references a relative path which does not exist",
        );
        assert_eq!(
            convert(r"![internal](/does-not-exist)", &HashSet::new())
                .unwrap_or_else(|e| (e.to_string(), String::new()))
                .0,
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
                &HashSet::new()
            )
            .unwrap_or_else(|e| (e.to_string(), String::new()))
            .0,
            "bad heading with level h3: heading level should be h1, h0, or h2",
        )
    }

    #[test]
    fn test_number_headings() {
        let (content, toc) = convert(
            r"
# fine
# also fine
## indented
# unindented
",
            &HashSet::new(),
        )
        .unwrap_or_else(|e| (e.to_string(), String::new()));
        assert_eq!(
            content,
            r##"<h1><a class="hlink" href="#fine"><small id="fine">1.</small> fine</a></h1>
<h1><a class="hlink" href="#also-fine"><small id="also-fine">2.</small> also fine</a></h1>
<h2><a class="hlink" href="#indented"><small id="indented">2.1</small> indented</a></h2>
<h1><a class="hlink" href="#unindented"><small id="unindented">3.</small> unindented</a></h1>
"##,
        );
        assert_eq!(
            toc,
            "<li class=\"toc-l1\"><a href=\"#fine\">1. fine</a></li>\
            <li class=\"toc-l1\"><a href=\"#also-fine\">2. also fine</a></li>\
            <li class=\"toc-l2\"><a href=\"#indented\">2.1 indented</a></li>\
            <li class=\"toc-l1\"><a href=\"#unindented\">3. unindented</a></li>"
        );
    }
}
