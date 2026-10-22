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

//! The `webroot` webapp's `servlet`, `filter` and `listener` elements and its lifecycle actions, as Java's
//! `MoquiContextListener` and `WebappInfo` set them up. A servlet, filter or listener is a script whose value is a
//! handler object (`location`), or one of the Java classes the port has natively ([`BUILT_IN`]); any other class is
//! logged and left out. Servlets and filters are mapped by URL pattern as the Servlet spec (12.2) maps them. Guide:
//! `docs/Web-Scripts.md`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::SourceLocation;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::context::{Error, ExecutionContextFactory};

/// What a servlet runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// `MoquiServlet`: the token routes, then the screens.
    Screen,
    /// `MoquiFopServlet`: the screen at the path info as a PDF.
    Fop,
    /// A script whose value is the handler object.
    Script(String),
}

/// The Java classes of the shipped conf that the port has natively, and the AuthFilter script that stands for
/// `MoquiAuthFilter`, so a conf written for Java keeps working.
const BUILT_IN: &[(&str, &str)] = &[
    ("org.moqui.impl.webapp.MoquiServlet", "servlet:screen"),
    ("org.moqui.impl.webapp.MoquiFopServlet", "servlet:fop"),
    ("org.moqui.impl.webapp.MoquiAuthFilter", AUTH_FILTER),
    ("org.moqui.impl.webapp.ElasticRequestLogFilter", "filter:request-log"),
    ("org.moqui.impl.webapp.MoquiSessionListener", "listener:session"),
];

/// The script of Java's `MoquiAuthFilter`.
pub const AUTH_FILTER: &str = "component://webroot/script/AuthFilter.js";

/// The lifecycle elements of a webapp, Java's `WebappInfo` actions.
pub const LIFECYCLE: [&str; 7] = [
    "first-hit-in-visit",
    "before-request",
    "after-request",
    "after-login",
    "before-logout",
    "after-startup",
    "before-shutdown",
];

/// One `url-pattern`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    /// `/path`: that path only; `""` is the root.
    Exact(String),
    /// `/path/*`: the path and everything under it, `""` for `/*`.
    Prefix(String),
    /// `*.ext`
    Extension(String),
    /// `/`: what nothing else maps.
    Default,
}

impl Pattern {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Some(Self::Exact("/".to_string()));
        }
        if text == "/" {
            return Some(Self::Default);
        }
        if let Some(extension) = text.strip_prefix("*.") {
            return (!extension.is_empty() && !extension.contains('/')).then(|| Self::Extension(extension.to_string()));
        }
        if !text.starts_with('/') {
            return None;
        }
        if let Some(prefix) = text.strip_suffix("/*") {
            return Some(Self::Prefix(prefix.to_string()));
        }
        (!text.contains('*')).then(|| Self::Exact(text.to_string()))
    }

    /// Whether a filter on this pattern sees `path`.
    fn covers(&self, path: &str) -> bool {
        match self {
            Self::Exact(exact) => path == exact,
            Self::Prefix(prefix) => under(path, prefix),
            Self::Extension(extension) => extension_of(path) == Some(extension.as_str()),
            Self::Default => true,
        }
    }
}

/// `path` is `prefix` or under it, on a segment boundary.
fn under(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}

fn extension_of(path: &str) -> Option<&str> {
    let last = path.rsplit('/').next()?;
    last.rsplit_once('.').map(|(_, extension)| extension)
}

/// A `servlet` element.
#[derive(Debug)]
pub struct ServletDef {
    pub name: String,
    pub handler: Handler,
    patterns: Vec<Pattern>,
    init_params: BTreeMap<String, String>,
    /// `max-body`: the largest request body, over the webapp's own limit.
    pub max_body: Option<usize>,
}

/// A `filter` element with a script.
#[derive(Debug)]
pub struct FilterDef {
    pub name: String,
    pub location: String,
    patterns: Vec<Pattern>,
    init_params: BTreeMap<String, String>,
}

