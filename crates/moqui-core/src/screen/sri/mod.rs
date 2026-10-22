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

//! `sri`: what the generated screen code calls while a screen renders. The functions here read the
//! [`RenderState`] and the context stack, and run the generated code of a section through a
//! [`Runner`]. The JavaScript bindings are in `script::runtime`.
//!
//! Deferred to the html slices because only their macros call them: `getFieldTransitionValue`
//! (calls a transition as a test render), `makeFormListSingleMap`,
//! `makeFormListMultiMap`, `getFormListHeaderValues`, `getFormFieldValues`, form-list aggregation
//! (`group-by`, `sub-list`, `show-total`), and saved finds.

mod fields;
mod forms;
mod layout;
mod menu_items;
mod options;
mod sections;
mod theme;

pub(in crate::screen) use sections::strip_controls;

use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::definition::ScreenDef;
use super::form::FormInstance;
use super::generate::{Unit, UnitRequest};
use super::render::RenderState;
use crate::context::Error;
use crate::web;

pub use fields::{
    context_value, field_validation, field_validation_rules, get_field_entity_value, get_field_value,
    get_field_value_class, get_field_value_plain_string, get_field_value_string, get_field_value_string_for,
    get_named_value_plain, node_parameters, set_in_context,
};
pub use forms::{
    close_form_list, form_list_find_screen_scheduled, form_list_sub_row_count, get_form, open_form_list,
    push_single_form_map_context, render_dynamic_form, render_user_columns_form, saved_find_full_location,
    start_form_list_row, start_form_list_sub_row, user_default_form_list_find_id, user_form_list_finds,
};
pub(super) use forms::{form_list_row_frame, form_list_sub_row_frame, single_form_map_frame};
pub use layout::form_list_column_char_widths;
pub use menu_items::menu_items;
pub(super) use menu_items::screen_permitted;
pub use options::{field_transition_value, get_field_options};
pub use sections::{
    append_to_after_screen_writer, append_to_script_writer, render_active_screen, render_include_screen,
    render_section, render_section_include, render_subscreen, render_text, render_text_value,
};
pub(super) use sections::{is_security, iteration_entries, paginate_items};
pub use theme::{get_theme_icon_class, get_theme_values};
pub(crate) use theme::{theme_resource_values, user_theme_id, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};

/// The first focus id wins. A tui form macro calls this with `form.field` from the form's `focus-field`.
pub fn set_focus_field(state: &RenderState, id: &str) {
    let mut slot = state.focus.borrow_mut();
    if !slot.is_empty() {
        return;
    }
    let clean = strip_controls(id);
    if !clean.is_empty() {
        *slot = clean;
    }
}

/// A node id that no earlier node of this document has: `id` the first time, then `id~2`, `id~3`. The same screen
/// rendered again gives the same ids, so the tui client keeps focus and edits across reloads, while a section that
/// is iterated, included twice, or named like another section does not repeat an id.
pub fn unique_id(state: &RenderState, id: &str) -> String {
    let mut ids = state.node_ids.borrow_mut();
    let (given, next) = &mut *ids;
    if given.insert(id.to_string()) {
        return id.to_string();
    }
    let suffix = next.entry(id.to_string()).or_insert(2);
    loop {
        let candidate = format!("{id}~{suffix}");
        *suffix += 1;
        if given.insert(candidate.clone()) {
            return candidate;
        }
    }
}

