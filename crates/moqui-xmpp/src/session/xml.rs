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

//! Small helpers to build and read elements.

use minidom::Element;

use crate::ns;

/// An element being built.
pub struct E(Element);

impl E {
    pub fn new(name: &str, namespace: &str) -> E {
        E(Element::bare(name, namespace))
    }

    /// An attribute; an empty value is left out.
    pub fn a(mut self, name: &str, value: impl Into<String>) -> E {
        set_attr(&mut self.0, name, value);
        self
    }

    /// An attribute when there is a value.
    pub fn ao(self, name: &str, value: Option<impl Into<String>>) -> E {
        match value {
            Some(value) => self.a(name, value),
            None => self,
        }
    }

    pub fn c(mut self, child: impl Into<Element>) -> E {
        self.0.append_child(child.into());
        self
    }

    pub fn cs(mut self, children: impl IntoIterator<Item = Element>) -> E {
        for child in children {
            self.0.append_child(child);
        }
        self
    }

    pub fn t(mut self, text: impl Into<String>) -> E {
        self.0.append_text(text.into());
        self
    }

    pub fn build(self) -> Element {
        self.0
    }
}

impl From<E> for Element {
    fn from(built: E) -> Element {
        built.0
    }
}

pub fn set_attr(element: &mut Element, name: &str, value: impl Into<String>) {
    let value = value.into();
    if value.is_empty() {
        return;
    }
    if let Ok(name) = rxml::NcName::try_from(name) {
        element.set_attr(rxml::Namespace::NONE, name, value);
    }
}

pub fn attr(element: &Element, name: &str) -> String {
    element.attr(name).unwrap_or("").to_string()
}

/// A child's text, or empty.
pub fn child_text(element: &Element, name: &str, namespace: &str) -> String {
    element
        .get_child(name, namespace)
        .map(Element::text)
        .unwrap_or_default()
}

/// An element with one text child.
pub fn text_el(name: &str, namespace: &str, text: impl Into<String>) -> Element {
    E::new(name, namespace).t(text).build()
}

/// The empty result of an iq.
pub fn iq_result(iq: &Element) -> E {
    E::new("iq", ns::CLIENT)
        .a("type", "result")
        .a("id", attr(iq, "id"))
        .a("to", attr(iq, "from"))
        .a("from", attr(iq, "to"))
}

/// The error of a stanza: the stanza turned around, type error, with its condition (RFC 6120 §8.3).
pub fn stanza_error(stanza: &Element, kind: &str, condition: &str, text: Option<&str>) -> Element {
    let mut error = E::new("error", ns::CLIENT)
        .a("type", kind)
        .c(E::new(condition, ns::STANZAS));
    if let Some(text) = text {
        error = error.c(text_el("text", ns::STANZAS, text));
    }
    E::new(stanza.name(), ns::CLIENT)
        .a("type", "error")
        .a("id", attr(stanza, "id"))
        .a("to", attr(stanza, "from"))
        .a("from", attr(stanza, "to"))
        .c(error)
        .build()
}

/// A stream error (RFC 6120 §4.9).
pub fn stream_error(condition: &str, text: Option<&str>) -> Element {
    let mut error = E::new("error", ns::STREAM).c(E::new(condition, ns::STREAMS_ERROR));
    if let Some(text) = text {
        error = error.c(text_el("text", ns::STREAMS_ERROR, text));
    }
    error.build()
}

/// A data form's field values by var (XEP-0004).
pub fn form_fields(form: &Element) -> Vec<(String, Vec<String>)> {
    form.children()
        .filter(|field| field.is("field", ns::DATA))
        .map(|field| {
            let values = field
                .children()
                .filter(|value| value.is("value", ns::DATA))
                .map(Element::text)
                .collect();
            (attr(field, "var"), values)
        })
        .collect()
}

/// A form of hidden FORM_TYPE and fields (`result` type).
pub fn form(kind: &str, form_type: &str, fields: &[(&str, &str)]) -> Element {
    let mut built = E::new("x", ns::DATA).a("type", kind).c(E::new("field", ns::DATA)
        .a("var", "FORM_TYPE")
        .a("type", "hidden")
        .c(text_el("value", ns::DATA, form_type)));
    for (var, value) in fields {
        built = built.c(E::new("field", ns::DATA)
            .a("var", *var)
            .c(text_el("value", ns::DATA, *value)));
    }
    built.build()
}

/// A timestamp in XEP-0082 form from Unix milliseconds.
pub fn stamp(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis)
        .map(|at| at.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

/// Unix milliseconds of an XEP-0082 timestamp.
pub fn parse_stamp(text: &str) -> Option<i64> {
    text.trim()
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|at| at.as_millisecond())
}

/// A random id.
pub fn random_id() -> String {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes).expect("the system random source works");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
