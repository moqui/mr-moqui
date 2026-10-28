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

//! Source stamps (plans/design-dev-aide.md F10): on a dev instance each widget element of a screen keeps, as its
//! `mq-src` attribute, the file that wrote it and its line and column (the base screen, or the `screen-extend` whose
//! merged element it is), and the generated code of a stamped element names it to `sri.stamp` before its macro runs.
//! A render that asks for stamps (`moquiAide=source` or `X-Moqui-Aide: source`, a Dev AIDE user) writes the pending
//! stamp as `data-m-src` on the first tag its macro writes, so the screen builder's canvas maps what it shows back to
//! the source. Elsewhere no element is stamped and the calls are not generated.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use moqui_lib::node::MNode;

/// The attribute a stamped element carries in its definition.
pub const ATTRIBUTE: &str = "mq-src";

/// The widget elements stamped: those whose macro writes an element of its own (a section writes none, so its first
/// child's tag would carry it).
pub const STAMPED: &[&str] = &[
    "container",
    "container-box",
    "container-row",
    "row-col",
    "container-panel",
    "container-split",
    "container-dialog",
    "dynamic-container",
    "dynamic-dialog",
    "button-menu",
    "tree",
    "link",
    "image",
    "label",
    "editable",
    "report-panel",
    "report-dashboard",
    "report-embed",
    "dashboard",
    "number-card",
    "chatter",
    "record-files",
    "html-view",
    "markdown-view",
    "code-view",
    "status-bar",
    "subscreens-panel",
    "subscreens-menu",
    "form-single",
    "form-list",
    "field",
];

thread_local! {
    static STAMPING: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `body` as a render that writes the stamps.
pub fn with<T>(body: impl FnOnce() -> T) -> T {
    let _restore = Restore(STAMPING.with(|on| on.replace(true)));
    body()
}

/// Puts back the stamping held before, and drops a pending stamp, however the body ends: a render that panics on a
/// pooled thread leaves no stamping on for the next request there.
struct Restore(bool);

impl Drop for Restore {
    fn drop(&mut self) {
        STAMPING.with(|on| on.set(self.0));
        PENDING.with(|pending| pending.borrow_mut().take());
    }
}

/// Run `body` writing no stamps, in a stamped render: what it writes is kept for other renders (a server-static
/// template).
pub fn paused<T>(body: impl FnOnce() -> T) -> T {
    struct Resume(bool);
    impl Drop for Resume {
        fn drop(&mut self) {
            STAMPING.with(|on| on.set(self.0));
        }
    }
    let _resume = Resume(STAMPING.with(|on| on.replace(false)));
    body()
}

/// Whether the render on this thread writes the stamps.
#[must_use]
pub fn active() -> bool {
    STAMPING.with(Cell::get)
}

/// The stamp for the next tag written, or none.
pub fn set_pending(stamp: Option<String>) {
    if active() {
        PENDING.with(|pending| *pending.borrow_mut() = stamp);
    }
}

/// `text` with the pending stamp on its first opening tag (not a `template`, which renders no element), which takes it.
pub fn inject(text: &str) -> Cow<'_, str> {
    if !active() {
        return Cow::Borrowed(text);
    }
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let Some(stamp) = pending.as_ref() else {
            return Cow::Borrowed(text);
        };
        let bytes = text.as_bytes();
        let mut at = 0;
        while let Some(found) = text[at..].find('<') {
            let start = at + found + 1;
            let name_end = text[start..]
                .find(|ch: char| !is_tag_name_char(ch))
                .map_or(text.len(), |end| start + end);
            if start < bytes.len()
                && bytes[start].is_ascii_alphabetic()
                && name_end > start
                && &text[start..name_end] != "template"
            {
                let escaped = stamp.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
                let mut out = String::with_capacity(text.len() + escaped.len() + 14);
                out.push_str(&text[..name_end]);
                out.push_str(" data-m-src=\"");
                out.push_str(&escaped);
                out.push('"');
                out.push_str(&text[name_end..]);
                *pending = None;
                return Cow::Owned(out);
            }
            at = start;
        }
        Cow::Borrowed(text)
    })
}

/// Stamp the widget elements under `root` (the screen's `widgets`, a form or section from a `screen-extend`) with the
/// file, line and column each was read from.
pub fn stamp_tree(root: &MNode) {
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        let name = node.name();
        if STAMPED.contains(&name.as_str()) && node.attribute(ATTRIBUTE).is_none() {
            if let Some(location) = node.file_location() {
                node.set_attribute(ATTRIBUTE, format!("{location}:{}:{}", node.line(), node.column()));
            }
        }
        stack.extend(node.children());
    }
}

