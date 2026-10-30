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

//! The XML of a DAV request body with its namespaces resolved, over the framework's own reader (`moqui_lib::node`, which
//! keeps a prefixed name as written and refuses a DOCTYPE, so no entity of a request is ever expanded), and the
//! escaping of what the server writes.

use moqui_lib::node::MNode;

pub(crate) const DAV: &str = "DAV:";
pub(crate) const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub(crate) const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
/// Apple's CalendarServer extensions: `getctag`.
pub(crate) const CS: &str = "http://calendarserver.org/ns/";
/// Apple's iCal extensions: `calendar-color`, `calendar-order`.
pub(crate) const ICAL: &str = "http://apple.com/ns/ical/";

/// An element of a request body: its namespace and local name, attributes as written, text, children.
#[derive(Clone, Debug, Default)]
pub(crate) struct Element {
    pub ns: String,
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub(crate) fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    pub(crate) fn child(&self, ns: &str, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.is(ns, name))
    }

    pub(crate) fn children_named<'a>(&'a self, ns: &'a str, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |child| child.is(ns, name))
    }

    pub(crate) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Parse a request body. `None` for an empty body (PROPFIND's allprop).
pub(crate) fn parse(body: &[u8]) -> Result<Option<Element>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "the request body is not UTF-8".to_string())?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let root =
        MNode::parse_text_raw("dav-request", text).map_err(|err| format!("the request body is not XML: {err}"))?;
    Ok(Some(resolve(&root, &[])))
}

/// `node` with its names resolved against the prefixes in scope: `scope` holds (prefix, namespace) pairs, the innermost
/// last; the empty prefix is the default namespace.
fn resolve(node: &MNode, scope: &[(String, String)]) -> Element {
    let mut inner: Vec<(String, String)> = scope.to_vec();
    let mut attributes = Vec::new();
    for (key, value) in node.attributes() {
        if key == "xmlns" {
            inner.push((String::new(), value));
        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
            inner.push((prefix.to_string(), value));
        } else {
            attributes.push((key, value));
        }
    }
    let name = node.name();
    let (prefix, local) = name.split_once(':').unwrap_or(("", name.as_str()));
    let ns = inner
        .iter()
        .rev()
        .find(|(known, _)| known == prefix)
        .map(|(_, ns)| ns.clone())
        .unwrap_or_default();
    Element {
        ns,
        name: local.to_string(),
        attributes,
        text: node.text().unwrap_or_default(),
        children: node.children().iter().map(|child| resolve(child, &inner)).collect(),
    }
}

/// Text escaped for an element's content or an attribute.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // characters XML 1.0 cannot carry are dropped, as a calendar text could hold one
            ch if (ch as u32) < 0x20 && !matches!(ch, '\t' | '\n' | '\r') => {}
            ch => out.push(ch),
        }
    }
    out
}

/// The prefix the server writes a namespace with.
pub(crate) fn prefix_of(ns: &str) -> Option<&'static str> {
    match ns {
        DAV => Some("D"),
        CALDAV => Some("C"),
        CARDDAV => Some("CR"),
        CS => Some("CS"),
        ICAL => Some("I"),
        _ => None,
    }
}

/// The namespace declarations of a response's root element.
pub(crate) const ROOT_NAMESPACES: &str = concat!(
    r#"xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CR="urn:ietf:params:xml:ns:carddav""#,
    r#" xmlns:CS="http://calendarserver.org/ns/" xmlns:I="http://apple.com/ns/ical/""#
);

/// An empty element named by `ns` and `name`, with the namespace declared on it when the server has no prefix of its own
/// for it (a dead property a client asked for).
pub(crate) fn empty_element(ns: &str, name: &str) -> String {
    match prefix_of(ns) {
        Some(prefix) => format!("<{prefix}:{name}/>"),
        None if ns.is_empty() => format!("<{name}/>"),
        None => format!(r#"<X:{name} xmlns:X="{}"/>"#, escape(ns)),
    }
}

/// An element with `inner` (already XML) inside.
pub(crate) fn element(ns: &str, name: &str, inner: &str) -> String {
    match prefix_of(ns) {
        Some(prefix) => format!("<{prefix}:{name}>{inner}</{prefix}:{name}>"),
        None if ns.is_empty() => format!("<{name}>{inner}</{name}>"),
        None => format!(r#"<X:{name} xmlns:X="{}">{inner}</X:{name}>"#, escape(ns)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_and_the_default_namespace_resolve() {
        let body = br#"<?xml version="1.0"?>
            <propfind xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><prop><getetag/><c:calendar-data/>
            <x:color xmlns:x="http://apple.com/ns/ical/"/></prop></propfind>"#;
        let root = parse(body).unwrap().unwrap();
        assert!(root.is(DAV, "propfind"));
        let prop = root.child(DAV, "prop").unwrap();
        let names: Vec<(String, String)> = prop.children.iter().map(|c| (c.ns.clone(), c.name.clone())).collect();
        assert_eq!(
            names,
            vec![
                (DAV.to_string(), "getetag".to_string()),
                (CALDAV.to_string(), "calendar-data".to_string()),
                (ICAL.to_string(), "color".to_string())
            ]
        );
        assert!(parse(b"  ").unwrap().is_none());
        assert!(parse(b"<!DOCTYPE x [<!ENTITY a 'b'>]><x>&a;</x>").is_err());
    }

    #[test]
    fn written_text_is_escaped_and_unknown_namespaces_declared() {
        assert_eq!(escape("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
        assert_eq!(empty_element(DAV, "getetag"), "<D:getetag/>");
        assert_eq!(empty_element("urn:x", "y"), r#"<X:y xmlns:X="urn:x"/>"#);
        assert_eq!(element(CS, "getctag", "1"), "<CS:getctag>1</CS:getctag>");
    }
}
