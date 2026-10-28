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
    let before = STAMPING.with(|on| on.replace(true));
    let result = body();
    STAMPING.with(|on| on.set(before));
    PENDING.with(|pending| pending.borrow_mut().take());
    result
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
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == ':' || ch == '.'))
                .map_or(text.len(), |end| start + end);
            if start < bytes.len() && bytes[start].is_ascii_alphabetic() && name_end > start {
                if &text[start..name_end] != "template" {
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

#[cfg(test)]
mod tests {
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
