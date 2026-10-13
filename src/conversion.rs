use crate::store::{Image, Post};
use anyhow::anyhow;
use maud::html;
use pulldown_cmark::{BrokenLink, BrokenLinkCallback, CowStr, Event, HeadingLevel, Parser, Tag, TagEnd, html};
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tracing::instrument;
use url::Url;

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

fn pulldown_parser(content: &str) -> (Arc<Mutex<Option<anyhow::Error>>>, Parser<'_, BrokenLinkTracker>) {
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
    let content = normalize_heading_spaces(content);
    let (error_capture, parser) = pulldown_parser(&content);
    let mut hn = HeadingChecker::default();
    let lc = RelativeLinkChecker { links: valid_links };
    let mut events = Vec::new();
    for evt in parser {
        match lc.observe(&evt).and_then(|_| hn.observe(&evt)) {
            Ok(mapped) => events.extend(mapped),
            Err(e) => {
                if let Ok(mut l) = error_capture.as_ref().lock() {
                    l.replace(e);
                }
                events.push(evt);
            }
        }
    }
    let mut output = String::new();
    html::push_html(&mut output, events.into_iter());

    if let Ok(locked) = error_capture.lock()
        && let Some(e) = locked.as_ref()
    {
        return Err(anyhow::format_err!("{}", e));
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
        if let Some((link_type, dest_url)) = capture.filter(|_| !self.links.is_empty()).filter(|(_, dl)| !self.is_valid(dl)) {
            return Err(anyhow!(
                "{} '{}' references a relative path which does not exist",
                link_type,
                dest_url
            ));
        }
        Ok(event.clone())
    }

    /// In-page anchors and absolute urls with a scheme (http, https, mailto, ..) are always valid. Relative paths must
    /// reference a known post or image, ignoring any query string or fragment.
    fn is_valid(&self, dest_url: &str) -> bool {
        if dest_url.starts_with('#') || Url::parse(dest_url).is_ok() {
            return true;
        }
        let path = dest_url.split(['#', '?']).next().unwrap_or_default();
        self.links.contains(path)
    }
}

/// Fixes lines like `#<nbsp>title` that were meant to be headings but would render as plain text because the space
/// after the `#` is not a normal space. macOS inserts U+00A0 when Option+Space is typed, which is easy to do by accident
/// while holding Option to type `#` on some keyboard layouts. The content is parsed first so that only text at the start
/// of a line is touched; code blocks and inline code arrive as other event types and are left alone.
pub fn normalize_heading_spaces(content: &str) -> Cow<'_, str> {
    let mut fixes = Vec::new();
    let mut at_line_start = false;
    for (event, range) in pulldown_parser(content).1.into_offset_iter() {
        let was_at_line_start = std::mem::replace(
            &mut at_line_start,
            matches!(
                event,
                Event::Start(Tag::Paragraph | Tag::Item) | Event::SoftBreak | Event::HardBreak
            ),
        );
        if !was_at_line_start || !matches!(event, Event::Text(_)) {
            continue;
        }
        let raw = &content[range.clone()];
        let after_hashes = raw.trim_start_matches('#');
        let hashes = raw.len() - after_hashes.len();
        if let Some(c) = after_hashes.chars().next()
            && (1..=6).contains(&hashes)
            && c.is_whitespace()
            && c != ' '
            && c != '\t'
        {
            let at = range.start + hashes;
            fixes.push(at..at + c.len_utf8());
        }
    }
    if fixes.is_empty() {
        return Cow::Borrowed(content);
    }
    let mut out = content.to_string();
    for fix in fixes.into_iter().rev() {
        out.replace_range(fix, " ");
    }
    Cow::Owned(out)
}

/// Numbers headings, wraps each heading's content in a self-link, and builds the table of contents. The events inside
/// a heading are buffered until the heading ends because its text may arrive as several events (eg: smart punctuation,
/// emphasis, or inline code).
#[derive(Debug, Default)]
struct HeadingChecker<'a> {
    level: i16,
    expected_number: Vec<usize>,
    toc: String,
    pending: Option<PendingHeading<'a>>,
}

#[derive(Debug)]
struct PendingHeading<'a> {
    prefix: String,
    start: Event<'a>,
    inner: Vec<Event<'a>>,
}

impl<'a> HeadingChecker<'a> {
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

