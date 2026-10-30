/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Line diffs of two texts: the compare view of resource versions and the reverse deltas `DbResourceFileHistory`
//! stores (Java left both as TODOs in `DbResourceReference`). The diff engine is `similar` (Patience). The unified
//! format is written here, not by `similar`, so that it and [`apply_unified`] split lines the same way: a line ends
//! after `\n` (a `\r\n` line keeps its `\r`), and a last line with no `\n` is followed by git's
//! `\ No newline at end of file`. A patch therefore applies back to exactly the bytes it was made from.

use std::fmt::Write as _;

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use similar::{Algorithm, ChangeTag, DiffOp, TextDiffConfig};

/// Lines of unchanged context around each change.
const CONTEXT: usize = 3;

const NO_NEWLINE: &str = "\\ No newline at end of file";

/// What one line of a diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Equal,
    Delete,
    Insert,
}

impl Tag {
    fn name(self) -> &'static str {
        match self {
            Tag::Equal => "equal",
            Tag::Delete => "delete",
            Tag::Insert => "insert",
        }
    }

    fn sign(self) -> char {
        match self {
            Tag::Equal => ' ',
            Tag::Delete => '-',
            Tag::Insert => '+',
        }
    }
}

/// One line of a hunk, with its line numbers (from 1) in the old and new text, and its text in spans; a span marked
/// `true` is the part of a changed line that differs from its counterpart. The text keeps its line ending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub tag: Tag,
    pub old_no: Option<usize>,
    pub new_no: Option<usize>,
    pub spans: Vec<(bool, String)>,
}

impl DiffLine {
    /// The whole line, with its line ending.
    pub fn text(&self) -> String {
        self.spans.iter().map(|(_, text)| text.as_str()).collect()
    }
}

/// A run of changes with its context, numbered as in a unified diff header: `old_start` and `new_start` count from 1,
/// and a side with no lines names the line before the change (0 at the start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    fn header(&self) -> String {
        format!(
            "@@ -{} +{} @@",
            range(self.old_start, self.old_lines),
            range(self.new_start, self.new_lines)
        )
    }
}

fn range(start: usize, lines: usize) -> String {
    if lines == 1 {
        start.to_string()
    } else {
        format!("{start},{lines}")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub insertions: usize,
    pub deletions: usize,
    pub hunks: usize,
}

/// The differences between two texts, from `from_name` to `to_name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDiff {
    pub from_name: String,
    pub to_name: String,
    pub hunks: Vec<Hunk>,
    pub stats: DiffStats,
}

/// The lines of `text`, each with its `\n` except a last line that has none. Empty text has no lines.
pub fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The line diff of `old` and `new` with word-level emphasis inside changed lines, for display.
pub fn diff(old: &str, new: &str, from_name: &str, to_name: &str) -> TextDiff {
    build(old, new, from_name, to_name, true)
}

/// The unified diff that turns `old` into `new`, with `--- from_name` and `+++ to_name` headers. Empty when the texts
/// are equal.
pub fn unified(old: &str, new: &str, from_name: &str, to_name: &str) -> String {
    build(old, new, from_name, to_name, false).unified()
}

fn build(old: &str, new: &str, from_name: &str, to_name: &str, inline: bool) -> TextDiff {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let text_diff = TextDiffConfig::default()
        .algorithm(Algorithm::Patience)
        .diff_slices(&old_lines, &new_lines);
    let mut stats = DiffStats::default();
    let mut hunks = Vec::new();
    for group in text_diff.grouped_ops(CONTEXT) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let (old_begin, old_end) = (first.old_range().start, last.old_range().end);
        let (new_begin, new_end) = (first.new_range().start, last.new_range().end);
        let mut lines = Vec::new();
        for op in &group {
            if inline && matches!(op, DiffOp::Replace { .. }) {
                for change in text_diff.iter_inline_changes(op) {
                    let spans = change
                        .iter_strings_lossy()
                        .map(|(emphasized, text)| (emphasized, text.into_owned()))
                        .collect();
                    lines.push(line(change.tag(), change.old_index(), change.new_index(), spans));
                }
            } else {
                for change in text_diff.iter_changes(op) {
                    let spans = vec![(false, change.value().to_string())];
                    lines.push(line(change.tag(), change.old_index(), change.new_index(), spans));
                }
            }
        }
        stats.insertions += lines.iter().filter(|line| line.tag == Tag::Insert).count();
        stats.deletions += lines.iter().filter(|line| line.tag == Tag::Delete).count();
        hunks.push(Hunk {
            old_start: header_start(old_begin, old_end),
            old_lines: old_end - old_begin,
            new_start: header_start(new_begin, new_end),
            new_lines: new_end - new_begin,
            lines,
        });
    }
    stats.hunks = hunks.len();
    TextDiff {
        from_name: from_name.to_string(),
        to_name: to_name.to_string(),
        hunks,
        stats,
    }
}

