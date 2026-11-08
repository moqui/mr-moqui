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
//! The XML tree the macro walker shared by XML actions and screens walks (`oxide::walker`), as JSON text.
//!
//! A macro module is a JavaScript function of the support object and of the modules loaded before it. The walker
//! visits an XML tree in post-order and calls `macros[name](node, children)` for each element. `node` is
//! `{name, attrs, text, children, parent, source}` and `children` is the already generated text of the child elements.
//! The macros of an opaque element's descendants are not called, but their node objects are built so the opaque
//! element's macro can read them.

use std::collections::HashSet;

use moqui_lib::node::MNode;

use crate::context::Error;

/// Replaces an element before it is walked, such as a form by its assembled form.
pub(crate) type Resolve<'a> = &'a dyn Fn(&MNode) -> Result<Option<MNode>, Error>;

/// An XML tree as JSON text for the macro walker, which builds its node objects in JavaScript (`oxide::walker`): each
/// element is `{name, text, attrs, src?, inert, children}`, with `resolve` applied to the elements that are not inert
/// (the descendants of an `opaque` element), and attributes in document order.
///
/// # Errors
///
/// Returns the error of `resolve`.
pub(crate) fn tree_json(node: &MNode, opaque: &HashSet<String>, resolve: Option<Resolve<'_>>) -> Result<String, Error> {
    fn text(out: &mut String, value: &str) {
        out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string()));
    }
    fn element(
        out: &mut String,
        node: &MNode,
        opaque: &HashSet<String>,
        resolve: Option<Resolve<'_>>,
        inert: bool,
    ) -> Result<(), Error> {
        let replaced = match resolve {
            Some(resolve) if !inert => resolve(node)?,
            _ => None,
        };
        let node = replaced.as_ref().unwrap_or(node);
        let name = node.name();
        out.push_str("{\"name\":");
        text(out, &name);
        out.push_str(",\"text\":");
        node.with_text(|value| text(out, value.unwrap_or_default()));
        out.push_str(",\"attrs\":{");
        let mut first = true;
        node.for_each_attribute(|key, value| {
            if key != crate::screen::stamp::ATTRIBUTE {
                if !first {
                    out.push(',');
                }
                first = false;
                text(out, key);
                out.push(':');
                text(out, value);
            }
        });
        out.push('}');
        if let Some(stamp) = node.attribute(crate::screen::stamp::ATTRIBUTE) {
            out.push_str(",\"src\":");
            text(out, &stamp);
        }
        out.push_str(if inert { ",\"inert\":true" } else { ",\"inert\":false" });
        out.push_str(",\"children\":[");
        let inert_children = inert || opaque.contains(name.as_str());
        for (index, child) in node.children().iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            element(out, child, opaque, resolve, inert_children)?;
        }
        out.push_str("]}");
        Ok(())
    }
    let mut out = String::new();
    element(&mut out, node, opaque, resolve, false)?;
    Ok(out)
}

/// The framework's own XML files, for the tests that compare the engines.
#[cfg(test)]
pub(crate) mod test_files {
    use moqui_lib::node::MNode;

    /// The XML files under the base and dev components' `dir` directories, sorted.
    pub(crate) fn component_xml_files(dir: &str) -> Vec<std::path::PathBuf> {
        fn visit(path: &std::path::Path, dir: &str, under: bool, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let under = under || path.file_name().is_some_and(|name| name == dir);
                    visit(&path, dir, under, out);
                } else if under && path.extension().is_some_and(|extension| extension == "xml") {
                    out.push(path);
                }
            }
        }
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
        let mut out = Vec::new();
        for component in ["base-component", "dev-component"] {
            visit(&root.join(component), dir, false, &mut out);
        }
        out.sort();
        out
    }

    /// Every element named `name` in `node`, in document order.
    pub(crate) fn named(node: &MNode, name: &str, out: &mut Vec<MNode>) {
        for child in node.children() {
            if child.name() == name {
                out.push(child);
            } else {
                named(&child, name, out);
            }
        }
    }
}
