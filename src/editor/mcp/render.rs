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
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = opts.offset.max(1);
    let end = opts
        .limit
        .map(|l| start.saturating_add(l.max(1) - 1).min(total_lines))
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

    // Each group is (first line, last line, matching lines), all 1-based.
    let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for n in (start..=end).filter(|n| re.is_match(lines[n - 1])) {
        let lo = n.saturating_sub(opts.context).max(1);
        let hi = (n + opts.context).min(total_lines);
        match groups.last_mut() {
            Some(g) if lo <= g.1 + 1 => {
                g.1 = hi;
                g.2.push(n);
            }
            _ => groups.push((lo, hi, vec![n])),
        }
    }

    // The budget is checked line by line. The page always reaches its first match, so paging always advances; a
    // cut before a group's first match resumes at that match, otherwise at the first line not emitted.
    let mut match_count = 0;
    'groups: for (i, (lo, hi, matches)) in groups.iter().enumerate() {
        let first_match = matches[0];
        for n in *lo..=*hi {
            let sep = if i > 0 && n == *lo { "--\n" } else { "" };
            let l = line(n);
            let must_emit = i == 0 && n <= first_match;
            if !must_emit && text.len() + sep.len() + l.len() > opts.budget {
                next_offset = Some(n.max(first_match));
                match_count += matches.iter().filter(|m| **m < n).count();
                break 'groups;
            }
            text.push_str(sep);
            text.push_str(&l);
        }
        match_count += matches.len();
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
    fn test_window_whole_post() -> Result<(), anyhow::Error> {
        let r = render("a\nb\nc\n", &opts(1, Some(400)));
        assert_eq!(r.text, "1\ta\n2\tb\n3\tc\n");
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.next_offset, None);
        assert_eq!(r.match_count, None);
        Ok(())
    }

    #[test]
    fn test_window_paging_sets_next_offset() -> Result<(), anyhow::Error> {
        let content = numbered(10);
        let r = render(&content, &opts(4, Some(3)));
        assert_eq!(r.text, "4\tline 4\n5\tline 5\n6\tline 6\n");
        assert_eq!(r.next_offset, Some(7));
        let r = render(&content, &opts(8, Some(3)));
        assert_eq!(r.text, "8\tline 8\n9\tline 9\n10\tline 10\n");
        assert_eq!(r.next_offset, None);
        Ok(())
    }

    #[test]
    fn test_offset_past_end_and_empty_post() -> Result<(), anyhow::Error> {
        let r = render("a\nb\n", &opts(5, Some(10)));
        assert_eq!(r.text, "");
        assert_eq!(r.total_lines, 2);
        assert_eq!(r.next_offset, None);
        let r = render("", &opts(1, Some(10)));
        assert_eq!(r.text, "");
        assert_eq!(r.total_lines, 0);
        assert_eq!(r.next_offset, None);
        Ok(())
    }

    #[test]
    fn test_zero_offset_and_limit_treated_as_one() -> Result<(), anyhow::Error> {
        let r = render("a\nb\n", &opts(0, Some(0)));
        assert_eq!(r.text, "1\ta\n");
        assert_eq!(r.next_offset, Some(2));
        Ok(())
    }

    #[test]
    fn test_crlf_and_trailing_newline() -> Result<(), anyhow::Error> {
        let re = Regex::new("^b$")?;
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 0;
        let r = render("a\r\nb\r\nc\r\n", &o);
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.text, "2\tb\n");
        assert_eq!(r.match_count, Some(1));
        Ok(())
    }

    #[test]
    fn test_pattern_groups_merge_and_separate() -> Result<(), anyhow::Error> {
        let content = numbered(20);
        // matches on 3, 5 (overlapping context -> merged), and 15 (separate group)
        let re = Regex::new(r"^line (3|5|15)$")?;
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
        Ok(())
    }

    #[test]
    fn test_pattern_adjacent_groups_merge() -> Result<(), anyhow::Error> {
        let content = numbered(10);
        // context 1: group 1-3 for match 2, group 4-6 for match 5 -> adjacent, merged without separator
        let re = Regex::new(r"^line (2|5)$")?;
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 1;
        let r = render(&content, &o);
        assert!(!r.text.contains("--"));
        assert!(r.text.starts_with("1\tline 1\n"));
        assert!(r.text.ends_with("6\tline 6\n"));
        Ok(())
    }

    #[test]
    fn test_pattern_context_extends_past_window() -> Result<(), anyhow::Error> {
        let content = numbered(10);
        let re = Regex::new(r"^line 5$")?;
        let mut o = opts(5, Some(1));
        o.pattern = Some(&re);
        o.context = 2;
        let r = render(&content, &o);
        assert_eq!(r.text, "3\tline 3\n4\tline 4\n5\tline 5\n6\tline 6\n7\tline 7\n");
        assert_eq!(r.next_offset, Some(6));
        Ok(())
    }

    #[test]
    fn test_pattern_no_matches() -> Result<(), anyhow::Error> {
        let re = Regex::new("zzz")?;
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        let r = render(&numbered(5), &o);
        assert_eq!(r.text, "");
        assert_eq!(r.match_count, Some(0));
        assert_eq!(r.next_offset, None);
        Ok(())
    }

    #[test]
    fn test_budget_truncates_on_complete_line() -> Result<(), anyhow::Error> {
        let content = numbered(100);
        let mut o = opts(1, None);
        o.budget = 30; // "1\tline 1\n" is 9 bytes; three lines fit (9+9+9=27), the fourth doesn't
        let r = render(&content, &o);
        assert_eq!(r.text, "1\tline 1\n2\tline 2\n3\tline 3\n");
        assert_eq!(r.next_offset, Some(4));
        Ok(())
    }

    #[test]
    fn test_budget_truncates_on_complete_group() -> Result<(), anyhow::Error> {
        let content = numbered(100);
        let re = Regex::new(r"^line (10|50)$")?;
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 0;
        o.budget = 15; // first group "10\tline 10\n" (11 bytes) fits; "--\n50\tline 50\n" doesn't
        let r = render(&content, &o);
        assert_eq!(r.text, "10\tline 10\n");
        assert_eq!(r.match_count, Some(1));
        assert_eq!(r.next_offset, Some(50));
        Ok(())
    }

    #[test]
    fn test_oversized_single_line_still_advances() -> Result<(), anyhow::Error> {
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
        Ok(())
    }

    #[test]
    fn test_next_page_after_group_truncation_moves_forward() -> Result<(), anyhow::Error> {
        let content = numbered(100);
        let re = Regex::new(r"^line (10|11|80)$")?;
        let mut o = opts(1, None);
        o.pattern = Some(&re);
        o.context = 5;
        o.budget = 1; // nothing fits, so the page still reaches its first match (lines 5-10) and stops there
        let r = render(&content, &o);
        assert!(r.text.ends_with("10\tline 10\n"));
        assert_eq!(r.match_count, Some(1));
        // next page must start after the last match already shown, never at or before it
        assert_eq!(r.next_offset, Some(11));
        Ok(())
    }

    #[test]
    fn test_budget_caps_single_large_group() -> Result<(), anyhow::Error> {
        let content: String = (1..=2000).map(|i| format!("line {i} {}\n", "x".repeat(80))).collect();
        let re = Regex::new("line")?;
        let mut offset = 1;
        let mut pages = 0;
        loop {
            let o = RenderOptions {
                offset,
                limit: None,
                pattern: Some(&re),
                context: 2,
                budget: OUTPUT_BUDGET,
            };
            let r = render(&content, &o);
            assert!(r.text.len() <= OUTPUT_BUDGET, "page {pages} is {} bytes", r.text.len());
            pages += 1;
            match r.next_offset {
                Some(n) => {
                    assert!(n > offset, "next_offset {n} must move past {offset}");
                    assert_eq!(r.match_count, Some(n - offset));
                    offset = n;
                }
                None => break,
            }
        }
        assert!(pages > 1);
        Ok(())
    }

    #[test]
    fn test_huge_offset_does_not_overflow() -> Result<(), anyhow::Error> {
        let r = render("a\nb\n", &opts(usize::MAX, Some(1000)));
        assert_eq!(r.text, "");
        assert_eq!(r.next_offset, None);
        Ok(())
    }
}