/// A `listener` element with a script.
#[derive(Debug)]
pub struct ListenerDef {
    pub location: String,
}

/// The servlet a path goes to, with the servlet API's servlet path and path info.
#[derive(Debug, Clone)]
pub struct Match {
    pub servlet: Arc<ServletDef>,
    pub servlet_path: String,
    pub path_info: Option<String>,
}

/// The `webroot` webapp's servlets, filters, listeners and lifecycle actions, read once from the merged conf.
#[derive(Debug, Default)]
pub struct WebappScripts {
    servlets: Vec<Arc<ServletDef>>,
    /// In `priority` order, then as declared.
    filters: Vec<Arc<FilterDef>>,
    listeners: Vec<ListenerDef>,
    /// Each lifecycle element's actions XML and the name its generated script has.
    actions: BTreeMap<&'static str, (String, SourceLocation)>,
    request_log: bool,
}

fn data_of(name: &str, location: &str, init_params: &BTreeMap<String, String>) -> Value {
    let mut map = indexmap::IndexMap::new();
    map.insert("name".to_string(), Value::string(name));
    map.insert("location".to_string(), Value::string(location));
    map.insert(
        "initParams".to_string(),
        Value::Map(
            init_params
                .iter()
                .map(|(key, value)| (key.clone(), Value::string(value)))
                .collect(),
        ),
    );
    Value::Map(map)
}

fn init_params(node: &MNode) -> BTreeMap<String, String> {
    node.children_named("init-param")
        .into_iter()
        .filter_map(|param| {
            let name = param.attribute("name")?;
            let raw = param.attribute("value").unwrap_or_default();
            Some((name, moqui_lib::env::expand(&raw).unwrap_or(raw)))
        })
        .collect()
}

fn patterns(node: &MNode, label: &str) -> Vec<Pattern> {
    node.children_named("url-pattern")
        .into_iter()
        .filter_map(|pattern| {
            let text = pattern.text().unwrap_or_default();
            let parsed = Pattern::parse(&text);
            if parsed.is_none() {
                tracing::error!("Leaving out the url-pattern {text} of {label}: not a servlet URL pattern");
            }
            parsed
        })
        .collect()
}

fn enabled(node: &MNode) -> bool {
    node.attribute("enabled").as_deref() != Some("false")
}

/// The script a `location` or a built-in `class` names, or what the built-in is; an error naming the element when
/// there is neither.
fn target(node: &MNode, label: &str) -> Result<String, String> {
    if let Some(location) = node.attribute("location").filter(|text| !text.trim().is_empty()) {
        return Ok(location.trim().to_string());
    }
    let class = node.attribute("class").unwrap_or_default();
    if class.is_empty() {
        return Err(format!("{label} has neither a location nor a class"));
    }
    BUILT_IN
        .iter()
        .find(|(name, _)| *name == class)
        .map(|(_, target)| (*target).to_string())
        .ok_or_else(|| format!("{label} has class {class}, which this server does not have; use a script location"))
}

impl WebappScripts {
    /// Read the `webroot` webapp of the merged conf. Each element that cannot be used is logged and left out. A
    /// webapp that declares no servlet gets the shipped two: the screens on `/*` and the PDF servlet on `/fop/*`.
    pub fn of_conf(conf_text: &str) -> Self {
        let webapp = MNode::parse_text("conf", conf_text).ok().and_then(|root| {
            root.first_named("webapp-list")?
                .first_named_attrs("webapp", &[("name", "webroot")])
        });
        let mut scripts = Self {
            request_log: true,
            ..Self::default()
        };
        if let Some(webapp) = &webapp {
            scripts.read(webapp);
        }
        if scripts.servlets.is_empty() {
            scripts.servlets = vec![
                Arc::new(ServletDef {
                    name: "MoquiServlet".to_string(),
                    handler: Handler::Screen,
                    patterns: vec![Pattern::Prefix(String::new())],
                    init_params: BTreeMap::new(),
                    max_body: None,
                }),
                Arc::new(ServletDef {
                    name: "MoquiFopServlet".to_string(),
                    handler: Handler::Fop,
                    patterns: vec![Pattern::Prefix("/fop".to_string())],
                    init_params: BTreeMap::new(),
                    max_body: None,
                }),
            ];
        }
        scripts
    }

