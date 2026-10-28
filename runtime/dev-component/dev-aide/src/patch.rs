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

//! A file's diff as hunks, and the patch of some of them (plans/design-dev-aide.md 4.5): what `git apply --cached`
//! stages, `--cached -R` unstages and `-R` discards, a hunk at a time. Pure, so the hunk arithmetic is tested here.

use serde::Serialize;

/// One hunk of a one-file unified diff: its header's ranges and its lines (each with its ` `, `+`, `-` or `\` mark).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The text after the second `@@` (git's function context), kept as it was.
    #[serde(skip)]
    pub section: String,
    #[serde(skip)]
    pub lines: Vec<String>,
}

/// A one-file diff: the header lines before the first hunk, and the hunks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
}

/// `start,count` of a hunk header, the count 1 when it is left out.
fn range(text: &str) -> Option<(u32, u32)> {
    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

/// Parse `git diff` of one file. Lines that are not part of a hunk before the first `@@` are the header.
#[must_use]
pub fn parse(diff: &str) -> FileDiff {
    let mut out = FileDiff::default();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("@@ -") {
            let parsed = rest.split_once(" +").and_then(|(old, rest)| {
                let (new, section) = rest.split_once(" @@")?;
                Some((range(old)?, range(new)?, section.to_string()))
            });
            if let Some(((old_start, old_lines), (new_start, new_lines), section)) = parsed {
                out.hunks.push(Hunk {
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    section,
                    lines: Vec::new(),
                });
                continue;
            }
        }
        match out.hunks.last_mut() {
            Some(hunk) => hunk.lines.push(line.to_string()),
            None => out.header.push(line.to_string()),
        }
    }
    out
}

/// The patch of the `selected` hunks (by index) of `diff`, to apply forward (`reverse` false: the old side is the
/// target, so each hunk's new start is moved by the hunks left out before it) or reversed (the new side is the target,
/// so the old starts move). `None` when nothing is selected.
#[must_use]
pub fn patch_for(diff: &FileDiff, selected: &[usize], reverse: bool) -> Option<String> {
    let mut chosen: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|index| *index < diff.hunks.len())
        .collect();
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.is_empty() {
        return None;
    }
    let mut text = String::new();
    for line in &diff.header {
        text.push_str(line);
        text.push('\n');
    }
    // the lines the chosen hunks before this one added (new minus old)
    let mut shift: i64 = 0;
    for index in chosen {
        let hunk = &diff.hunks[index];
        let (old_start, new_start) = if reverse {
            (i64::from(hunk.new_start) - shift, i64::from(hunk.new_start))
        } else {
            (i64::from(hunk.old_start), i64::from(hunk.old_start) + shift)
        };
        let old_start = if hunk.old_lines == 0 && !reverse {
            i64::from(hunk.old_start)
        } else {
            old_start
        };
        let new_start = if hunk.new_lines == 0 && reverse {
            i64::from(hunk.new_start)
        } else {
            new_start
        };
        text.push_str(&format!(
            "@@ -{},{} +{},{} @@{}\n",
            old_start.max(0),
            hunk.old_lines,
            new_start.max(0),
            hunk.new_lines,
            hunk.section
        ));
        for line in &hunk.lines {
            text.push_str(line);
            text.push('\n');
        }
        shift += i64::from(hunk.new_lines) - i64::from(hunk.old_lines);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/f.txt b/f.txt
index 1111111..2222222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,5 @@
 one
+one and a half
 two
 three
 four
@@ -10,3 +11,2 @@ fn ten
 ten
-eleven
 twelve
";

    #[test]
    fn hunks_with_their_ranges_and_lines() {
        let diff = parse(DIFF);
        assert_eq!(diff.header.len(), 4);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(
            (
                diff.hunks[0].old_start,
                diff.hunks[0].old_lines,
                diff.hunks[0].new_start,
                diff.hunks[0].new_lines
            ),
            (1, 4, 1, 5)
        );
        assert_eq!(diff.hunks[1].section, " fn ten");
        assert_eq!(diff.hunks[1].lines, vec![" ten", "-eleven", " twelve"]);
    }

    #[test]
    fn a_later_hunk_alone_moves_to_where_the_target_has_it() {
        let diff = parse(DIFF);
        // forward, without the first hunk's added line, the second's new start is its old start
        let forward = patch_for(&diff, &[1], false).unwrap();
        assert!(forward.contains("@@ -10,3 +10,2 @@ fn ten\n"), "{forward}");
        assert!(forward.starts_with("diff --git a/f.txt b/f.txt\n"));
        // reversed, the target is the new side, so the old start moves back by the first hunk's line
        let reverse = patch_for(&diff, &[1], true).unwrap();
        assert!(reverse.contains("@@ -11,3 +11,2 @@ fn ten\n"), "{reverse}");
        // both hunks keep their own numbers
        let both = patch_for(&diff, &[0, 1], false).unwrap();
        assert!(
            both.contains("@@ -1,4 +1,5 @@\n") && both.contains("@@ -10,3 +11,2 @@"),
            "{both}"
        );
        assert_eq!(patch_for(&diff, &[], false), None);
        assert_eq!(patch_for(&diff, &[7], false), None);
    }
}