    /// Returns the events to emit in place of the observed event; empty while a heading is being buffered.
    fn observe(&mut self, evt: &Event<'a>) -> Result<Vec<Event<'a>>, anyhow::Error> {
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
                let mut prefix = String::with_capacity(self.expected_number.len() * 2);
                for (i, x) in self.expected_number.iter().enumerate() {
                    prefix.push_str(x.to_string().as_str());
                    if self.expected_number.len() == 1 || i < self.expected_number.len() - 1 {
                        prefix.push('.');
                    }
                }
                self.pending = Some(PendingHeading {
                    prefix,
                    start: evt.clone(),
                    inner: vec![],
                });
                Ok(vec![])
            }
            Event::End(TagEnd::Heading(_)) => match self.pending.take() {
                Some(heading) => Ok(self.finish_heading(heading, evt.clone())),
                None => Ok(vec![evt.clone()]),
            },
            _ => match self.pending.as_mut() {
                Some(heading) => {
                    heading.inner.push(evt.clone());
                    Ok(vec![])
                }
                None => Ok(vec![evt.clone()]),
            },
        }
    }

    fn finish_heading(&mut self, heading: PendingHeading<'a>, end: Event<'a>) -> Vec<Event<'a>> {
        let text: String = heading
            .inner
            .iter()
            .filter_map(|e| match e {
                Event::Text(s) | Event::Code(s) => Some(s.as_ref()),
                _ => None,
            })
            .collect();
        let valid_id = Self::convert_to_valid_id(&text);
        let prefix = heading.prefix;
        self.toc.push_str(
            html! {
                li class=(format!("toc-l{}", self.expected_number.len())) {
                    a href={"#" (valid_id)} {
                         (prefix) " " (text)
                    }
                }
            }
            .0
            .as_str(),
        );
        let anchor_open = html! {
            small id=(valid_id) { (prefix) }
            " "
        };
        let mut out = Vec::with_capacity(heading.inner.len() + 4);
        out.push(heading.start);
        out.push(Event::InlineHtml(CowStr::from(format!(
            r##"<a class="hlink" href="#{}">{}"##,
            valid_id, anchor_open.0
        ))));
        out.extend(heading.inner);
        out.push(Event::InlineHtml(CowStr::from("</a>")));
        out.push(end);
        out
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
    fn test_links_with_anchors_and_schemes() {
        let links = HashSet::from(["/posts/x".to_string()]);
        for md in [
            "[jump](#setup)",
            "[mail](mailto:someone@example.com)",
            "[part](/posts/x#install)",
            "[query](/posts/x?a=b)",
        ] {
            assert!(convert(md, &links).is_ok(), "{md} should be valid");
        }
        assert_eq!(
            convert("[missing](/posts/y#install)", &links)
                .unwrap_or_else(|e| (e.to_string(), String::new()))
                .0,
            "link '/posts/y#install' references a relative path which does not exist",
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
    fn test_normalize_heading_spaces() {
        for (md, expected) in [
            ("#\u{a0}title", "# title"),
            ("###\u{a0}title", "### title"),
            ("para\n##\u{202f}title", "para\n## title"),
            ("- #\u{a0}title", "- # title"),
            ("> #\u{a0}title", "> # title"),
            ("#\u{a0}one\n\n##\u{a0}two", "# one\n\n## two"),
        ] {
            assert_eq!(normalize_heading_spaces(md), expected, "{md:?} should be fixed");
        }
        for md in [
            "# title",
            "#hashtag",
            "text with #\u{a0}inside",
            "```\n#\u{a0}in code\n```",
            "`#\u{a0}inline`",
            "#######\u{a0}too many",
        ] {
            assert!(
                matches!(normalize_heading_spaces(md), Cow::Borrowed(_)),
                "{md:?} should be untouched"
            );
        }
    }

    #[test]
    fn test_convert_heading_with_abnormal_space() {
        let (content, _) = convert("#\u{a0}title", &HashSet::new()).unwrap_or_else(|e| (e.to_string(), String::new()));
        assert_eq!(
            content,
            "<h1><a class=\"hlink\" href=\"#title\"><small id=\"title\">1.</small> title</a></h1>\n"
        );
    }

    #[test]
    fn test_headings_with_split_text() {
        let (content, toc) = convert(
            r"
# Don't panic
## Hello *world*
## `code` first
",
            &HashSet::new(),
        )
        .unwrap_or_else(|e| (e.to_string(), String::new()));
        assert_eq!(
            content,
            r##"<h1><a class="hlink" href="#dont-panic"><small id="dont-panic">1.</small> Don’t panic</a></h1>
<h2><a class="hlink" href="#hello-world"><small id="hello-world">1.1</small> Hello <em>world</em></a></h2>
<h2><a class="hlink" href="#code-first"><small id="code-first">1.2</small> <code>code</code> first</a></h2>
"##,
        );
        assert_eq!(
            toc,
            "<li class=\"toc-l1\"><a href=\"#dont-panic\">1. Don’t panic</a></li>\
            <li class=\"toc-l2\"><a href=\"#hello-world\">1.1 Hello world</a></li>\
            <li class=\"toc-l2\"><a href=\"#code-first\">1.2 code first</a></li>"
        );
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