    fn read(&mut self, webapp: &MNode) {
        for kind in LIFECYCLE {
            let actions: Vec<MNode> = webapp
                .children_named(kind)
                .into_iter()
                .filter_map(|element| element.first_named("actions"))
                .collect();
            let Some(actions) = actions.first() else {
                continue;
            };
            let xml = actions.write_string();
            let location = SourceLocation {
                file: Some(format!("webapp_webroot.{kind}.actions")),
                line: 1,
                column: 1,
            };
            self.actions.insert(kind, (xml, location));
        }

        let mut filters: Vec<(i64, MNode)> = webapp
            .children_named("filter")
            .into_iter()
            .filter(enabled)
            .map(|node| {
                let priority = node
                    .attribute("priority")
                    .and_then(|text| text.trim().parse::<i64>().ok())
                    .unwrap_or(5);
                (priority, node)
            })
            .collect();
        filters.sort_by_key(|(priority, _)| *priority);
        for (_, node) in filters {
            let name = node.attribute("name").unwrap_or_default();
            let label = format!("filter {name}");
            match target(&node, &label).as_deref() {
                Ok("filter:request-log") => {}
                Ok(target) if target.contains(':') && !target.contains("://") => {
                    tracing::error!("Leaving out {label}: {target} is not a filter");
                }
                Ok(location) => self.filters.push(Arc::new(FilterDef {
                    name: name.clone(),
                    location: location.to_string(),
                    patterns: patterns(&node, &label),
                    init_params: init_params(&node),
                })),
                Err(message) => tracing::error!("Leaving out {message}"),
            }
        }
        // The request log is the host's; turned off only by its filter element saying so.
        self.request_log = !webapp.children_named("filter").into_iter().any(|node| {
            !enabled(&node)
                && node.attribute("class").as_deref() == Some("org.moqui.impl.webapp.ElasticRequestLogFilter")
        });

        for node in webapp.children_named("listener").into_iter().filter(enabled) {
            let label = format!(
                "listener {}",
                node.attribute("location")
                    .or_else(|| node.attribute("class"))
                    .unwrap_or_default()
            );
            match target(&node, &label).as_deref() {
                Ok("listener:session") => {}
                Ok(target) if target.contains(':') && !target.contains("://") => {
                    tracing::error!("Leaving out {label}: {target} is not a listener");
                }
                Ok(location) => self.listeners.push(ListenerDef {
                    location: location.to_string(),
                }),
                Err(message) => tracing::error!("Leaving out {message}"),
            }
        }

        for node in webapp.children_named("servlet").into_iter().filter(enabled) {
            let name = node.attribute("name").unwrap_or_default();
            let label = format!("servlet {name}");
            let handler = match target(&node, &label).as_deref() {
                Ok("servlet:screen") => Handler::Screen,
                Ok("servlet:fop") => Handler::Fop,
                Ok(target) if target.contains(':') && !target.contains("://") => {
                    tracing::error!("Leaving out {label}: {target} is not a servlet");
                    continue;
                }
                Ok(location) => Handler::Script(location.to_string()),
                Err(message) => {
                    tracing::error!("Leaving out {message}");
                    continue;
                }
            };
            let max_body = node.attribute("max-body").and_then(|text| {
                let text = moqui_lib::env::expand(&text).unwrap_or(text);
                let size = super::parse_size(&text);
                if size.is_none() && !text.trim().is_empty() {
                    tracing::error!("Ignoring the max-body {text} of {label}: not a size");
                }
                size
            });
            self.servlets.push(Arc::new(ServletDef {
                name,
                handler,
                patterns: patterns(&node, &label),
                init_params: init_params(&node),
                max_body,
            }));
        }
    }

