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

//! The screen definition: all of `xml-screen-3.xsd`, parsed once per cached screen.
//!
//! `MNode` is `Rc`, so a cached definition holds XML text and plain fields and is `Send + Sync`.
//! Widget subtrees and forms stay as [`XmlFragment`] text. [`XmlFragment::node`] parses one back
//! into an `MNode` on the calling thread, which is how the macro walker reads them.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use moqui_lib::node::MNode;

use super::cache::ScreenFacade;
use crate::context::{Error, ExecutionContextFactory};

/// Name of the context field a wrapped XML condition sets when it holds.
pub(crate) const CONDITION_FIELD: &str = "screenConditionResult";

const MAX_INCLUDE_DEPTH: usize = 8;

/// A subtree as XML text.
#[derive(Clone, Debug)]
pub struct XmlFragment {
    text: Arc<str>,
}

impl XmlFragment {
    fn of(node: &MNode) -> Self {
        Self {
            text: Arc::from(node.write_string()),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Parse the fragment on this thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not XML.
    pub fn node(&self) -> Result<MNode, Error> {
        MNode::parse_text("screen-fragment", &self.text).map_err(Error::from)
    }
}

/// `<actions>` XML that passed `script::check_actions`. Run it with `script::run_actions`.
#[derive(Clone, Debug)]
pub struct Actions {
    pub xml: Arc<str>,
}

impl Actions {
    /// The children of `node` as an `<actions>` element, whatever `node` is called.
    pub(super) fn from_node(node: &MNode, label: &str) -> Result<Self, Error> {
        let copy = node.deep_copy();
        copy.set_name("actions");
        Self::from_xml(copy.write_string(), label)
    }

    pub(super) fn from_xml(xml: String, label: &str) -> Result<Self, Error> {
        crate::script::check_actions(&xml).map_err(|err| Error::new(format!("{label}: {}", err.message())))?;
        Ok(Self { xml: Arc::from(xml) })
    }
}

/// A boolean test. An `expression` alone is a JavaScript expression. Any other XML condition
/// becomes actions that set [`CONDITION_FIELD`] when it holds.
#[derive(Clone, Debug)]
pub enum Condition {
    Expression(String),
    Wrapped(Actions),
}

impl Condition {
    fn from_attr(expression: &str, label: &str) -> Result<Self, Error> {
        crate::script::parse_js(expression)
            .map_err(|err| Error::new(format!("{label}: condition `{expression}`: {}", err.message())))?;
        Ok(Self::Expression(expression.to_string()))
    }

    /// `node` is a `condition` element.
    pub(crate) fn from_element(node: &MNode, label: &str) -> Result<Option<Self>, Error> {
        let children = node.children();
        if children.is_empty() {
            return Ok(None);
        }
        if children.len() == 1 && children[0].name() == "expression" && children[0].children().is_empty() {
            let text = children[0].text().unwrap_or_default();
            if text.trim().is_empty() {
                return Ok(None);
            }
            return Self::from_attr(text.trim(), label).map(Some);
        }
        let inner: String = children.iter().map(MNode::write_string).collect();
        let xml = format!(
            "<actions><if><condition>{inner}</condition><then><set field=\"{CONDITION_FIELD}\" from=\"true\"/></then></if></actions>"
        );
        Actions::from_xml(xml, label).map(|actions| Some(Self::Wrapped(actions)))
    }
}

/// `require-authentication`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authentication {
    Required,
    Optional,
    AnonymousView,
    AnonymousAll,
}

impl Authentication {
    fn of(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("false") => Self::Optional,
            Some("anonymous-view") => Self::AnonymousView,
            Some("anonymous-all") => Self::AnonymousAll,
            _ => Self::Required,
        }
    }
}

/// `parameter` and `path-parameter` source. `from` is an expression, `value` a string to expand.
#[derive(Clone, Debug)]
pub struct ParameterDef {
    pub name: String,
    pub required: bool,
    pub from: Option<String>,
    pub value: Option<String>,
}

impl ParameterDef {
    fn of(node: &MNode) -> Option<Self> {
        let name = node.attribute("name").filter(|text| !text.is_empty())?;
        Some(Self {
            name,
            required: node.attribute_is("required", "true"),
            from: node.attribute("from").filter(|text| !text.is_empty()),
            value: node.attribute("value").filter(|text| !text.is_empty()),
        })
    }
}