/// Generates and runs the code of a section on the script runtime that is running the render.
pub trait Runner {
    /// The generated code for a section, None when it has no widgets.
    ///
    /// # Errors
    ///
    /// Returns an error when the section's code cannot be generated.
    fn generate(&self, request: &UnitRequest<'_>) -> Result<Option<Arc<Unit>>, Error>;

    /// The code for a dynamic form as it was just assembled.
    ///
    /// # Errors
    ///
    /// Returns an error when the form's code cannot be generated.
    fn generate_dynamic(&self, request: &UnitRequest<'_>, form: &FormInstance) -> Result<Arc<Unit>, Error>;

    /// Run a unit on the render's runtime.
    ///
    /// # Errors
    ///
    /// Returns the error the code raised.
    fn run(&self, unit: &Unit) -> Result<(), Error>;
}

/// An element as the generated code passes it: name, attributes, text, children, and the chain of parents.
#[derive(Clone, Debug, Default)]
pub struct NodeLit {
    pub name: String,
    pub attrs: IndexMap<String, String>,
    pub text: String,
    pub children: Vec<NodeLit>,
    pub parent: Option<Box<NodeLit>>,
}

impl NodeLit {
    /// An attribute, empty when it is not there.
    pub fn attr(&self, name: &str) -> &str {
        self.attrs.get(name).map_or("", String::as_str)
    }

    /// The element two levels up, which for a widget is its field.
    pub fn field(&self) -> Option<&NodeLit> {
        self.parent.as_ref()?.parent.as_deref()
    }

    pub fn child(&self, name: &str) -> Option<&NodeLit> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The element as XML text.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write_xml(&mut out);
        out
    }

    fn write_xml(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (key, value) in &self.attrs {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            out.push_str(&moqui_lib::string::encode_for_xml_attribute(value, false));
            out.push('"');
        }
        out.push('>');
        out.push_str(
            &self
                .text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        );
        for child in &self.children {
            child.write_xml(out);
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }
}

/// The rows of a form-list being rendered: the context frame contents of each row after the row actions.
#[derive(Debug, Default)]
pub struct FormRows {
    pub rows: Vec<IndexMap<String, Value>>,
}

/// `WebUtilities.encodeHtml`.
pub fn encode_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

// ---- output ----

pub fn content_type_header(state: &RenderState) -> String {
    format!("{};charset={}", state.content_type, state.character_encoding)
}

/// Append text to the response. The first write opens it. Nothing is written once a redirect or
/// another response has stopped the render.
pub fn write(state: &RenderState, text: &str) {
    // A binary mode's document is the response; text such as an untyped `text` element has no place in it.
    if state.dont_do_render.get() || text.is_empty() || state.binary {
        return;
    }
    if !web::output_sent() {
        web::start_output(200, &content_type_header(state));
    }
    web::write_output(text);
}

/// Text added with `appendToAfterScreenWriter` and `appendToScriptWriter` that nothing else wrote.
pub fn flush_end_text(state: &RenderState) {
    let after = std::mem::take(&mut *state.after_text.borrow_mut());
    write(state, &after);
    let script_text = std::mem::take(&mut *state.script_text.borrow_mut());
    if !script_text.is_empty() {
        write(state, &format!("\n<script>\n{script_text}\n</script>\n"));
    }
}

pub fn do_boundary_comments(state: &RenderState) -> bool {
    state.screen_path_index.get() != 0 && state.facade.conf().boundary_comments
}

pub fn active_def(state: &RenderState) -> Arc<ScreenDef> {
    if let Some(def) = state.override_def.borrow().as_ref() {
        return Arc::clone(def);
    }
    Arc::clone(&state.url_info.path_defs[state.screen_path_index.get()])
}

/// The path names to the active screen, without the root.
pub fn active_screen_path(state: &RenderState) -> Vec<String> {
    let index = state.screen_path_index.get().min(state.url_info.full_path.len());
    state.url_info.full_path[..index].to_vec()
}

#[cfg(test)]
mod tests {
    use super::{encode_html, form_list_column_char_widths, paginate_items};
    use crate::script::{self, ContextFrame};
    use moqui_lib::object::Value;

    #[test]
    fn paginate_items_cuts_a_page_unless_the_list_is_already_one() {
        let items =
            |count: i32| -> Vec<(Option<String>, Value)> { (0..count).map(|n| (None, Value::I32(n))).collect() };
        let _frame = ContextFrame::push();
        script::set_context_name("pageIndex", Value::I32(1)).unwrap();
        let page = paginate_items("all", items(45)).unwrap();
        assert_eq!(page.first().map(|item| &item.1), Some(&Value::I32(20)));
        assert_eq!(page.len(), 20);
        assert_eq!(script::context_value("allPageRangeLow"), Some(Value::I32(21)));

        // page 2 of a paginated find: the count and range are of all the rows, and the list is the page
        script::set_context_name("ordersCount", Value::I32(45)).unwrap();
        script::set_context_name("ordersPageRangeLow", Value::I32(21)).unwrap();
        script::set_context_name("ordersPageRangeHigh", Value::I32(40)).unwrap();
        assert_eq!(paginate_items("orders", items(20)).unwrap().len(), 20);
        script::set_context_name("itemsAlreadyPaginated", Value::Bool(true)).unwrap();
        assert_eq!(paginate_items("items", items(7)).unwrap().len(), 7);
    }

    fn columns(specs: &[(&str, &str)]) -> Vec<Vec<(String, String)>> {
        specs
            .iter()
            .map(|(width, kind)| vec![((*width).to_string(), (*kind).to_string())])
            .collect()
    }

    #[test]
    fn equal_columns_share_the_line_and_the_remainder_goes_to_the_first() {
        // 132 - 2 separators = 130; a third of it is 43.33
        assert_eq!(
            form_list_column_char_widths(&columns(&[("", ""), ("", ""), ("", "")]), 132).unwrap(),
            [44, 43, 43]
        );
        // 5 columns: 128 shared, 25.6 each rounds to 26 and the 2 over come back from the first two
        assert_eq!(
            form_list_column_char_widths(&columns(&[("", ""); 5]), 132).unwrap(),
            [25, 25, 26, 26, 26]
        );
        // a line width of 0 is 132
        assert_eq!(form_list_column_char_widths(&columns(&[("", "")]), 0).unwrap(), [132]);
        assert!(form_list_column_char_widths(&[], 80).unwrap().is_empty());
    }

    #[test]
    fn fixed_percent_and_zero_widths() {
        // a fixed column of 20 characters, the other two split the 110 left
        assert_eq!(
            form_list_column_char_widths(&columns(&[("20", "characters"), ("", ""), ("", "")]), 132).unwrap(),
            [20, 55, 55]
        );
        // a percent width is scaled with the others to 100 percent
        assert_eq!(
            form_list_column_char_widths(&columns(&[("50", ""), ("", ""), ("", "")]), 132).unwrap(),
            [56, 37, 37]
        );
        // a zero width hides the column and gives its separator back
        assert_eq!(
            form_list_column_char_widths(&columns(&[("0", ""), ("", ""), ("", "")]), 132).unwrap(),
            [0, 65, 66]
        );
        // a percent that is fewer characters than the fixed width gives way to it
        assert_eq!(
            form_list_column_char_widths(&columns(&[("40", "characters"), ("10", "")]), 132).unwrap(),
            [40, 91]
        );
        let err =
            form_list_column_char_widths(&columns(&[("100", "characters"), ("100", "characters")]), 132).unwrap_err();
        assert!(err.message().contains("exceeded total line characters 132 by"), "{err}");
        assert!(form_list_column_char_widths(&columns(&[("x", "")]), 80).is_err());
    }

    #[test]
    fn html_is_encoded_for_text_the_page_shows() {
        assert_eq!(
            encode_html("<a href=\"x\">T&C's</a>"),
            "&lt;a href=&quot;x&quot;&gt;T&amp;C&#39;s&lt;/a&gt;"
        );
    }
}