    /// Check each script and action block once, logging what will fail: a script file that is not there, actions
    /// that do not compile. The element stays; it fails again, logged, when it runs.
    pub fn check(&self) {
        let locations = self
            .servlets
            .iter()
            .filter_map(|servlet| match &servlet.handler {
                Handler::Script(location) => Some((format!("servlet {}", servlet.name), location.as_str())),
                _ => None,
            })
            .chain(
                self.filters
                    .iter()
                    .map(|filter| (format!("filter {}", filter.name), filter.location.as_str())),
            )
            .chain(
                self.listeners
                    .iter()
                    .map(|listener| ("listener".to_string(), listener.location.as_str())),
            );
        for (label, location) in locations {
            if let Err(err) = crate::context::locate(location) {
                tracing::error!("The script {location} of {label} is not there: {}", err.message());
            }
        }
        for (kind, (xml, _)) in &self.actions {
            if let Err(err) = crate::script::check_actions(xml) {
                tracing::error!(
                    "The {kind} actions of the webroot webapp do not compile: {}",
                    err.message()
                );
            }
        }
    }

    /// The servlet for `path`: an exact pattern, else the longest path prefix, else an extension, else the default.
    pub fn match_servlet(&self, path: &str) -> Option<Match> {
        let path = if path.is_empty() { "/" } else { path };
        let found = |servlet: &Arc<ServletDef>, servlet_path: &str, path_info: Option<&str>| Match {
            servlet: Arc::clone(servlet),
            servlet_path: servlet_path.to_string(),
            path_info: path_info.map(str::to_string),
        };
        for servlet in &self.servlets {
            if servlet
                .patterns
                .iter()
                .any(|pattern| *pattern == Pattern::Exact(path.to_string()))
            {
                return Some(found(servlet, path, None));
            }
        }
        let mut best: Option<(&Arc<ServletDef>, &str)> = None;
        for servlet in &self.servlets {
            for pattern in &servlet.patterns {
                if let Pattern::Prefix(prefix) = pattern {
                    if under(path, prefix) && best.is_none_or(|(_, known)| prefix.len() > known.len()) {
                        best = Some((servlet, prefix.as_str()));
                    }
                }
            }
        }
        if let Some((servlet, prefix)) = best {
            let rest = &path[prefix.len()..];
            return Some(found(servlet, prefix, (!rest.is_empty()).then_some(rest)));
        }
        let extension = extension_of(path);
        for servlet in &self.servlets {
            if servlet
                .patterns
                .iter()
                .any(|pattern| matches!(pattern, Pattern::Extension(known) if Some(known.as_str()) == extension))
            {
                return Some(found(servlet, path, None));
            }
        }
        self.servlets
            .iter()
            .find(|servlet| servlet.patterns.contains(&Pattern::Default))
            .map(|servlet| found(servlet, path, None))
    }

    /// The filters a request for `path` passes through, in order.
    pub fn filters_for(&self, path: &str) -> Vec<Arc<FilterDef>> {
        self.filters
            .iter()
            .filter(|filter| filter.patterns.iter().any(|pattern| pattern.covers(path)))
            .cloned()
            .collect()
    }

    pub fn has_listeners(&self) -> bool {
        !self.listeners.is_empty()
    }

    /// Whether the host logs each request (the `ElasticRequestLogFilter` element, unless it is `enabled="false"`).
    pub fn request_log(&self) -> bool {
        self.request_log
    }

    /// The largest body a request for `path` may have when its servlet says (`max-body`).
    pub fn max_body_for(&self, path: &str) -> Option<usize> {
        self.match_servlet(path).and_then(|found| found.servlet.max_body)
    }