/// A side's start in a hunk header: from 1, or the line before when the side is empty.
fn header_start(begin: usize, end: usize) -> usize {
    if end > begin {
        begin + 1
    } else {
        begin
    }
}

fn line(tag: ChangeTag, old_index: Option<usize>, new_index: Option<usize>, spans: Vec<(bool, String)>) -> DiffLine {
    let tag = match tag {
        ChangeTag::Equal => Tag::Equal,
        ChangeTag::Delete => Tag::Delete,
        ChangeTag::Insert => Tag::Insert,
    };
    DiffLine {
        tag,
        old_no: old_index.map(|index| index + 1),
        new_no: new_index.map(|index| index + 1),
        spans,
    }
}

impl TextDiff {
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    /// The unified diff text, git style. Empty when there are no changes.
    pub fn unified(&self) -> String {
        let mut out = String::new();
        if self.hunks.is_empty() {
            return out;
        }
        let _ = writeln!(out, "--- {}", self.from_name);
        let _ = writeln!(out, "+++ {}", self.to_name);
        for hunk in &self.hunks {
            out.push_str(&hunk.header());
            out.push('\n');
            for line in &hunk.lines {
                let text = line.text();
                out.push(line.tag.sign());
                out.push_str(&text);
                if !text.ends_with('\n') {
                    out.push('\n');
                    out.push_str(NO_NEWLINE);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// An HTML table of the hunks: old and new line numbers, a sign column, and the line with `<del>` and `<ins>`
    /// around the changed words. Every text is escaped, `{` included so a qvt screen's Vue compile reads no
    /// interpolation in it; the table has no script. Styled by the `moqui-diff` rules in the webroot CSS.
    pub fn render_html(&self) -> String {
        let mut out = String::from("<table class=\"moqui-diff\">");
        let _ = write!(
            out,
            "<thead><tr><th class=\"ln\" title=\"{from}\">{from}</th><th class=\"ln\" title=\"{to}\">{to}</th>\
             <th class=\"sign\"></th><th class=\"code\"></th></tr></thead><tbody>",
            from = escape(&self.from_name),
            to = escape(&self.to_name)
        );
        if self.hunks.is_empty() {
            out.push_str("<tr class=\"diff-none\"><td colspan=\"4\">No differences</td></tr>");
        }
        for hunk in &self.hunks {
            let _ = write!(
                out,
                "<tr class=\"diff-hunk\"><td class=\"ln\"></td><td class=\"ln\"></td><td class=\"sign\"></td>\
                 <td class=\"code\">{}</td></tr>",
                escape(&hunk.header())
            );
            for line in &hunk.lines {
                let (class, mark) = match line.tag {
                    Tag::Equal => ("diff-ctx", ""),
                    Tag::Delete => ("diff-del", "del"),
                    Tag::Insert => ("diff-ins", "ins"),
                };
                let _ = write!(
                    out,
                    "<tr class=\"{class}\"><td class=\"ln\">{}</td><td class=\"ln\">{}</td><td class=\"sign\">{}</td>\
                     <td class=\"code\">",
                    line.old_no.map(|no| no.to_string()).unwrap_or_default(),
                    line.new_no.map(|no| no.to_string()).unwrap_or_default(),
                    line.tag.sign()
                );
                let last = line.spans.len().saturating_sub(1);
                for (index, (emphasized, text)) in line.spans.iter().enumerate() {
                    let text = if index == last {
                        trim_line_end(text)
                    } else {
                        text.as_str()
                    };
                    if text.is_empty() {
                        continue;
                    }
                    if *emphasized && !mark.is_empty() {
                        let _ = write!(out, "<{mark}>{}</{mark}>", escape(text));
                    } else {
                        out.push_str(&escape(text));
                    }
                }
                out.push_str("</td></tr>");
            }
        }
        out.push_str("</tbody></table>");
        out
    }

    /// The diff as a script value: `{fromName, toName, unifiedDiff, html, stats: {insertions, deletions, hunks},
    /// hunks: [{oldStart, oldLines, newStart, newLines, lines: [{tag, oldNo, newNo, text}]}]}`.
    pub fn to_value(&self) -> Value {
        let mut stats = IndexMap::new();
        stats.insert("insertions".to_string(), count(self.stats.insertions));
        stats.insert("deletions".to_string(), count(self.stats.deletions));
        stats.insert("hunks".to_string(), count(self.stats.hunks));
        let hunks = self
            .hunks
            .iter()
            .map(|hunk| {
                let lines = hunk
                    .lines
                    .iter()
                    .map(|line| {
                        let mut map = IndexMap::new();
                        map.insert("tag".to_string(), Value::string(line.tag.name()));
                        map.insert("oldNo".to_string(), line.old_no.map_or(Value::Null, count));
                        map.insert("newNo".to_string(), line.new_no.map_or(Value::Null, count));
                        map.insert("text".to_string(), Value::string(trim_line_end(&line.text())));
                        Value::Map(map)
                    })
                    .collect();
                let mut map = IndexMap::new();
                map.insert("oldStart".to_string(), count(hunk.old_start));
                map.insert("oldLines".to_string(), count(hunk.old_lines));
                map.insert("newStart".to_string(), count(hunk.new_start));
                map.insert("newLines".to_string(), count(hunk.new_lines));
                map.insert("lines".to_string(), Value::List(lines));
                Value::Map(map)
            })
            .collect();
        let mut map = IndexMap::new();
        map.insert("fromName".to_string(), Value::string(&self.from_name));
        map.insert("toName".to_string(), Value::string(&self.to_name));
        map.insert("unifiedDiff".to_string(), Value::string(self.unified()));
        map.insert("html".to_string(), Value::string(self.render_html()));
        map.insert("stats".to_string(), Value::Map(stats));
        map.insert("hunks".to_string(), Value::List(hunks));
        Value::Map(map)
    }
}

fn count(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

fn trim_line_end(text: &str) -> &str {
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.strip_suffix('\r').unwrap_or(text)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // the qvt render mode compiles screen text as a Vue template: a brace written as a reference is never
            // read as an interpolation, as the qvt macros write them
            '{' => out.push_str("&#123;"),
            other => out.push(other),
        }
    }
    out
}

/// One parsed hunk of a patch: where it starts in the base, and the lines it replaces and the lines it puts there,
/// each with its line ending.
struct PatchHunk {
    header: String,
    old_start: usize,
    old_count: usize,
    old: Vec<String>,
    new: Vec<String>,
}

/// Apply a unified diff to `base` strictly: every hunk must apply at the line its header names, in order, with each
/// context and removed line equal to the base's byte for byte. There is no fuzz and no offset search; a stored delta
/// that does not match its base is an error naming the hunk, never a guess.
pub fn apply_unified(base: &str, patch: &str) -> Result<String, Error> {
    let hunks = parse_patch(patch)?;
    let base_lines = split_lines(base);
    let mut out = String::with_capacity(base.len() + patch.len());
    let mut cursor = 0usize;
    for (index, hunk) in hunks.iter().enumerate() {
        let position = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start - 1
        };
        let end = position + hunk.old.len();
        if position < cursor || end > base_lines.len() {
            return Err(Error::new(format!(
                "hunk {} ({}) is outside the text, which has {} lines",
                index + 1,
                hunk.header,
                base_lines.len()
            )));
        }
        if let Some(offset) = (0..hunk.old.len()).find(|&offset| base_lines[position + offset] != hunk.old[offset]) {
            return Err(Error::new(format!(
                "hunk {} ({}) does not match line {} of the text",
                index + 1,
                hunk.header,
                position + offset + 1
            )));
        }
        for line in &base_lines[cursor..position] {
            out.push_str(line);
        }
        for line in &hunk.new {
            out.push_str(line);
        }
        cursor = end;
    }
    for line in &base_lines[cursor..] {
        out.push_str(line);
    }
    Ok(out)
}

fn parse_patch(patch: &str) -> Result<Vec<PatchHunk>, Error> {
    let lines = split_lines(patch);
    let mut hunks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let raw = lines[index];
        index += 1;
        if !raw.starts_with("@@") {
            if hunks.is_empty() {
                // `---`, `+++` and any `diff`/`index` lines before the first hunk
                continue;
            }
            return Err(Error::new(format!("unexpected line in patch: {}", trim_line_end(raw))));
        }
        let header = trim_line_end(raw).to_string();
        let (old_start, old_count, new_count) = parse_header(&header)?;
        let mut hunk = PatchHunk {
            header,
            old_start,
            old_count,
            old: Vec::new(),
            new: Vec::new(),
        };
        // Which side the last body line went to, so a `\ No newline` marker can take its newline off.
        let mut last: Option<(bool, bool)> = None;
        while hunk.old.len() < old_count || hunk.new.len() < new_count || next_is_marker(&lines, index) {
            let Some(raw) = lines.get(index) else {
                return Err(Error::new(format!("patch ends inside hunk {}", hunk.header)));
            };
            index += 1;
            let (sign, text) = match raw.chars().next() {
                Some(sign @ (' ' | '-' | '+' | '\\')) => (sign, &raw[1..]),
                // A blank context line some tools write without its leading space.
                Some('\n') => (' ', *raw),
                _ => {
                    return Err(Error::new(format!(
                        "bad line in hunk {}: {}",
                        hunk.header,
                        trim_line_end(raw)
                    )))
                }
            };
            match sign {
                '\\' => {
                    let Some((to_old, to_new)) = last else {
                        return Err(Error::new(format!("misplaced newline marker in hunk {}", hunk.header)));
                    };
                    if to_old {
                        strip_newline(hunk.old.last_mut());
                    }
                    if to_new {
                        strip_newline(hunk.new.last_mut());
                    }
                    last = None;
                }
                ' ' => {
                    hunk.old.push(text.to_string());
                    hunk.new.push(text.to_string());
                    last = Some((true, true));
                }
                '-' => {
                    hunk.old.push(text.to_string());
                    last = Some((true, false));
                }
                _ => {
                    hunk.new.push(text.to_string());
                    last = Some((false, true));
                }
            }
            if hunk.old.len() > old_count || hunk.new.len() > new_count {
                return Err(Error::new(format!(
                    "hunk {} has more lines than its header says",
                    hunk.header
                )));
            }
        }
        if old_count > 0 && old_start == 0 {
            return Err(Error::new(format!("hunk {} starts at line 0", hunk.header)));
        }
        hunks.push(hunk);
    }
    Ok(hunks)
}

fn next_is_marker(lines: &[&str], index: usize) -> bool {
    lines.get(index).is_some_and(|line| line.starts_with('\\'))
}

fn strip_newline(line: Option<&mut String>) {
    if let Some(line) = line {
        if line.ends_with('\n') {
            line.pop();
        }
    }
}

/// `@@ -a[,b] +c[,d] @@ ...` as (a, b, d).
fn parse_header(header: &str) -> Result<(usize, usize, usize), Error> {
    let bad = || Error::new(format!("bad hunk header: {header}"));
    let body = header.strip_prefix("@@ ").ok_or_else(bad)?;
    let body = &body[..body.find(" @@").ok_or_else(bad)?];
    let (old, new) = body.split_once(' ').ok_or_else(bad)?;
    let (old_start, old_count) = parse_range(old.strip_prefix('-').ok_or_else(bad)?).ok_or_else(bad)?;
    let (_, new_count) = parse_range(new.strip_prefix('+').ok_or_else(bad)?).ok_or_else(bad)?;
    Ok((old_start, old_count, new_count))
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(old: &str, new: &str) {
        let patch = unified(old, new, "a", "b");
        assert_eq!(apply_unified(old, &patch).expect(&patch), new, "patch:\n{patch}");
        let back = unified(new, old, "b", "a");
        assert_eq!(apply_unified(new, &back).expect(&back), old, "reverse patch:\n{back}");
    }

    #[test]
    fn patches_apply_back_to_the_exact_text() {
        let cases = [
            ("", ""),
            ("same\n", "same\n"),
            ("", "one\ntwo\n"),
            ("one\ntwo\n", ""),
            ("one", "one\n"),
            ("one\n", "one"),
            ("a\nb\nc", "a\nB\nc"),
            ("a\r\nb\r\nc\r\n", "a\r\nX\r\nc\r\n"),
            ("a\r\nb\r\n", "a\nb\n"),
            ("x\n\n\ny\n", "x\n\ny\n"),
            ("only", "different"),
        ];
        for (old, new) in cases {
            round_trip(old, new);
        }
    }

    #[test]
    fn many_hunks_round_trip() {
        let old: String = (1..=200).map(|n| format!("line {n}\n")).collect();
        let new: String = (1..=200)
            .filter(|n| n % 37 != 0)
            .map(|n| {
                if n % 23 == 0 {
                    format!("changed {n}\n")
                } else {
                    format!("line {n}\n")
                }
            })
            .chain(["appended\n".to_string()])
            .collect();
        let diff = diff(&old, &new, "a", "b");
        assert!(diff.stats.hunks > 5, "{:?}", diff.stats);
        round_trip(&old, &new);
    }

    /// Edit sequences from a fixed linear congruential generator: inserts, deletes and replacements anywhere,
    /// blank lines and a missing last newline.
    #[test]
    fn generated_edits_round_trip() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(seed >> 33).unwrap_or(0) % bound.max(1)
        };
        let words = ["alpha", "beta", "", "gamma delta", "  indented", "}", "{"];
        let mut text = String::from("start\n");
        for _ in 0..200 {
            let mut lines: Vec<String> = split_lines(&text).iter().map(|line| (*line).to_string()).collect();
            for _ in 0..=next(4) {
                let at = next(lines.len() + 1);
                let word = format!("{}{}\n", words[next(words.len())], next(9));
                match next(3) {
                    0 => lines.insert(at.min(lines.len()), word),
                    1 if at < lines.len() => {
                        lines.remove(at);
                    }
                    _ if at < lines.len() => lines[at] = word,
                    _ => lines.push(word),
                }
            }
            let mut next_text: String = lines.concat();
            if next(5) == 0 {
                next_text = next_text.trim_end_matches('\n').to_string();
            }
            round_trip(&text, &next_text);
            text = next_text;
        }
    }

    #[test]
    fn a_patch_that_does_not_match_its_base_is_an_error() {
        let patch = unified("a\nb\nc\n", "a\nB\nc\n", "1", "2");
        let err = apply_unified("a\nb2\nc\n", &patch).unwrap_err();
        assert!(err.to_string().contains("hunk 1"), "{err}");
        assert!(apply_unified("a\n", &patch).is_err());
        assert!(apply_unified("a\nb\nc\n", "@@ nonsense @@\n").is_err());
        assert!(apply_unified("a\nb\nc\n", "@@ -1,2 +1,2 @@\n a\n").is_err());
    }

    #[test]
    fn unified_text_is_git_style() {
        let patch = unified("a\nb\nc\n", "a\nB\nc", "02", "01");
        assert_eq!(
            patch,
            "--- 02\n+++ 01\n@@ -1,3 +1,3 @@\n a\n-b\n-c\n+B\n+c\n\\ No newline at end of file\n"
        );
        assert_eq!(unified("same\n", "same\n", "x", "y"), "");
    }

    #[test]
    fn html_marks_changed_words_and_escapes_text() {
        let diff = diff("keep\n<b> the old word\n", "keep\n<b> the new word\n", "01", "02");
        let html = diff.render_html();
        assert!(html.contains("<del>old</del>"), "{html}");
        assert!(html.contains("<ins>new</ins>"), "{html}");
        assert!(html.contains("&lt;b&gt; the"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
        assert_eq!(
            diff.stats,
            DiffStats {
                insertions: 1,
                deletions: 1,
                hunks: 1
            }
        );
        let script = super::diff("", "<script>alert(1)</script> {{ ec.user }}\n", "a", "b").render_html();
        assert!(!script.contains("<script>"), "{script}");
        assert!(!script.contains("{{"), "{script}");
        assert!(super::diff("x\n", "x\n", "a", "b")
            .render_html()
            .contains("No differences"));
    }

    #[test]
    fn script_value_has_stats_and_lines() {
        let Value::Map(map) = diff("a\n", "a\nb\n", "01", "02").to_value() else {
            panic!("not a map");
        };
        let Some(Value::Map(stats)) = map.get("stats") else {
            panic!("no stats");
        };
        assert_eq!(stats.get("insertions"), Some(&Value::I64(1)));
        assert!(matches!(map.get("unifiedDiff"), Some(Value::String(text)) if text.contains("+b")));
        let Some(Value::List(hunks)) = map.get("hunks") else {
            panic!("no hunks");
        };
        assert_eq!(hunks.len(), 1);
    }
}