/// The elements of a `.qvue` file's `<template>` block, each the byte offset just past its tag name and its line and
/// column (from 1, the column in characters): what [`stamp_qvue`] stamps and the Design view of a `.qvue` edits
/// (`pure.vueElements`, the same scan, held to it by `crates/moqui-lib/testdata/qvue-element-vectors.json`). The outer
/// `template` itself is not one, nor a `template` or `slot` inside, which render no element of their own. Comments,
/// quoted attribute values and `{{ }}` interpolations are passed over, so a `<` inside them starts no element; the block
/// starts at the first `<template` tag outside a comment and ends at the last `</template>` before a `<script` or
/// `<style` tag (not `<script-editor>`).
#[must_use]
pub fn qvue_elements(text: &str) -> Vec<(usize, u32, u32)> {
    let bytes = text.as_bytes();
    let Some(open) = qvue_tag(text, 0, "<template") else {
        return Vec::new();
    };
    let Some(body_start) = text[open..].find('>').map(|end| open + end + 1) else {
        return Vec::new();
    };
    // the block's end: the last </template> before the <script> or <style> block, if any
    let tail = ["<script", "<style"]
        .iter()
        .filter_map(|tag| qvue_tag(text, body_start, tag))
        .min()
        .unwrap_or(text.len());
    let body_end = text[..tail].rfind("</template>").unwrap_or(tail);
    // the line and column of `at`, counted on from the last element's
    let (mut line, mut column, mut counted) = (1_u32, 1_u32, 0_usize);
    let mut position = |at: usize| {
        for ch in text[counted..at].chars() {
            if ch == '\n' {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(1);
            }
        }
        counted = at;
        (line, column)
    };
    let mut out = Vec::new();
    let mut at = body_start;
    while at < body_end {
        if text[at..].starts_with("<!--") {
            at = text[at..].find("-->").map_or(body_end, |end| at + end + 3);
            continue;
        }
        if text[at..].starts_with("{{") {
            at = text[at..].find("}}").map_or(body_end, |end| at + end + 2);
            continue;
        }
        if bytes[at] == b'<' && at + 1 < body_end && bytes[at + 1].is_ascii_alphabetic() {
            let name_start = at + 1;
            let name_end = text[name_start..]
                .find(|ch: char| !is_tag_name_char(ch))
                .map_or(body_end, |end| name_start + end);
            let name = &text[name_start..name_end];
            if name != "template" && name != "slot" {
                let (line, column) = position(at);
                out.push((name_end, line, column));
            }
            // past the tag, its quoted values passed over
            let mut quote = None;
            at = name_end;
            while at < body_end {
                let byte = bytes[at];
                match quote {
                    Some(open) if byte == open => quote = None,
                    Some(_) => {}
                    None if byte == b'"' || byte == b'\'' => quote = Some(byte),
                    None if byte == b'>' => break,
                    None => {}
                }
                at += 1;
            }
            at += 1;
            continue;
        }
        at += text[at..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// The offset of the first `tag` (`<template`, `<script`) at or after `from` that is a tag of that name, followed by
/// white space, `>` or `/`, and not inside a comment.
fn qvue_tag(text: &str, from: usize, tag: &str) -> Option<usize> {
    let mut at = from;
    while at < text.len() {
        let comment = text[at..].find("<!--").map(|found| at + found);
        let found = text[at..].find(tag).map(|found| at + found)?;
        if let Some(comment) = comment.filter(|comment| *comment < found) {
            at = text[comment..].find("-->").map_or(text.len(), |end| comment + end + 3);
            continue;
        }
        let next = text[found + tag.len()..].chars().next();
        if next.is_none_or(|ch| ch.is_whitespace() || ch == '>' || ch == '/') {
            return Some(found);
        }
        at = found + tag.len();
    }
    None
}

/// The characters of an element's name after its first, in a template or an HTML render: Vue's components take `-`,
/// `.` and `:` (`q-btn`, `v-slot:x`), and `_`.
pub(crate) fn is_tag_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == ':' || ch == '.' || ch == '_'
}

/// A `.qvue` file's text with each element of its template stamped `data-m-src="<file>:<line>:<column>"`, for a
/// stamped render (F10): the screen builder's canvas maps what the template draws back to its lines, as it does an
/// XML screen's widgets. A component that passes its attributes to its root element shows the stamp there.
#[must_use]
pub fn stamp_qvue(text: &str, file: &str) -> String {
    let elements = qvue_elements(text);
    if elements.is_empty() {
        return text.to_string();
    }
    let file = file.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let mut out = String::with_capacity(text.len() + elements.len() * (file.len() + 24));
    let mut from = 0;
    for (at, line, column) in elements {
        out.push_str(&text[from..at]);
        out.push_str(&format!(" data-m-src=\"{file}:{line}:{column}\""));
        from = at;
    }
    out.push_str(&text[from..]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stamped_render_that_panics_leaves_its_thread_unstamped() {
        let caught = std::panic::catch_unwind(|| {
            super::with(|| {
                super::set_pending(Some("component://a/screen/A.xml:1:1".into()));
                panic!("in the render");
            })
        });
        assert!(caught.is_err());
        assert!(!super::active());
        super::PENDING.with(|pending| assert!(pending.borrow().is_none()));
    }

    /// The vectors the Design view's `pure.vueElements` is held to as well (`DevAideQvue.page.js`).
    #[test]
    fn the_qvue_element_vectors() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../moqui-lib/testdata/qvue-element-vectors.json")).unwrap();
        for vector in vectors.as_array().unwrap() {
            let text = vector["text"].as_str().unwrap();
            let found: Vec<String> = super::qvue_elements(text)
                .into_iter()
                .map(|(name_end, line, column)| {
                    let open = text[..name_end].rfind('<').unwrap();
                    format!("{}@{line}:{column}", &text[open + 1..name_end])
                })
                .collect();
            let expected: Vec<&str> = vector["stamped"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect();
            assert_eq!(found, expected, "{}", vector["name"]);
        }
    }

    #[test]
    fn a_qvue_template_is_stamped_by_its_lines() {
        let text = "<!-- a screen -->\n<template><div class=\"row\">\n  <q-btn label=\"a > b\" @click=\"go\"/>\n  \
                    <!-- <span>not</span> -->{{ a < b }}<template v-slot:x><b>x</b></template>\n</div></template>\n\
                    <script>const x = '<p>';</script>";
        let stamped = super::stamp_qvue(text, "component://a/screen/A.qvue");
        assert!(
            stamped.contains("<div data-m-src=\"component://a/screen/A.qvue:2:11\" class=\"row\">"),
            "{stamped}"
        );
        assert!(
            stamped.contains("<q-btn data-m-src=\"component://a/screen/A.qvue:3:3\" label=\"a > b\""),
            "{stamped}"
        );
        assert!(
            stamped.contains("<b data-m-src=\"component://a/screen/A.qvue:4:58\">"),
            "{stamped}"
        );
        // the comment, the interpolation, the inner template, the script: none stamped
        assert!(
            stamped.contains("<!-- <span>not</span> -->{{ a < b }}<template v-slot:x>"),
            "{stamped}"
        );
        assert!(stamped.ends_with("<script>const x = '<p>';</script>"), "{stamped}");
        assert_eq!(super::qvue_elements("<script>x</script>"), Vec::new());
    }

    use super::*;

    #[test]
    fn the_pending_stamp_goes_on_the_first_element_tag() {
        assert_eq!(inject("<div>"), "<div>", "nothing outside a stamped render");
        with(|| {
            set_pending(Some("component://x/screen/A.xml:3:9".into()));
            assert_eq!(inject("</div>text"), "</div>text");
            assert_eq!(
                inject("<template v-slot:header><m-box id=\"a\">"),
                "<template v-slot:header><m-box data-m-src=\"component://x/screen/A.xml:3:9\" id=\"a\">"
            );
            assert_eq!(inject("<span>"), "<span>", "taken once");
            set_pending(Some("a\"b".into()));
            assert_eq!(inject("<br/>"), "<br data-m-src=\"a&quot;b\"/>");
        });
        assert!(!active());
    }

    #[test]
    fn widgets_are_stamped_with_their_file_and_line() {
        let node = MNode::parse_text(
            "component://x/screen/A.xml",
            "<widgets>\n  <container>\n    <label text=\"a\"/>\n  </container>\n  <section name=\"s\"/>\n</widgets>",
        )
        .unwrap();
        stamp_tree(&node);
        let container = node.first_named("container").unwrap();
        assert_eq!(
            container.attribute(ATTRIBUTE).as_deref(),
            Some("component://x/screen/A.xml:2:3")
        );
        assert_eq!(
            container.first_named("label").unwrap().attribute(ATTRIBUTE).as_deref(),
            Some("component://x/screen/A.xml:3:5")
        );
        assert!(node.first_named("section").unwrap().attribute(ATTRIBUTE).is_none());
    }
}