    /// Run a lifecycle element's actions, in a context frame of their own that holds `values`. Nothing when the
    /// webapp has none.
    ///
    /// # Errors
    ///
    /// Returns the error of the actions.
    pub fn run_actions(&self, kind: &str, values: &BTreeMap<String, Value>) -> Result<(), Error> {
        let Some((xml, location)) = self.actions.get(kind) else {
            return Ok(());
        };
        let _frame = crate::script::ContextFrame::push();
        for (name, value) in values {
            crate::script::set_context_name(name, value.clone())?;
        }
        crate::script::run_actions_at(xml, Some(location))
    }

    /// [`Self::run_actions`] where a failure does not stop what comes after: it is logged.
    pub fn run_actions_logged(&self, kind: &str, values: &BTreeMap<String, Value>) {
        if let Err(err) = self.run_actions(kind, values) {
            tracing::error!("Error in the {kind} actions of the webroot webapp: {err}");
        }
    }

    pub fn has_actions(&self, kind: &str) -> bool {
        self.actions.contains_key(kind)
    }

    /// Call each listener's method for `event` (`contextInitialized`, `sessionCreated`, `requestDestroyed`...) with
    /// `event_data` on the event object. A listener that fails is logged and the others still run.
    pub fn fire(&self, event: &str, event_data: Value) {
        for listener in &self.listeners {
            let mut data = indexmap::IndexMap::new();
            data.insert("name".to_string(), Value::string(&listener.location));
            data.insert("location".to_string(), Value::string(&listener.location));
            data.insert("initParams".to_string(), Value::Map(indexmap::IndexMap::new()));
            data.insert("event".to_string(), Value::string(event));
            data.insert("eventData".to_string(), event_data.clone());
            if let Err(err) = crate::script::invoke_web_handler(&listener.location, "listener", Value::Map(data)) {
                tracing::error!("Error in the {event} of listener {}: {err}", listener.location);
            }
        }
    }
}

/// The webapp is up, as Java's `MoquiContextListener.contextInitialized` ends: each script and action block is checked
/// (what will fail is logged now), the listeners' `contextInitialized`, then the `after-startup` actions.
pub fn started(factory: &Arc<ExecutionContextFactory>) {
    let context = factory.new_context();
    let _entered = context.enter();
    let scripts = factory.webapp_scripts();
    scripts.check();
    scripts.fire("contextInitialized", Value::Null);
    scripts.run_actions_logged("after-startup", &BTreeMap::new());
}

/// The webapp is stopping, as Java's `contextDestroyed`: the `before-shutdown` actions, then the listeners'
/// `contextDestroyed`, while the services and the database are still up.
pub fn stopping(factory: &Arc<ExecutionContextFactory>) {
    let context = factory.new_context();
    let _entered = context.enter();
    let scripts = factory.webapp_scripts();
    scripts.run_actions_logged("before-shutdown", &BTreeMap::new());
    scripts.fire("contextDestroyed", Value::Null);
}

impl ServletDef {
    /// What the handler object's `config` is made from.
    pub fn data(&self) -> Value {
        let location = match &self.handler {
            Handler::Script(location) => location.as_str(),
            _ => "",
        };
        data_of(&self.name, location, &self.init_params)
    }
}

impl FilterDef {
    pub fn data(&self) -> Value {
        data_of(&self.name, &self.location, &self.init_params)
    }
}

/// The end of a filter chain: the servlet.
pub type Terminal = Box<dyn FnOnce() -> Result<(), Error>>;

struct ChainState {
    filters: Vec<Arc<FilterDef>>,
    next: usize,
    terminal: Option<Terminal>,
}

thread_local! {
    /// The filter chains running on this thread, innermost last.
    static CHAINS: RefCell<Vec<ChainState>> = const { RefCell::new(Vec::new()) };
}

struct PopChain;

impl Drop for PopChain {
    fn drop(&mut self) {
        CHAINS.with(|chains| chains.borrow_mut().pop());
    }
}