fn parameters_of(parent: &MNode) -> Vec<ParameterDef> {
    parent
        .children_named("parameter")
        .iter()
        .filter_map(ParameterDef::of)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseKind {
    None,
    Url,
    ScreenLast,
    ScreenLastNoParam,
}

/// `conditional-response`, `default-response`, or `error-response`.
#[derive(Clone, Debug)]
pub struct ResponseDef {
    pub kind: ResponseKind,
    pub url: String,
    /// `screen-path` or `plain`.
    pub url_type: String,
    pub condition: Option<Condition>,
    pub parameters: Vec<ParameterDef>,
    /// A JavaScript expression that gives a map of extra parameters.
    pub parameter_map: Option<String>,
    pub save_current_screen: bool,
    pub save_parameters: bool,
}

impl ResponseDef {
    fn of(node: &MNode, label: &str) -> Result<Self, Error> {
        let kind = match node.attribute("type").as_deref() {
            Some("none") => ResponseKind::None,
            Some("screen-last") => ResponseKind::ScreenLast,
            Some("screen-last-noparam") => ResponseKind::ScreenLastNoParam,
            _ => ResponseKind::Url,
        };
        let condition = match node.first_named("condition") {
            Some(condition) => Condition::from_element(&condition, &format!("{label} condition"))?,
            None => None,
        };
        let parameter_map = node.attribute("parameter-map").filter(|text| !text.is_empty());
        if let Some(expression) = &parameter_map {
            crate::script::parse_js(expression)
                .map_err(|err| Error::new(format!("{label}: parameter-map `{expression}`: {}", err.message())))?;
        }
        Ok(Self {
            kind,
            url: node.attribute("url").unwrap_or_default(),
            url_type: node
                .attribute("url-type")
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "screen-path".to_string()),
            condition,
            parameters: parameters_of(node),
            parameter_map,
            save_current_screen: node.attribute_is("save-current-screen", "true"),
            save_parameters: node.attribute_is("save-parameters", "true"),
        })
    }

    fn none() -> Self {
        Self {
            kind: ResponseKind::None,
            url: String::new(),
            url_type: "screen-path".to_string(),
            condition: None,
            parameters: Vec::new(),
            parameter_map: None,
            save_current_screen: false,
            save_parameters: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TransitionDef {
    pub name: String,
    /// Lowercase HTTP method, or `any`.
    pub method: String,
    /// Location of the screen the transition runs under. A `transition-include` takes the includer.
    pub screen_location: String,
    pub begin_transaction: bool,
    pub read_only: bool,
    /// `None` uses the webapp default.
    pub require_session_token: Option<bool>,
    pub authz_action: String,
    /// The transition names its `authz-action`; else a single service's action is used when it runs.
    pub authz_explicit: bool,
    pub parameters: Vec<ParameterDef>,
    pub path_parameters: Vec<String>,
    pub condition: Option<Condition>,
    /// The `service-call` shortcut, compiled on its own. Its script returns when the call leaves an
    /// error, and that return must not skip [`Self::actions`].
    pub service_actions: Option<Actions>,
    /// The transition's `<actions>`. They run even when there is already an error.
    pub actions: Option<Actions>,
    /// Set when either part did not compile. The screen still loads, and running the transition
    /// fails with this message before either part runs.
    pub actions_error: Option<String>,
    pub single_service: Option<String>,
    pub conditional_responses: Vec<ResponseDef>,
    pub default_response: ResponseDef,
    pub error_response: Option<ResponseDef>,
    /// The automatic transitions every screen has, as Java's `ScreenDefinition`: `actions` (the data of a server-static
    /// screen as JSON), `formSelectColumns`, `formSaveFind` and `screenDoc`. A screen's own transition of the name wins.
    pub implicit: Implicit,
}

/// Which automatic transition a [`TransitionDef`] is, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Implicit {
    None,
    Actions,
    FormSelectColumns,
    FormSaveFind,
    ScreenDoc,
}

/// The children as one `<actions>` script. An empty list is still a script: an `<actions>` element
/// with nothing in it is a transition that has actions.
fn compile_actions(children: Vec<MNode>, label: &str) -> Result<Actions, String> {
    let wrapper = MNode::new("actions");
    for child in children {
        wrapper.append_node(child);
    }
    Actions::from_xml(wrapper.write_string(), label).map_err(|err| err.message().to_string())
}

impl TransitionDef {
    /// The automatic transitions: `ActionsTransitionItem` runs the screen's data preparation on demand, and the
    /// saved find, column and screen document ones are `ScreenDefinition`'s other items. The saved find and column
    /// ones write rows, so they take only a POST with the session token (Java takes any method and no token).
    fn implicit(screen_location: &str, implicit: Implicit) -> Self {
        let (name, read_only, begin_transaction) = match implicit {
            Implicit::FormSelectColumns => ("formSelectColumns", false, true),
            Implicit::FormSaveFind => ("formSaveFind", false, true),
            Implicit::ScreenDoc => ("screenDoc", true, false),
            Implicit::Actions | Implicit::None => ("actions", true, true),
        };
        Self {
            name: name.to_string(),
            method: if read_only { "any" } else { "post" }.to_string(),
            screen_location: screen_location.to_string(),
            begin_transaction,
            read_only,
            require_session_token: Some(!read_only),
            authz_action: "view".to_string(),
            authz_explicit: true,
            parameters: Vec::new(),
            path_parameters: Vec::new(),
            condition: None,
            service_actions: None,
            actions: None,
            actions_error: None,
            single_service: None,
            conditional_responses: Vec::new(),
            default_response: ResponseDef::none(),
            error_response: None,
            implicit,
        }
    }

    /// Names of the `parameter` children marked required.
    pub fn required_parameters(&self) -> Vec<&str> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.required)
            .map(|parameter| parameter.name.as_str())
            .collect()
    }

    fn of(node: &MNode, screen_location: &str) -> Result<Self, Error> {
        let name = node.attribute("name").unwrap_or_default();
        let label = format!("{screen_location} transition {name}");
        let method = node
            .attribute("method")
            .map(|text| text.to_ascii_lowercase())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "any".to_string());
        let mut single_service = None;
        let mut service_actions = None;
        let mut actions = None;
        let mut actions_error = None;
        let mut has_actions = false;
        // The service-call shortcut defaults in-map, out-map, and multi as the Java screen does.
        // It is compiled apart from `<actions>`: the generated call returns on an error, and that
        // return would skip the actions if they were one script (ScreenDefinition.groovy).
        if let Some(call) = node.first_named("service-call") {
            has_actions = true;
            let call = call.deep_copy();
            for (key, value) in [("in-map", "true"), ("out-map", "context")] {
                if call.attribute(key).is_none_or(|text| text.is_empty()) {
                    call.set_attribute(key, value);
                }
            }
            if call.attribute("multi").is_none_or(|text| text.is_empty()) && !call.attribute_is("async", "true") {
                call.set_attribute("multi", "parameter");
            }
            single_service = call.attribute("name").filter(|text| !text.is_empty());
            match compile_actions(vec![call], &label) {
                Ok(compiled) => service_actions = Some(compiled),
                Err(message) => actions_error = Some(message),
            }
        }
        if let Some(actions_node) = node.first_named("actions") {
            has_actions = true;
            if actions_error.is_none() {
                let children = actions_node.children().iter().map(MNode::deep_copy).collect();
                match compile_actions(children, &label) {
                    Ok(compiled) => actions = Some(compiled),
                    Err(message) => {
                        actions_error = Some(message);
                        service_actions = None;
                    }
                }
            }
        }
        let condition = match node.first_named("condition") {
            Some(condition) => Condition::from_element(&condition, &format!("{label} condition"))?,
            None => None,
        };
        let read_only = !has_actions || node.attribute_is("read-only", "true");
        let authz_explicit = node.attribute("authz-action").is_some_and(|text| !text.is_empty());
        let authz_action = match node.attribute("authz-action").filter(|text| !text.is_empty()) {
            Some(action) => action,
            None if read_only => "view".to_string(),
            None => "update".to_string(),
        };
        let conditional_responses = node
            .children_named("conditional-response")
            .iter()
            .map(|response| ResponseDef::of(response, &format!("{label} conditional-response")))
            .collect::<Result<Vec<_>, _>>()?;
        let default_response = match node.first_named("default-response") {
            Some(response) => ResponseDef::of(&response, &format!("{label} default-response"))?,
            None => ResponseDef::none(),
        };
        let error_response = match node.first_named("error-response") {
            Some(response) => Some(ResponseDef::of(&response, &format!("{label} error-response"))?),
            None => None,
        };
        Ok(Self {
            name,
            method,
            screen_location: screen_location.to_string(),
            begin_transaction: !node.attribute_is("begin-transaction", "false"),
            read_only,
            require_session_token: flag(node.attribute("require-session-token")),
            authz_action,
            authz_explicit,
            parameters: parameters_of(node),
            path_parameters: node
                .children_named("path-parameter")
                .iter()
                .filter_map(|parameter| parameter.attribute("name"))
                .filter(|text| !text.is_empty())
                .collect(),
            condition,
            service_actions,
            actions,
            actions_error,
            single_service,
            conditional_responses,
            default_response,
            error_response,
            implicit: Implicit::None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SubscreensItem {
    pub name: String,
    pub location: String,
    pub menu_title: String,
    pub menu_index: Option<i64>,
    pub menu_include: bool,
    pub no_sub_path: bool,
    /// A JavaScript expression run when the menu is built.
    pub disable_when: Option<String>,
    /// From a `SubscreensItem` row: only members of this group see the item.
    pub user_group_id: Option<String>,
}

impl SubscreensItem {
    /// `isValidInCurrentContext`: an item limited to a user group is there only for its members.
    pub fn is_valid_in_current_context(&self) -> bool {
        match self.user_group_id.as_deref() {
            None | Some("") | Some("ALL_USERS") => true,
            Some(group) => crate::user::current_user_groups().is_ok_and(|groups| groups.contains(group)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Subscreens {
    pub default_item: Option<String>,
    pub always_use_full_path: bool,
    /// `(condition expression, item name)` in document order.
    pub conditional_defaults: Vec<(String, String)>,
    /// In the order the sources add them. A later source replaces an item of the same name.
    pub items: Vec<SubscreensItem>,
    /// Indexes of `items` in menu order, sorted once.
    order: std::sync::OnceLock<Vec<usize>>,
}

impl Subscreens {
    pub fn get(&self, name: &str) -> Option<&SubscreensItem> {
        self.items.iter().find(|item| item.name == name)
    }

    /// Menu order: items with an index first by index, then by upper-case title.
    pub fn sorted(&self) -> Vec<&SubscreensItem> {
        let order = self.order.get_or_init(|| {
            let titles: Vec<String> = self.items.iter().map(|item| item.menu_title.to_uppercase()).collect();
            let mut order: Vec<usize> = (0..self.items.len()).collect();
            order.sort_by(|&left, &right| {
                let by_index = match (self.items[left].menu_index, self.items[right].menu_index) {
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(a), Some(b)) => a.cmp(&b),
                    (None, None) => std::cmp::Ordering::Equal,
                };
                by_index.then_with(|| titles[left].cmp(&titles[right]))
            });
            order
        });
        order.iter().map(|&index| &self.items[index]).collect()
    }

    fn put(&mut self, item: SubscreensItem) {
        self.order = std::sync::OnceLock::new();
        match self.items.iter_mut().find(|existing| existing.name == item.name) {
            Some(existing) => *existing = item,
            None => self.items.push(item),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SectionKind {
    Section,
    Iterate {
        list: String,
        entry: String,
        key: Option<String>,
        paginate: bool,
    },
    /// A `section-include` whose name or location holds `${`, so it is resolved when rendered.
    Include {
        location: String,
        name: String,
    },
}

/// The screen's own root section, a `section`, or a `section-iterate`.
#[derive(Clone, Debug)]
pub struct SectionDef {
    pub name: String,
    pub kind: SectionKind,
    pub condition: Option<Condition>,
    pub actions: Option<Actions>,
    pub widgets: Option<XmlFragment>,
    pub fail_widgets: Option<XmlFragment>,
}

impl SectionDef {
    fn of(node: &MNode, name: &str, label: &str) -> Result<Self, Error> {
        let kind = match node.name().as_str() {
            "section-iterate" => SectionKind::Iterate {
                list: node.attribute("list").unwrap_or_default(),
                entry: node.attribute("entry").unwrap_or_default(),
                key: node.attribute("key").filter(|text| !text.is_empty()),
                paginate: node.attribute_is("paginate", "true"),
            },
            _ => SectionKind::Section,
        };
        let mut condition = match node.attribute("condition").filter(|text| !text.is_empty()) {
            Some(expression) => Some(Condition::from_attr(&expression, label)?),
            None => None,
        };
        if condition.is_none() {
            if let Some(element) = node.first_named("condition") {
                condition = Condition::from_element(&element, label)?;
            }
        }
        let actions = match node.first_named("actions") {
            Some(actions) => Some(Actions::from_node(&actions, &format!("{label} actions"))?),
            None => None,
        };
        Ok(Self {
            name: name.to_string(),
            kind,
            condition,
            actions,
            widgets: node.first_named("widgets").as_ref().map(XmlFragment::of),
            fail_widgets: node.first_named("fail-widgets").as_ref().map(XmlFragment::of),
        })
    }
}

#[derive(Clone, Debug)]
pub struct FormDef {
    pub name: String,
    pub list: bool,
    /// `dynamic="true"`: assembled from the context each time it renders.
    pub dynamic: bool,
    pub node: XmlFragment,
    /// Same-named forms from `screen-extend` files, in the order they were found.
    pub extensions: Vec<XmlFragment>,
}

#[derive(Clone, Debug)]
pub struct TreeDef {
    pub name: String,
    pub node: XmlFragment,
}

#[derive(Clone, Debug, Default)]
pub struct WebSettings {
    pub allow_web_request: bool,
    pub require_encryption: bool,
    pub mime_type: Option<String>,
    pub character_encoding: Option<String>,
}

/// A file a definition was read from, to check whether the cached copy is still current.
#[derive(Clone, Debug)]
pub struct Source {
    pub location: String,
    pub modified: Option<i64>,
}

impl Source {
    /// The location with its modified time now. A directory counts, so a file added to it is seen.
    pub(super) fn of(factory: &ExecutionContextFactory, location: &str) -> Self {
        let modified = factory
            .resource()
            .get(location)
            .ok()
            .filter(|resource| resource.exists())
            .and_then(|resource| resource.last_modified());
        Self {
            location: location.to_string(),
            modified,
        }
    }

    /// Whether the location still has the modified time it had when read.
    pub(super) fn is_current(&self, factory: &ExecutionContextFactory) -> bool {
        match factory.resource().get(&self.location) {
            Ok(resource) => resource.exists() && resource.last_modified() == self.modified,
            Err(_) => false,
        }
    }
}

#[derive(Debug)]
pub struct ScreenDef {
    pub location: String,
    /// The file name up to the first dot.
    pub screen_name: String,
    pub authentication: Authentication,
    pub standalone: bool,
    pub allow_extra_path: bool,
    pub begin_transaction: bool,
    pub transaction_timeout: Option<i64>,
    pub include_child_content: bool,
    pub history: bool,
    pub track_artifact_hit: bool,
    pub login_path: Option<String>,
    pub default_menu_title: String,
    pub default_menu_index: Option<i64>,
    pub default_menu_include: bool,
    pub menu_image: Option<String>,
    pub menu_image_type: String,
    pub screen_theme_type: String,
    /// `all`, or the render modes this screen supports.
    pub render_modes: Vec<String>,
    pub server_static: Vec<String>,
    /// `server_static` is `all` because the widgets are only the subscreens placeholders, not because the screen says so.
    pub server_static_auto: bool,
    pub web: WebSettings,
    /// `macro-template` by render mode type.
    pub macro_templates: BTreeMap<String, String>,
    pub parameters: Vec<ParameterDef>,
    pub always_actions: Option<Actions>,
    pub pre_actions: Option<Actions>,
    pub transitions: Vec<TransitionDef>,
    pub subscreens: Subscreens,
    pub root: SectionDef,
    /// `section`, `section-iterate`, and resolved `section-include` by name, from the whole screen.
    pub sections: BTreeMap<String, SectionDef>,
    pub forms: BTreeMap<String, FormDef>,
    pub trees: BTreeMap<String, TreeDef>,
    pub has_tab_menu: bool,
    /// The `shell-providers` of an app root, for the shell data (`shell.rs`).
    pub shell_providers: Option<super::shell::ShellProviders>,
    /// The merged screen XML, after `screen-extend`.
    pub source: XmlFragment,
    pub sources: Vec<Source>,
    /// When the sources were last found current, in epoch milliseconds.
    pub(super) checked_at: std::sync::atomic::AtomicI64,
    /// The screen configuration data generation the `SubscreensItem` rows were read at.
    pub(super) config_generation: u64,
    /// Render code and assembled forms made while rendering. They live as long as this definition.
    pub(super) generated: super::generate::Generated,
}

#[cfg(test)]
impl ScreenDef {
    pub(super) fn generated(&self) -> &super::generate::Generated {
        &self.generated
    }
}

/// The `config_generation` of a definition whose `SubscreensItem` rows could not be read. No count reaches it, so the
/// definition is never current.
pub(super) const STALE_GENERATION: u64 = u64::MAX;

/// Whether a failed read of the screen configuration entity `name` only means this database has none: the entity is
/// not defined, or its table is not there, as in a bare test factory. Any other failure is not "no rows".
pub(super) fn screen_entity_absent(entity: &crate::entity::EntityFacade, name: &str) -> bool {
    !crate::entity::info::is_entity_defined(entity, name) || matches!(entity.table_exists(name), Ok(false))
}

/// The count of screen configuration writes of the factory's database, 0 with none installed.
pub(super) fn screen_config_generation(factory: &ExecutionContextFactory) -> u64 {
    factory
        .entity()
        .map_or(0, |facade| crate::entity::screen_config_generation(&facade))
}

impl ScreenDef {
    pub fn parameter(&self, name: &str) -> Option<&ParameterDef> {
        self.parameters.iter().find(|parameter| parameter.name == name)
    }

    pub fn has_transition(&self, name: &str) -> bool {
        self.transitions.iter().any(|transition| transition.name == name)
    }

    /// A transition by name for an HTTP method. A method-specific transition wins over `any`. With no method, as for a
    /// link, `any` comes first, then `get`, then the first declared; Java finds none there (Parity-Diffs P7-11).
    pub fn transition(&self, name: &str, method: Option<&str>) -> Option<&TransitionDef> {
        let method = method.map(str::to_ascii_lowercase).unwrap_or_default();
        self.transitions
            .iter()
            .find(|item| item.name == name && !method.is_empty() && item.method == method)
            .or_else(|| {
                self.transitions
                    .iter()
                    .find(|item| item.name == name && item.method == "any")
            })
            .or_else(|| {
                self.transitions
                    .iter()
                    .find(|item| item.name == name && method.is_empty() && item.method == "get")
            })
            .or_else(|| {
                self.transitions
                    .iter()
                    .find(|item| item.name == name && method.is_empty())
            })
            .or_else(|| {
                // As Java: a transition name is also a regular expression the whole name may match, first match wins,
                // for a request that has a method.
                if method.is_empty() {
                    return None;
                }
                self.transitions.iter().find(|item| {
                    (item.method == "any" || item.method == method)
                        && item.name.contains(['[', '(', '*', '+', '?', '|', '\\', '.', '{'])
                        && moqui_lib::object::regex_full_match(name, &item.name).unwrap_or(false)
                })
            })
    }

    pub fn is_server_static(&self, render_mode: &str) -> bool {
        self.server_static
            .iter()
            .any(|mode| mode == "all" || mode == render_mode)
    }

    /// Whether `render_mode` is allowed. `html-print` also answers to `xsl-fo`.
    pub fn allows_render_mode(&self, render_mode: &str) -> bool {
        self.render_modes
            .iter()
            .any(|mode| mode == "all" || mode == render_mode || (mode == "xsl-fo" && render_mode == "html-print"))
    }

    pub fn macro_template_location(&self, render_mode: &str) -> Option<&str> {
        self.macro_templates.get(render_mode).map(String::as_str)
    }

    /// The whole merged screen as an `MNode`, parsed on this thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored XML does not parse.
    pub fn screen_node(&self) -> Result<MNode, Error> {
        self.source.node()
    }

    /// Load `location`. `facade` serves `transition-include` and `section-include` targets.
    pub(super) fn load(
        facade: &ScreenFacade,
        factory: &ExecutionContextFactory,
        location: &str,
        depth: usize,
    ) -> Result<Self, Error> {
        if depth > MAX_INCLUDE_DEPTH {
            return Err(Error::new(format!(
                "screen includes are nested too deeply at {location}"
            )));
        }
        let resource = factory.resource().get(location)?;
        if !resource.exists() {
            return Err(Error::of_kind(
                crate::context::ErrorKind::NotFound,
                format!("screen not found: {location}"),
            ));
        }
        let mut sources = vec![Source {
            location: location.to_string(),
            modified: resource.last_modified(),
        }];
        let node = MNode::parse_text(location, &resource.read_text()?).map_err(Error::from)?;
        if !node.has_name("screen") {
            return Err(Error::new(format!("{location} root is {} and not screen", node.name())));
        }
        let extended = apply_screen_extends(factory, &node, location, &mut sources)?;
        Self::build(facade, factory, &node, location, depth, extended, sources)
    }

    fn build(
        facade: &ScreenFacade,
        factory: &ExecutionContextFactory,
        node: &MNode,
        location: &str,
        depth: usize,
        extended: Extended,
        mut sources: Vec<Source>,
    ) -> Result<Self, Error> {
        let file = location.rsplit('/').next().unwrap_or(location);
        let screen_name = file.split('.').next().unwrap_or(file).to_string();
        let int_attr = |name: &str| node.attribute(name).and_then(|text| text.trim().parse::<i64>().ok());
        let list_attr = |name: &str| -> Vec<String> {
            node.attribute(name)
                .map(|text| {
                    text.split(',')
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut render_modes = list_attr("render-modes");
        if render_modes.is_empty() {
            render_modes.push("all".to_string());
        }

        let web = node.first_named("web-settings");
        let web = WebSettings {
            allow_web_request: web
                .as_ref()
                .is_none_or(|web| !web.attribute_is("allow-web-request", "false")),
            require_encryption: web
                .as_ref()
                .is_none_or(|web| !web.attribute_is("require-encryption", "false")),
            mime_type: web
                .as_ref()
                .and_then(|web| web.attribute("mime-type"))
                .filter(|text| !text.is_empty()),
            character_encoding: web
                .as_ref()
                .and_then(|web| web.attribute("character-encoding"))
                .filter(|text| !text.is_empty()),
        };

        let always_actions = match node.first_named("always-actions") {
            Some(actions) => Some(Actions::from_node(&actions, &format!("{location} always-actions"))?),
            None => None,
        };
        let pre_actions = match node.first_named("pre-actions") {
            Some(actions) => Some(Actions::from_node(&actions, &format!("{location} pre-actions"))?),
            None => None,
        };

        let mut transitions: Vec<TransitionDef> = Vec::new();
        for transition in node.children_named("transition") {
            transitions.push(TransitionDef::of(&transition, location)?);
        }
        for include in node.children_named("transition-include") {
            let (name, from) = (
                include.attribute("name").unwrap_or_default(),
                include.attribute("location").unwrap_or_default(),
            );
            let other = facade.screen_def_at(factory, &from, depth + 1)?;
            let method = include.attribute("method");
            let Some(found) = other.transition(&name, method.as_deref()) else {
                return Err(Error::new(format!(
                    "{location}: transition-include could not find transition {name} in {from}"
                )));
            };
            let mut included = found.clone();
            included.screen_location = location.to_string();
            transitions.push(included);
            sources.extend(other.sources.iter().cloned());
        }

        for implicit in [
            Implicit::Actions,
            Implicit::FormSelectColumns,
            Implicit::FormSaveFind,
            Implicit::ScreenDoc,
        ] {
            let transition = TransitionDef::implicit(location, implicit);
            if !transitions.iter().any(|item| item.name == transition.name) {
                transitions.push(transition);
            }
        }

        // Read before the rows, so a write while they load makes this definition stale.
        let config_generation = screen_config_generation(factory);
        let (subscreens, rows_read) = load_subscreens(facade, factory, node, location, &mut sources)?;
        // Built without rows that could not be read, the definition is stale at once and is built again next time.
        let config_generation = if rows_read { config_generation } else { STALE_GENERATION };
        let mut macro_templates = BTreeMap::new();
        for template in node.children_named("macro-template") {
            if let (Some(kind), Some(from)) = (template.attribute("type"), template.attribute("location")) {
                macro_templates.insert(kind, from);
            }
        }

        let label = format!("{location} screen");
        let root = SectionDef::of(node, "", &label)?;
        let mut sections = BTreeMap::new();
        let mut forms: BTreeMap<String, FormDef> = BTreeMap::new();
        let mut trees = BTreeMap::new();
        let mut has_tab_menu = false;
        for widget_root in ["widgets", "fail-widgets"] {
            let Some(widgets) = node.first_named(widget_root) else {
                continue;
            };
            let mut found = Vec::new();
            descendants(&widgets, &mut found);
            for item in found {
                let name = item.attribute("name").unwrap_or_default();
                match item.name().as_str() {
                    "section" | "section-iterate" => {
                        let label = format!("{location} section {name}");
                        sections.insert(name.clone(), SectionDef::of(&item, &name, &label)?);
                    }
                    "section-include" => {
                        pull_section_include(
                            facade,
                            factory,
                            &item,
                            location,
                            depth,
                            &mut sections,
                            &mut forms,
                            &mut trees,
                            &mut sources,
                        )?;
                    }
                    kind @ ("form-single" | "form-list") => {
                        forms.insert(
                            name.clone(),
                            FormDef {
                                name: name.clone(),
                                list: kind == "form-list",
                                dynamic: item.attribute_is("dynamic", "true"),
                                node: XmlFragment::of(&item),
                                extensions: extended
                                    .forms
                                    .iter()
                                    .filter(|other| {
                                        other.name() == kind && other.attribute("name") == Some(name.clone())
                                    })
                                    .map(XmlFragment::of)
                                    .collect(),
                            },
                        );
                    }
                    "tree" => {
                        trees.insert(
                            name.clone(),
                            TreeDef {
                                name,
                                node: XmlFragment::of(&item),
                            },
                        );
                    }
                    "subscreens-panel" | "subscreens-menu" => {
                        // a wizard's steps and a stack's breadcrumb are its own menu, as tabs are
                        let kind = item.attribute("type").unwrap_or_default();
                        if kind.is_empty() || kind == "tab" || kind == "wizard" || kind == "stack" {
                            has_tab_menu = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        // A section in a screen-extend file replaces the base section of the same name.
        for item in &extended.sections {
            let name = item.attribute("name").unwrap_or_default();
            let label = format!("{location} extended section {name}");
            sections.insert(name.clone(), SectionDef::of(item, &name, &label)?);
        }

        let mut server_static = list_attr("server-static");
        let server_static_auto = server_static.is_empty() && is_placeholder(node);
        if server_static_auto {
            server_static.push("all".to_string());
        }

        let default_menu_title = node
            .attribute("default-menu-title")
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| pretty_menu_name(location));
        Ok(Self {
            location: location.to_string(),
            screen_name,
            authentication: Authentication::of(node.attribute("require-authentication")),
            standalone: node.attribute_is("standalone", "true"),
            allow_extra_path: node.attribute_is("allow-extra-path", "true"),
            begin_transaction: node.attribute_is("begin-transaction", "true"),
            transaction_timeout: int_attr("transaction-timeout"),
            include_child_content: node.attribute_is("include-child-content", "true"),
            history: !node.attribute_is("history", "false"),
            track_artifact_hit: !node.attribute_is("track-artifact-hit", "false"),
            login_path: node.attribute("login-path").filter(|text| !text.is_empty()),
            default_menu_title,
            default_menu_index: int_attr("default-menu-index"),
            default_menu_include: !node.attribute_is("default-menu-include", "false"),
            menu_image: node.attribute("menu-image").filter(|text| !text.is_empty()),
            menu_image_type: node
                .attribute("menu-image-type")
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "url-screen".to_string()),
            screen_theme_type: node
                .attribute("screen-theme-type-enum-id")
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "STT_INTERNAL".to_string()),
            render_modes,
            server_static,
            server_static_auto,
            web,
            macro_templates,
            parameters: parameters_of(node),
            always_actions,
            pre_actions,
            transitions,
            subscreens,
            root,
            sections,
            forms,
            trees,
            has_tab_menu,
            shell_providers: super::shell::ShellProviders::of(node, location),
            source: XmlFragment::of(node),
            sources,
            checked_at: std::sync::atomic::AtomicI64::new(super::cache::now_millis()),
            config_generation,
            generated: super::generate::Generated::default(),
        })
    }
}

/// Elements of a `screen-extend` file that the base screen load needs after the merge.
#[derive(Default)]
struct Extended {
    sections: Vec<MNode>,
    forms: Vec<MNode>,
}

fn apply_screen_extends(
    factory: &ExecutionContextFactory,
    screen: &MNode,
    location: &str,
    sources: &mut Vec<Source>,
) -> Result<Extended, Error> {
    let after_scheme = location.split_once("://").map_or(location, |(_, rest)| rest);
    let after_screen = after_scheme.find("/screen/").map(|index| &after_scheme[index + 8..]);
    let mut extended = Extended::default();
    for component in factory.components() {
        let directory = component.location.join("screen-extend");
        if !directory.is_dir() {
            continue;
        }
        let mut found = directory.join(after_scheme.trim_start_matches('/'));
        if !found.is_file() {
            if let Some(after) = after_screen {
                found = directory.join(after);
            }
        }
        if !found.is_file() {
            continue;
        }
        let path = found.to_string_lossy().into_owned();
        let modified = std::fs::metadata(&found)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok());
        sources.push(Source {
            location: path.clone(),
            modified,
        });
        let extend = MNode::parse_file(Path::new(&path)).map_err(Error::from)?;
        merge_extend(screen, &extend, location, &mut extended);
    }
    Ok(extended)
}

fn merge_extend(screen: &MNode, extend: &MNode, location: &str, extended: &mut Extended) {
    let mut found = Vec::new();
    descendants(extend, &mut found);
    for item in found {
        match item.name().as_str() {
            "section" | "section-iterate" => extended.sections.push(item),
            "form-single" | "form-list" => extended.forms.push(item),
            _ => {}
        }
    }
    screen.copy_attributes_from(extend);
    for child in ["parameter", "transition", "transition-include"] {
        let keys: &[&str] = &["name"];
        screen.merge_children_by_keys(extend, child, keys, None);
    }
    if let Some(over) = extend.first_named("subscreens") {
        match screen.first_named("subscreens") {
            Some(base) => base.merge_node_with_child_key(&over, "subscreens-item", &["name"], None),
            None => screen.append_node(over.deep_copy()),
        }
    }
    for actions_extend in extend.children_named("actions-extend") {
        let kind = actions_extend
            .attribute("type")
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "actions".to_string());
        let current = screen.first_named(&kind).unwrap_or_else(|| screen.append(kind.clone()));
        let copies: Vec<MNode> = actions_extend.children().iter().map(MNode::deep_copy).collect();
        match actions_extend.attribute("when").as_deref() {
            Some("replace") => {
                current.clear_children();
                copies.into_iter().for_each(|copy| current.append_node(copy));
            }
            Some("before") => {
                for (offset, copy) in copies.into_iter().enumerate() {
                    current.insert_node(offset, copy);
                }
            }
            _ => copies.into_iter().for_each(|copy| current.append_node(copy)),
        }
    }
    for widgets_extend in extend.children_named("widgets-extend") {
        let Some(target) = widgets_extend.attribute("name").filter(|text| !text.is_empty()) else {
            continue;
        };
        let mut matches = Vec::new();
        breadth_first(screen, &mut |candidate| {
            if candidate.attribute_is("name", &target) || candidate.attribute_is("id", &target) {
                matches.push(candidate.clone());
            }
        });
        for matched in matches {
            let Some(parent) = matched.parent() else {
                tracing::warn!("screen-extend element {target} has no parent in {location}");
                continue;
            };
            let Some(mut index) = parent.index_of(&matched) else {
                continue;
            };
            if widgets_extend.attribute_is("where", "after") {
                index += 1;
            }
            for (offset, child) in widgets_extend.children().iter().enumerate() {
                parent.insert_node(index + offset, child.deep_copy());
            }
        }
    }
}

fn breadth_first(root: &MNode, visit: &mut dyn FnMut(&MNode)) {
    let mut queue = std::collections::VecDeque::from(root.children());
    while let Some(node) = queue.pop_front() {
        visit(&node);
        queue.extend(node.children());
    }
}

/// Every element below `root` that is a section, form, tree, section-include, or subscreens
/// panel or menu, in document order. Nested ones are included.
fn descendants(root: &MNode, out: &mut Vec<MNode>) {
    root.for_each_child(|child| {
        let wanted = child.with_name(|name| {
            matches!(
                name,
                "section"
                    | "section-iterate"
                    | "section-include"
                    | "form-single"
                    | "form-list"
                    | "tree"
                    | "subscreens-panel"
                    | "subscreens-menu"
            )
        });
        if wanted {
            out.push(child.clone());
        }
        descendants(child, out);
    });
}

/// A screen whose widgets are only the subscreens placeholders renders the same every time.
fn is_placeholder(node: &MNode) -> bool {
    let Some(widgets) = node.first_named("widgets") else {
        return false;
    };
    widgets.children().iter().all(|child| {
        matches!(
            child.name().as_str(),
            "subscreens-panel" | "subscreens-active" | "subscreens-menu"
        )
    })
}

#[expect(clippy::too_many_arguments)]
fn pull_section_include(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    include: &MNode,
    location: &str,
    depth: usize,
    sections: &mut BTreeMap<String, SectionDef>,
    forms: &mut BTreeMap<String, FormDef>,
    trees: &mut BTreeMap<String, TreeDef>,
    sources: &mut Vec<Source>,
) -> Result<(), Error> {
    let mut from = include.attribute("location").unwrap_or_default();
    let mut name = include.attribute("name").unwrap_or_default();
    if from.contains("${") || name.contains("${") {
        sections.insert(
            name.clone(),
            SectionDef {
                name: name.clone(),
                kind: SectionKind::Include { location: from, name },
                condition: None,
                actions: None,
                widgets: None,
                fail_widgets: None,
            },
        );
        return Ok(());
    }
    if let Some((path, section)) = from.clone().split_once('#') {
        from = path.to_string();
        name = section.to_string();
    }
    let other = facade.screen_def_at(factory, &from, depth + 1)?;
    let Some(section) = other.sections.get(&name) else {
        return Err(Error::new(format!(
            "{location}: could not find section {name} to include at {from}"
        )));
    };
    sections.insert(name.clone(), section.clone());
    // As Java's pullSectionInclude: only the sections, forms and trees inside the included section come along, and a
    // section-include inside it is pulled the same way, from its own location: the sections, forms and trees nested in
    // the section it includes are then found from this screen too.
    let mut names: Vec<(String, String, MNode)> = Vec::new();
    for fragment in [&section.widgets, &section.fail_widgets].into_iter().flatten() {
        collect_named(&fragment.node()?, &mut names);
    }
    for (element, key, node) in names {
        match element.as_str() {
            "section-include" => {
                if depth >= MAX_INCLUDE_DEPTH {
                    return Err(Error::new(format!(
                        "{location}: section includes are nested too deeply at {from}"
                    )));
                }
                pull_section_include(
                    facade,
                    factory,
                    &node,
                    &from,
                    depth + 1,
                    sections,
                    forms,
                    trees,
                    sources,
                )?;
            }
            "section" | "section-iterate" => {
                if let Some(value) = other.sections.get(&key) {
                    sections.entry(key).or_insert_with(|| value.clone());
                }
            }
            "form-single" | "form-list" => {
                if let Some(value) = other.forms.get(&key) {
                    forms.entry(key).or_insert_with(|| value.clone());
                }
            }
            _ => {
                if let Some(value) = other.trees.get(&key) {
                    trees.entry(key).or_insert_with(|| value.clone());
                }
            }
        }
    }
    sources.extend(other.sources.iter().cloned());
    Ok(())
}

/// The sections, section includes, forms and trees below `node`: element, name and node. A section-include is there
/// without a name too, as its location may name the section after `#`.
fn collect_named(node: &MNode, out: &mut Vec<(String, String, MNode)>) {
    for child in node.children() {
        let element = child.name();
        if element == "section-include" {
            out.push((
                element.clone(),
                child.attribute("name").unwrap_or_default(),
                child.clone(),
            ));
        } else if matches!(
            element.as_str(),
            "section" | "section-iterate" | "form-single" | "form-list" | "tree"
        ) {
            if let Some(name) = child.attribute("name").filter(|name| !name.is_empty()) {
                out.push((element.clone(), name, child.clone()));
            }
        }
        collect_named(&child, out);
    }
}

fn load_subscreens(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    node: &MNode,
    location: &str,
    sources: &mut Vec<Source>,
) -> Result<(Subscreens, bool), Error> {
    let mut subscreens = Subscreens::default();
    let mut rows_read = true;
    let element = node.first_named("subscreens");
    // The directory named like the screen file, minus .xml, holds subscreens by convention.
    let base = location.strip_suffix(".xml").unwrap_or(location);
    let directory = factory.resource().get(base)?;
    if directory.is_directory() {
        // The directory's own time changes when a file is added or removed; each file's when its menu title does.
        sources.push(Source::of(factory, base));
        let mut entries = directory.children()?;
        entries.sort_by(|left, right| left.location().cmp(right.location()));
        for entry in entries {
            let file = entry.location().rsplit('/').next().unwrap_or("").to_string();
            let Some(name) = file.strip_suffix(".xml") else {
                continue;
            };
            if !entry.is_file() {
                continue;
            }
            let Ok(root) = MNode::parse_root_only(entry.location(), &entry.read_text()?) else {
                continue;
            };
            if !root.has_name("screen") {
                continue;
            }
            let item_location = format!("{base}/{file}");
            sources.push(Source::of(factory, entry.location()));
            subscreens.put(SubscreensItem {
                name: name.to_string(),
                menu_title: root
                    .attribute("default-menu-title")
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| pretty_menu_name(&item_location)),
                menu_index: root
                    .attribute("default-menu-index")
                    .and_then(|text| text.trim().parse().ok()),
                menu_include: !root.attribute_is("default-menu-include", "false"),
                location: item_location,
                no_sub_path: false,
                disable_when: None,
                user_group_id: None,
            });
        }
    }
    if let Some(element) = &element {
        subscreens.default_item = element.attribute("default-item").filter(|text| !text.is_empty());
        subscreens.always_use_full_path = element.attribute_is("always-use-full-path", "true");
        for item in element.children_named("conditional-default") {
            let (condition, name) = (
                item.attribute("condition").unwrap_or_default(),
                item.attribute("item").unwrap_or_default(),
            );
            if condition.is_empty() || name.is_empty() {
                continue;
            }
            crate::script::parse_js(&condition).map_err(|err| {
                Error::new(format!(
                    "{location}: conditional-default `{condition}`: {}",
                    err.message()
                ))
            })?;
            subscreens.conditional_defaults.push((condition, name));
        }
        for item in element.children_named("subscreens-item") {
            if let Some(item) = subscreens_item_of(factory, base, &item, sources) {
                subscreens.put(item);
            }
        }
    }
    // Conf `screen-facade/screen` adds and overrides items for a screen by location.
    if let Some(conf) = facade.conf().screens.get(location) {
        for item in &conf.items {
            if let Some(item) = subscreens_item_of(factory, base, &item.node(), sources) {
                subscreens.put(item);
            }
        }
        if let Some(default) = &conf.default_subscreen {
            subscreens.default_item = Some(default.clone());
        }
    }
    // `moqui.screen.SubscreensItem` rows add and replace items last, as Java: this is how a component mounts its app
    // under `apps` from its data. The user group is checked when the item is used, not here, since the definition is
    // shared by every user.
    if let Some(entity) = factory.entity() {
        let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
        let rows = entity.find("moqui.screen.SubscreensItem").and_then(|find| {
            find.eq("screenLocation", moqui_lib::object::Value::string(location))
                .use_cache(true)
                .list()
        });
        match rows {
            Ok(rows) => {
                for row in rows.values() {
                    let text = |name: &str| {
                        row.get(name)
                            .map(moqui_lib::object::to_plain_string)
                            .filter(|text| !text.is_empty())
                    };
                    let Some(name) = text("subscreenName") else {
                        continue;
                    };
                    let item_location = text("subscreenLocation").unwrap_or_else(|| format!("{base}/{name}.xml"));
                    let menu_title = text("menuTitle").unwrap_or_else(|| {
                        sources.push(Source::of(factory, &item_location));
                        target_menu_title(factory, &item_location)
                    });
                    if text("makeDefault").as_deref() == Some("Y") {
                        subscreens.default_item = Some(name.clone());
                    }
                    subscreens.put(SubscreensItem {
                        menu_index: text("menuIndex").and_then(|index| index.trim().parse().ok()),
                        menu_include: text("menuInclude").as_deref() == Some("Y"),
                        no_sub_path: text("noSubPath").as_deref() == Some("Y"),
                        disable_when: None,
                        user_group_id: text("userGroupId"),
                        name,
                        location: item_location,
                        menu_title,
                    });
                }
            }
            // A database without the screen tables (a bare test factory) has no rows to add.
            Err(err) if screen_entity_absent(&entity, "moqui.screen.SubscreensItem") => {
                tracing::debug!("no SubscreensItem rows for {location}: {err}");
            }
            Err(err) => {
                tracing::warn!(
                    "could not read SubscreensItem rows for {location}; reading them again next time: {err}"
                );
                rows_read = false;
            }
        }
    }
    Ok((subscreens, rows_read))
}

/// The `default-menu-title` of the screen at `location`, else its pretty file name.
fn target_menu_title(factory: &ExecutionContextFactory, location: &str) -> String {
    factory
        .resource()
        .get(location)
        .ok()
        .filter(|resource| resource.exists() && resource.is_file())
        .and_then(|resource| resource.read_text().ok())
        .and_then(|text| MNode::parse_root_only(location, &text).ok())
        .and_then(|root| root.attribute("default-menu-title"))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| pretty_menu_name(location))
}

fn subscreens_item_of(
    factory: &ExecutionContextFactory,
    base: &str,
    item: &MNode,
    sources: &mut Vec<Source>,
) -> Option<SubscreensItem> {
    let name = item.attribute("name").filter(|text| !text.is_empty())?;
    // An empty location means the file named like the item under the screen's directory.
    let location = item
        .attribute("location")
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("{base}/{name}.xml"));
    let menu_title = item
        .attribute("menu-title")
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            // The target's own default-menu-title, else the pretty file name. The title is read again when the target changes.
            sources.push(Source::of(factory, &location));
            target_menu_title(factory, &location)
        });
    Some(SubscreensItem {
        name,
        menu_title,
        menu_index: item.attribute("menu-index").and_then(|text| text.trim().parse().ok()),
        menu_include: item
            .attribute("menu-include")
            .or_else(|| item.attribute("include-in-menu"))
            .as_deref()
            != Some("false"),
        no_sub_path: item.attribute_is("no-sub-path", "true"),
        disable_when: item.attribute("disable-when").filter(|text| !text.is_empty()),
        user_group_id: None,
        location,
    })
}

/// `FindCustomer.xml` is `Find Customer`.
pub(crate) fn pretty_menu_name(location: &str) -> String {
    let file = location.rsplit('/').next().unwrap_or(location);
    let stem = file.split('.').next().unwrap_or(file);
    let mut out = String::new();
    for (index, ch) in stem.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            out.push(' ');
        }
        if index == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn flag(value: Option<String>) -> Option<bool> {
    match value.as_deref() {
        Some("false") => Some(false),
        Some("true") => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use moqui_lib::node::MNode;

    use super::{merge_extend, Extended, ScreenDef, SectionKind};
    use crate::context::ExecutionContextFactory;
    use crate::screen::ScreenFacade;

    const BASE: &str = r#"<screen require-authentication="false" default-menu-title="Base">
        <parameter name="a"/>
        <transition name="t1"><default-response type="none"/></transition>
        <subscreens default-item="One"><subscreens-item name="One" location="x/One.xml" menu-title="One"/></subscreens>
        <pre-actions><script>base_pre()</script></pre-actions>
        <actions><script>base_actions()</script></actions>
        <widgets>
            <container id="box"><label text="inside"/></container>
            <section name="main"><widgets><label text="main"/></widgets></section>
            <form-single name="Edit"><field name="f1"/></form-single>
        </widgets>
    </screen>"#;

    const EXTEND: &str = r#"<screen-extend default-menu-title="Extended" menu-image="icon">
        <parameter name="b" required="true"/>
        <transition name="t2"><default-response type="none"/></transition>
        <subscreens><subscreens-item name="Two" location="x/Two.xml" menu-title="Two"/></subscreens>
        <actions-extend type="pre-actions" when="before"><script>extend_pre_before()</script></actions-extend>
        <actions-extend type="actions" when="replace"><script>extend_actions()</script></actions-extend>
        <actions-extend type="always-actions"><script>extend_always()</script></actions-extend>
        <widgets-extend name="box" where="before"><label text="before-box"/></widgets-extend>
        <widgets-extend name="main" where="after"><label text="after-main"/></widgets-extend>
        <section name="main"><widgets><label text="replaced"/></widgets></section>
        <form-single name="Edit"><field name="f2"/></form-single>
    </screen-extend>"#;

    #[test]
    fn a_screen_extend_merges_attributes_actions_widgets_and_replaces_sections() {
        let context = ExecutionContextFactory::test_context();
        let facade = ScreenFacade::of(context.factory());
        let base = MNode::parse_text("Base.xml", BASE).unwrap();
        let extend = MNode::parse_text("Base.xml", EXTEND).unwrap();
        let mut extended = Extended::default();
        merge_extend(&base, &extend, "Base.xml", &mut extended);
        let def = ScreenDef::build(&facade, context.factory(), &base, "Base.xml", 0, extended, Vec::new()).unwrap();

        assert_eq!(def.default_menu_title, "Extended");
        assert_eq!(def.menu_image.as_deref(), Some("icon"));
        assert!(def.parameter("a").is_some() && def.parameter("b").unwrap().required);
        assert!(def.has_transition("t1") && def.has_transition("t2"));
        assert!(def.subscreens.get("One").is_some() && def.subscreens.get("Two").is_some());
        // The base subscreens default stays when the extend names none.
        assert_eq!(def.subscreens.default_item.as_deref(), Some("One"));

        let pre = def.pre_actions.as_ref().unwrap().xml.to_string();
        assert!(
            pre.find("extend_pre_before").unwrap() < pre.find("base_pre").unwrap(),
            "{pre}"
        );
        let actions = def.root.actions.as_ref().unwrap().xml.to_string();
        assert!(
            actions.contains("extend_actions") && !actions.contains("base_actions"),
            "{actions}"
        );
        assert!(def.always_actions.as_ref().unwrap().xml.contains("extend_always"));

        let source = def.source.text();
        let before = source.find("before-box").unwrap();
        assert!(before < source.find("id=\"box\"").unwrap(), "{source}");
        // The extend replaced the section by name, and a widgets-extend after it adds its widgets.
        let main = def.sections.get("main").unwrap();
        assert!(main.widgets.as_ref().unwrap().text().contains("replaced"));
        assert!(source.contains("after-main"));
        // Same-named forms keep the base form and list the extension beside it.
        let form = def.forms.get("Edit").unwrap();
        assert!(!form.list && form.extensions.len() == 1);
        assert!(form.extensions[0].text().contains("f2"));
    }

    #[test]
    fn sections_forms_trees_includes_and_settings_are_read() {
        let dir = std::env::temp_dir().join(format!("moqui-screen-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Shared.xml"),
            r#"<screen require-authentication="false">
                <transition name="shared"><default-response type="none"/></transition>
                <widgets><section name="common"><actions><script>common_actions()</script></actions><widgets><label text="c"/>
                    <form-single name="SharedIn"><field name="a"><default-field><display/></default-field></field></form-single></widgets></section>
                    <form-single name="SharedOut"><field name="b"><default-field><display/></default-field></field></form-single></widgets>
            </screen>"#,
        )
        .unwrap();
        let shared = dir.join("Shared.xml").to_string_lossy().into_owned();
        let model = format!(
            r#"<screen require-authentication="anonymous-view" begin-transaction="true" transaction-timeout="90"
                    include-child-content="true" history="false" menu-image="fa-home" menu-image-type="icon"
                    server-static="qvt, html" render-modes="html,xsl-fo" login-path="/In" track-artifact-hit="false">
                <macro-template type="html" location="x/Html.js"/>
                <web-settings mime-type="text/x-test" character-encoding="ISO-8859-1" require-encryption="false"/>
                <parameter name="p1" required="true" from="x"/>
                <transition-include name="shared" location="{shared}"/>
                <transition name="svc" read-only="true" authz-action="all">
                    <condition><expression>ok</expression></condition>
                    <path-parameter name="id"/>
                    <conditional-response type="screen-last" save-current-screen="true"/>
                    <default-response url="Next" url-type="plain" parameter-map="{{a: 1}}" save-parameters="true"/>
                    <error-response type="none"/>
                </transition>
                <subscreens default-item="A" always-use-full-path="true">
                    <conditional-default condition="x == 1" item="B"/>
                    <subscreens-item name="A" location="a/A.xml" menu-title="Item A" menu-index="3" menu-include="false" no-sub-path="true"/>
                </subscreens>
                <widgets>
                    <subscreens-menu id="m" type="tab"/>
                    <section name="outer" condition="flag"><actions><set field="o" from="1"/></actions>
                        <widgets><section-iterate name="rows" list="things" entry="thing" key="k" paginate="true">
                            <widgets><label text="${{thing}}"/></widgets></section-iterate></widgets>
                        <fail-widgets><label text="none"/></fail-widgets>
                    </section>
                    <section-include name="common" location="{shared}"/>
                    <section-include name="dyn${{x}}" location="${{y}}"/>
                    <form-list name="List1" list="things"/>
                    <tree name="Tree1"/>
                </widgets>
                <fail-widgets><label text="root-fail"/></fail-widgets>
            </screen>"#
        );
        std::fs::write(dir.join("Model.xml"), model).unwrap();
        let context = ExecutionContextFactory::test_context();
        let facade = ScreenFacade::of(context.factory());
        let def = facade
            .screen_def(context.factory(), &dir.join("Model.xml").to_string_lossy())
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(def.authentication, super::Authentication::AnonymousView);
        assert!(def.begin_transaction && def.transaction_timeout == Some(90));
        assert!(def.include_child_content && !def.history && !def.track_artifact_hit);
        assert_eq!(def.menu_image.as_deref(), Some("fa-home"));
        assert_eq!(def.menu_image_type, "icon");
        assert_eq!(def.login_path.as_deref(), Some("/In"));
        assert_eq!(def.server_static, ["qvt", "html"]);
        assert!(def.is_server_static("qvt") && !def.is_server_static("csv"));
        assert!(
            def.allows_render_mode("html") && def.allows_render_mode("html-print") && !def.allows_render_mode("csv")
        );
        assert_eq!(def.macro_template_location("html"), Some("x/Html.js"));
        assert_eq!(def.web.mime_type.as_deref(), Some("text/x-test"));
        assert_eq!(def.web.character_encoding.as_deref(), Some("ISO-8859-1"));
        assert!(!def.web.require_encryption && def.web.allow_web_request);
        assert!(def.has_tab_menu);
        assert_eq!(def.default_menu_title, "Model");

        // transition-include copies the transition and runs it under this screen.
        let shared = def.transition("shared", None).unwrap();
        assert_eq!(shared.screen_location, def.location);
        let svc = def.transition("svc", None).unwrap();
        assert!(svc.read_only && svc.authz_action == "all" && svc.condition.is_some());
        assert_eq!(svc.default_response.url_type, "plain");
        assert_eq!(svc.default_response.parameter_map.as_deref(), Some("{a: 1}"));
        assert!(svc.default_response.save_parameters);
        assert_eq!(svc.conditional_responses[0].kind, super::ResponseKind::ScreenLast);
        assert!(svc.conditional_responses[0].save_current_screen);

        let a = def.subscreens.get("A").unwrap();
        assert!(a.no_sub_path && !a.menu_include && a.menu_index == Some(3));
        assert!(def.subscreens.always_use_full_path);
        assert_eq!(def.subscreens.conditional_defaults.len(), 1);

        let outer = def.sections.get("outer").unwrap();
        assert!(matches!(outer.kind, SectionKind::Section));
        assert!(outer.condition.is_some() && outer.actions.is_some());
        assert!(outer.fail_widgets.as_ref().unwrap().text().contains("none"));
        match &def.sections.get("rows").unwrap().kind {
            SectionKind::Iterate {
                list,
                entry,
                key,
                paginate,
            } => {
                assert_eq!(
                    (list.as_str(), entry.as_str(), key.as_deref(), *paginate),
                    ("things", "thing", Some("k"), true)
                );
            }
            other => panic!("{other:?}"),
        }
        // An included section is copied in; a name or location with ${} waits for the render.
        assert!(def.sections.get("common").unwrap().actions.is_some());
        // Only what the included section holds comes along.
        assert!(def.forms.contains_key("SharedIn") && !def.forms.contains_key("SharedOut"));
        assert!(matches!(
            def.sections.get("dyn${x}").unwrap().kind,
            SectionKind::Include { .. }
        ));
        assert!(def.forms.get("List1").unwrap().list);
        assert!(def.trees.contains_key("Tree1"));
        assert!(def.root.fail_widgets.is_some());
        // The widget subtree comes back as an MNode for the macro walker.
        let widgets = outer.widgets.as_ref().unwrap().node().unwrap();
        assert_eq!(widgets.name(), "widgets");
        assert_eq!(def.screen_node().unwrap().name(), "screen");
    }

    #[test]
    fn a_screen_of_only_subscreens_placeholders_is_server_static() {
        let context = ExecutionContextFactory::test_context();
        let facade = ScreenFacade::of(context.factory());
        let node = MNode::parse_text(
            "P.xml",
            "<screen><widgets><subscreens-menu id=\"m\"/><subscreens-active/></widgets></screen>",
        )
        .unwrap();
        let def = ScreenDef::build(
            &facade,
            context.factory(),
            &node,
            "P.xml",
            0,
            Extended::default(),
            Vec::new(),
        )
        .unwrap();
        assert!(def.is_server_static("qvt"));
        assert_eq!(
            super::pretty_menu_name("a/FindCustomerOrder.xml"),
            "Find Customer Order"
        );
    }

    #[test]
    fn a_transition_name_is_also_a_pattern_for_a_request_with_a_method() {
        let context = ExecutionContextFactory::test_context();
        let facade = ScreenFacade::of(context.factory());
        let node = MNode::parse_text(
            "T.xml",
            r#"<screen><transition name="exact"><default-response type="none"/></transition>
                <transition name="report_(csv|xml)" method="get"><default-response type="none"/></transition></screen>"#,
        )
        .unwrap();
        let def = ScreenDef::build(
            &facade,
            context.factory(),
            &node,
            "T.xml",
            0,
            Extended::default(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            def.transition("exact", Some("post")).map(|t| t.name.as_str()),
            Some("exact")
        );
        assert_eq!(
            def.transition("report_csv", Some("GET")).map(|t| t.name.as_str()),
            Some("report_(csv|xml)")
        );
        assert!(
            def.transition("report_csv", Some("post")).is_none(),
            "the method must match"
        );
        assert!(
            def.transition("report_csv", None).is_none(),
            "a lookup with no method takes names only"
        );
        assert!(def.transition("report_pdf", Some("get")).is_none());
    }

    #[test]
    fn a_lookup_with_no_method_prefers_any_then_get_over_the_first_declared() {
        let context = ExecutionContextFactory::test_context();
        let facade = ScreenFacade::of(context.factory());
        let node = MNode::parse_text(
            "T.xml",
            r#"<screen><transition name="image" method="post"><default-response type="none"/></transition>
                <transition name="image" method="get" read-only="true"><default-response type="none"/></transition>
                <transition name="oneClick" method="post"><default-response type="none"/></transition>
                <transition name="oneClick" read-only="true"><default-response type="none"/></transition>
                <transition name="save" method="post"><default-response type="none"/></transition></screen>"#,
        )
        .unwrap();
        let def = ScreenDef::build(
            &facade,
            context.factory(),
            &node,
            "T.xml",
            0,
            Extended::default(),
            Vec::new(),
        )
        .unwrap();
        let method = |name: &str, request: Option<&str>| def.transition(name, request).map(|t| t.method.as_str());
        assert_eq!(method("image", None), Some("get"));
        assert_eq!(method("image", Some("post")), Some("post"));
        assert_eq!(method("oneClick", None), Some("any"));
        assert_eq!(method("oneClick", Some("post")), Some("post"));
        assert_eq!(
            method("save", None),
            Some("post"),
            "the first declared is the last fallback"
        );
    }
}