/// Run `filters` in order, each passing on with `chain.doFilter`, and then `terminal`. A filter that does not pass
/// on ends the request with what it wrote.
///
/// # Errors
///
/// Returns the first error of a filter or the servlet that no filter caught.
pub fn run_chain(filters: Vec<Arc<FilterDef>>, terminal: Terminal) -> Result<(), Error> {
    if filters.is_empty() {
        return terminal();
    }
    CHAINS.with(|chains| {
        chains.borrow_mut().push(ChainState {
            filters,
            next: 0,
            terminal: Some(terminal),
        });
    });
    let _pop = PopChain;
    chain_next()
}

/// `chain.doFilter()`: the next filter of the innermost chain, or its servlet. Called again after the servlet ran,
/// it does nothing.
///
/// # Errors
///
/// Returns an error when no chain is running, or what the filter or servlet failed with.
pub fn chain_next() -> Result<(), Error> {
    enum Step {
        Filter(Arc<FilterDef>),
        Terminal(Terminal),
        Done,
    }
    let step = CHAINS.with(|chains| {
        let mut chains = chains.borrow_mut();
        let state = chains
            .last_mut()
            .ok_or_else(|| Error::new("chain.doFilter called with no filter chain running"))?;
        Ok::<_, Error>(if state.next < state.filters.len() {
            state.next += 1;
            Step::Filter(Arc::clone(&state.filters[state.next - 1]))
        } else {
            state.terminal.take().map_or(Step::Done, Step::Terminal)
        })
    })?;
    match step {
        Step::Filter(filter) => crate::script::invoke_web_handler(&filter.location, "filter", filter.data()),
        Step::Terminal(terminal) => terminal(),
        Step::Done => Ok(()),
    }
}

impl ExecutionContextFactory {
    /// The `webroot` webapp's servlets, filters, listeners and lifecycle actions.
    pub fn webapp_scripts(&self) -> Arc<WebappScripts> {
        self.webapp_scripts_cell()
            .get_or_init(|| Arc::new(WebappScripts::of_conf(self.conf_text())))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts(webapp: &str) -> WebappScripts {
        WebappScripts::of_conf(&format!(
            r#"<moqui-conf><webapp-list><webapp name="webroot">{webapp}</webapp></webapp-list></moqui-conf>"#
        ))
    }

    #[test]
    fn a_path_goes_to_the_exact_then_longest_prefix_then_extension_then_default_servlet() {
        let scripts = scripts(
            r#"<servlet name="Screens" class="org.moqui.impl.webapp.MoquiServlet"><url-pattern>/*</url-pattern></servlet>
            <servlet name="Fop" class="org.moqui.impl.webapp.MoquiFopServlet"><url-pattern>/fop/*</url-pattern></servlet>
            <servlet name="Search" location="component://webroot/script/ProxyServlet.js" max-body="100m">
                <init-param name="prefix" value="/search"/><url-pattern>/search/*</url-pattern></servlet>
            <servlet name="Deep" location="component://x/Deep.js"><url-pattern>/search/deep/*</url-pattern></servlet>
            <servlet name="Exact" location="component://x/Exact.js"><url-pattern>/search/exact</url-pattern></servlet>
            <servlet name="Java" class="com.example.Unknown"><url-pattern>/java/*</url-pattern></servlet>"#,
        );
        let at = |path: &str| {
            let found = scripts.match_servlet(path).unwrap();
            (found.servlet.name.clone(), found.servlet_path, found.path_info)
        };
        assert_eq!(at("/apps/x"), ("Screens".into(), "".into(), Some("/apps/x".into())));
        assert_eq!(at("/fop/apps/x"), ("Fop".into(), "/fop".into(), Some("/apps/x".into())));
        assert_eq!(at("/fopx"), ("Screens".into(), "".into(), Some("/fopx".into())));
        assert_eq!(at("/search"), ("Search".into(), "/search".into(), None));
        assert_eq!(
            at("/search/_cat/indices"),
            ("Search".into(), "/search".into(), Some("/_cat/indices".into()))
        );
        assert_eq!(
            at("/search/deep/x"),
            ("Deep".into(), "/search/deep".into(), Some("/x".into()))
        );
        assert_eq!(at("/search/exact"), ("Exact".into(), "/search/exact".into(), None));
        assert_eq!(at("/java/x").0, "Screens", "an unknown class is left out");
        assert_eq!(scripts.max_body_for("/search/x"), Some(100 * 1024 * 1024));
        assert_eq!(scripts.max_body_for("/apps"), None);

        let other = scripts_of_extensions();
        assert_eq!(other.match_servlet("/a/b.csv").unwrap().servlet.name, "Csv");
        assert_eq!(other.match_servlet("/a/b.txt").unwrap().servlet.name, "Default");
    }

    fn scripts_of_extensions() -> WebappScripts {
        scripts(
            r#"<servlet name="Csv" location="component://x/Csv.js"><url-pattern>*.csv</url-pattern></servlet>
            <servlet name="Default" location="component://x/Default.js"><url-pattern>/</url-pattern></servlet>"#,
        )
    }

    #[test]
    fn filters_run_by_priority_then_as_declared_and_java_classes_map_to_the_port() {
        let scripts = scripts(
            r#"<filter name="Log" class="org.moqui.impl.webapp.ElasticRequestLogFilter"><url-pattern>/*</url-pattern></filter>
            <filter name="Second" location="component://x/B.js"><url-pattern>/search/*</url-pattern></filter>
            <filter name="Auth" class="org.moqui.impl.webapp.MoquiAuthFilter" priority="1">
                <init-param name="permission" value="SearchRemote"/><url-pattern>/search/*</url-pattern></filter>
            <filter name="Off" location="component://x/C.js" enabled="false"><url-pattern>/*</url-pattern></filter>
            <filter name="Csv" location="component://x/D.js"><url-pattern>*.csv</url-pattern></filter>
            <listener class="org.moqui.impl.webapp.MoquiSessionListener"/>
            <listener location="component://x/Listen.js"/>"#,
        );
        let names = |path: &str| -> Vec<String> {
            scripts
                .filters_for(path)
                .iter()
                .map(|filter| filter.name.clone())
                .collect()
        };
        assert_eq!(names("/search/x"), ["Auth", "Second"]);
        assert_eq!(names("/searchx"), Vec::<String>::new());
        assert_eq!(names("/a/b.csv"), ["Csv"]);
        assert_eq!(scripts.filters_for("/search")[0].location, AUTH_FILTER);
        assert!(scripts.request_log());
        assert_eq!(scripts.listeners.len(), 1);
        // with no servlet declared, the screens and the PDF servlet
        assert_eq!(scripts.match_servlet("/fop/a").unwrap().servlet.handler, Handler::Fop);
        assert_eq!(scripts.match_servlet("/a").unwrap().servlet.handler, Handler::Screen);

        let off = super::tests::scripts(
            r#"<filter name="Log" class="org.moqui.impl.webapp.ElasticRequestLogFilter" enabled="false"/>"#,
        );
        assert!(!off.request_log());
    }

    #[test]
    fn lifecycle_actions_are_kept_by_element() {
        let scripts = scripts(
            r#"<before-request><actions><set field="a" value="1"/></actions></before-request>
            <after-startup><actions><log message="up"/></actions></after-startup>"#,
        );
        assert!(scripts.has_actions("before-request"));
        assert!(scripts.has_actions("after-startup"));
        assert!(!scripts.has_actions("after-request"));
        let (xml, location) = &scripts.actions["before-request"];
        assert!(xml.contains("<set"), "{xml}");
        assert_eq!(location.file.as_deref(), Some("webapp_webroot.before-request.actions"));
    }
}
