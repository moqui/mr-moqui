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

//! The screen statistics of the tools artifact stats screen: `ScreenFacadeImpl.getAllRootScreenLocations` and
//! `getScreenInfoList`. Each screen counts its own forms, trees, sections, and transitions, and the totals of all
//! the screens under it. The whole tree is walked; the list stops at `levels`.

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::cache::ScreenFacade;
use super::definition::ScreenDef;
use crate::context::{Error, ExecutionContextFactory};

/// How deep the walk goes before it stops, against a screen that mounts an ancestor.
const MAX_DEPTH: usize = 24;

/// The `root-screen` elements of each `webapp` in the merged conf, read once per conf generation. The error when the
/// conf does not parse.
struct RootScreens(Result<Vec<WebappRoots>, String>);

/// A webapp's name and its `root-screen` elements in conf order.
type WebappRoots = (String, Vec<RootEntry>);

/// One `root-screen`: its `host` pattern (empty when absent), `location` and `framework-routes`.
struct RootEntry {
    host: String,
    location: Option<String>,
    framework_routes: FrameworkRoutes,
}

/// What a host's root screen serves beside its own screens: the `root-screen` `framework-routes` attribute
/// (plans/Edge-Node2.md §4.2, docs/Run-Deploy.md "Several sites by host name").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameworkRoutes {
    /// Not set, as Moqui Java: the root's own screens. The shared static files (`/js`, `/css`, `/libs`, `/images`) come
    /// from the default root when the root's directory has no such file, and the routes outside the screens (the token
    /// routes, the servlets, the sockets) answer as on every host.
    #[default]
    Own,
    /// `include`: the root's own screens first, then the default root's for a first path name the root does not have:
    /// the shell and its transitions, sign-in, the apps, `/rest`, the error screens and the files beside it.
    Include,
    /// `refuse`: the root's own screens and the shared static files only. The token routes, the servlets other than
    /// the screens' and the host's own routes (the sockets, `/dav`, `/health`, `/metrics` and the rest) are Not Found on
    /// the host, as on a public site's restricted host.
    Refuse,
}

impl FrameworkRoutes {
    fn of(text: &str) -> Self {
        match text.trim() {
            "include" => Self::Include,
            "refuse" => Self::Refuse,
            other => {
                if !other.is_empty() {
                    tracing::error!("root-screen framework-routes=\"{other}\" is not include or refuse; using neither");
                }
                Self::Own
            }
        }
    }
}

/// The root screen a host gets ([`root_for_host`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRoot {
    pub location: String,
    pub framework_routes: FrameworkRoutes,
    /// The `.*` fallback answered: no other pattern named the host.
    pub fallback: bool,
}

fn root_screens(factory: &ExecutionContextFactory) -> std::sync::Arc<RootScreens> {
    factory.conf_derived(|factory| {
        let conf = match MNode::parse_shared("moqui-conf", &factory.conf_text()) {
            Ok(conf) => conf,
            Err(err) => return RootScreens(Err(err.to_string())),
        };
        let mut webapps = Vec::new();
        if let Some(list) = conf.first_named("webapp-list") {
            for webapp in list.named_iter("webapp") {
                let roots = webapp
                    .children_named("root-screen")
                    .iter()
                    .map(|root| RootEntry {
                        host: root.attribute("host").unwrap_or_default(),
                        location: root.attribute("location"),
                        framework_routes: FrameworkRoutes::of(&root.attribute("framework-routes").unwrap_or_default()),
                    })
                    .collect();
                webapps.push((webapp.attribute("name").unwrap_or_default(), roots));
            }
        }
        RootScreens(Ok(webapps))
    })
}

/// The `root-screen` locations of every `webapp` in the conf, in conf order, each once (two hosts may share a root).
pub fn all_root_screen_locations(factory: &ExecutionContextFactory) -> Vec<String> {
    let Ok(webapps) = &root_screens(factory).0 else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for location in webapps
        .iter()
        .flat_map(|(_, roots)| roots.iter().filter_map(|root| root.location.clone()))
    {
        if !location.is_empty() && !out.contains(&location) {
            out.push(location);
        }
    }
    out
}

/// The `root-screen` location for `host` in webapp `webapp_name`.
///
/// The first host regex other than `.*` that matches the whole host (port stripped) wins. `.*` is the fallback
/// even when it is listed first. Java's `ScreenFacadeImpl.rootScreenFromHost`. Every request asks, so the conf is read
/// once per factory ([`root_screens`]), not here.
///
/// # Errors
///
/// Returns an error when the webapp is missing, no pattern matches, or a host regex is invalid.
pub fn root_screen_for_host(factory: &ExecutionContextFactory, host: &str, webapp_name: &str) -> Result<String, Error> {
    root_for_host(factory, host, webapp_name).map(|root| root.location)
}

/// [`root_screen_for_host`] with the root's `framework-routes` and whether the `.*` fallback answered.
///
/// # Errors
///
/// As [`root_screen_for_host`].
pub fn root_for_host(factory: &ExecutionContextFactory, host: &str, webapp_name: &str) -> Result<HostRoot, Error> {
    // a host name is the same in any case and with its root's trailing dot ("Forum.Example.org." is forum.example.org)
    let host = crate::web::host_without_port(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let host = host.as_str();
    let screens = root_screens(factory);
    let webapps = screens.0.as_ref().map_err(|err| Error::new(err.clone()))?;
    let Some((_, roots)) = webapps.iter().find(|(name, _)| name == webapp_name) else {
        return Err(Error::new(format!("Could not find webapp {webapp_name}")));
    };
    let not_found = || Error::new(format!("Could not find root screen for host: {host}"));
    let mut wildcard = None;
    for root in roots {
        if root.host == ".*" {
            wildcard = Some(root);
            continue;
        }
        if root.host.is_empty() {
            continue;
        }
        if moqui_lib::object::regex_full_match(host, &root.host)? {
            let location = root
                .location
                .clone()
                .filter(|text| !text.is_empty())
                .ok_or_else(not_found)?;
            return Ok(HostRoot {
                location,
                framework_routes: root.framework_routes,
                fallback: false,
            });
        }
    }
    let root = wildcard.ok_or_else(not_found)?;
    let location = root
        .location
        .clone()
        .filter(|text| !text.is_empty())
        .ok_or_else(not_found)?;
    Ok(HostRoot {
        location,
        framework_routes: root.framework_routes,
        fallback: true,
    })
}

/// The `webroot` webapp's default root, its `.*` `root-screen` (`webroot.xml` in the default conf): what a wrapper
/// root falls back to for the shared static files, the error screens and, with `framework-routes="include"`, the
/// framework's screens. [`super::ROOT_SCREEN`] when the conf names none.
pub fn default_root(factory: &ExecutionContextFactory) -> String {
    let screens = root_screens(factory);
    screens
        .0
        .as_ref()
        .ok()
        .and_then(|webapps| webapps.iter().find(|(name, _)| name == "webroot"))
        .and_then(|(_, roots)| roots.iter().find(|root| root.host == ".*"))
        .and_then(|root| root.location.clone())
        .filter(|location| !location.is_empty())
        .unwrap_or_else(|| super::ROOT_SCREEN.to_string())
}

/// The host a link made in a request to `host` (a `Host` header) may use ([`crate::web::webapp_root_url`]): the header
/// as a plain DNS name, its `:port` (digits only) left off, a trailing dot trimmed and in lower case, when a
/// `root-screen` pattern of `webapp_name` other than `.*` names it and that root does not refuse the framework's routes
/// (a link from it is often to the apps, which such a host does not serve). None for anything else: a header that is
/// not `name` or `name:digits` (`shop.example:@evil.test`), a name with any character but letters, digits, `-` and `.`,
/// so a broad pattern such as `.*\.example\.com` never makes `evil.test/x.example.com` a link's host, and a host no
/// valid pattern names. The value returned is the one checked.
pub fn link_host(factory: &ExecutionContextFactory, host: &str, webapp_name: &str) -> Option<String> {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        Some(_) => return None,
        None => host,
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    if !plain {
        return None;
    }
    root_for_host(factory, &name, webapp_name)
        .is_ok_and(|root| !root.fallback && root.framework_routes != FrameworkRoutes::Refuse)
        .then_some(name)
}

/// Whether the `webroot` root screen of `host` (a `Host` header) says `framework-routes="refuse"`: the host serves its
/// root's screens and the shared static files, and none of the framework's other routes ([`FrameworkRoutes::Refuse`]).
/// The host's own routes (the sockets, `/dav`, `/health`...) ask this before they answer. A conf whose root screens
/// cannot be read (a host pattern that is not a valid regex) refuses on every host: the check guards a security
/// boundary. A conf with no root screen for the host refuses nothing.
pub fn host_refuses_framework_routes(factory: &ExecutionContextFactory, host: &str) -> bool {
    match root_for_host(factory, host, "webroot") {
        Ok(root) => root.framework_routes == FrameworkRoutes::Refuse,
        // no root screen for the host (a conf with none) refuses nothing; one that cannot be read refuses
        Err(err) if unreadable_roots(factory, "webroot") => {
            tracing::error!("Root screen of host {host}: {err}; refusing the framework's routes there");
            true
        }
        Err(_) => false,
    }
}

/// Whether the `root-screen` elements of `webapp_name` cannot be read: the conf does not parse, or a host pattern is
/// not a valid regex.
fn unreadable_roots(factory: &ExecutionContextFactory, webapp_name: &str) -> bool {
    let screens = root_screens(factory);
    let Ok(webapps) = &screens.0 else {
        return true;
    };
    webapps
        .iter()
        .filter(|(name, _)| name == webapp_name)
        .flat_map(|(_, roots)| roots.iter())
        .any(|root| {
            !root.host.is_empty() && root.host != ".*" && moqui_lib::object::regex_full_match("", &root.host).is_err()
        })
}

#[derive(Default)]
struct Totals {
    subscreens: i64,
    non_placeholder: i64,
    forms: i64,
    trees: i64,
    sections: i64,
    transitions: i64,
    with_actions: i64,
}

struct Info {
    row: moqui_lib::object::ValueMap,
    level: usize,
    own: Totals,
    all: Totals,
    children: Vec<Info>,
}

/// One row for each screen under `root_location`, down to `levels`, in menu order. Each row has the counts of the
/// screen (`forms`, `transitions`, ...) and of all the screens under it (`allSubscreensForms`, ...), its `level`, its
/// `name`, and `ssi`, the subscreens item that mounts it (null for the root).
///
/// # Errors
///
/// Returns an error when the root screen does not load. A subscreen that does not load is left out, as Java logs it.
pub fn screen_info_list(
    factory: &ExecutionContextFactory,
    root_location: &str,
    levels: usize,
) -> Result<Vec<moqui_lib::object::ValueMap>, Error> {
    let facade = ScreenFacade::of(factory);
    let root = facade.screen_def(factory, root_location)?;
    let mut path = vec![root.location.clone()];
    let info = walk(&facade, factory, &root, None, 0, &mut path);
    let mut out = Vec::new();
    flatten(info, levels, &mut out);
    Ok(out)
}

/// Resolve a transition's mounted path and backing service without exposing implementation objects.
/// The caller chooses the root screen; this operation only reads metadata, never grants execution access.
pub fn transition_info(
    factory: &ExecutionContextFactory,
    root: &str,
    location: &str,
    name: &str,
    method: &str,
) -> Result<Value, Error> {
    let facade = ScreenFacade::of(factory);
    let def = facade.screen_def(factory, location)?;
    let transition = def
        .transition(name, Some(method))
        .ok_or_else(|| Error::new("Screen transition not found"))?;
    fn find_paths(info: &Info, target: &str, path: &str, paths: &mut Vec<String>) {
        if info.row["location"].as_str() == Some(target) {
            paths.push(path.to_string());
        } else if paths.len() < 2 {
            for child in &info.children {
                find_paths(
                    child,
                    target,
                    &format!("{path}/{}", child.row["name"].as_str().unwrap_or_default()),
                    paths,
                );
            }
        }
    }
    // Reuse the existing bounded tree walk, including cycle and unreadable-screen handling.
    let root = facade.screen_def(factory, root)?;
    let tree = walk(&facade, factory, &root, None, 0, &mut vec![root.location.clone()]);
    let mut paths = Vec::new();
    find_paths(&tree, &def.location, "", &mut paths);
    let [path] = paths.as_slice() else {
        return Err(Error::new(if paths.is_empty() {
            "Screen is not mounted under the selected root"
        } else {
            "Screen is mounted more than once under the selected root"
        }));
    };
    // The implicit `actions` transition runs the screen's actions, so it takes the screen's parameters.
    let declared = if transition.implicit == super::definition::Implicit::Actions {
        &def.parameters
    } else {
        &transition.parameters
    };
    // `hasDefault`: the parameter's `from` or `value` gives it a value when the request has none.
    let parameters = declared
        .iter()
        .map(|p| (&p.name, p.required, p.from.is_some() || p.value.is_some()))
        .chain(transition.path_parameters.iter().map(|name| (name, true, false)))
        .map(|(name, required, has_default)| {
            (
                name.clone(),
                Value::Map(moqui_lib::object::ValueMap::from_iter([
                    ("name".to_string(), Value::string(name)),
                    ("required".to_string(), Value::Bool(required)),
                    ("hasDefault".to_string(), Value::Bool(has_default)),
                ])),
            )
        })
        .collect();
    Ok(Value::Map(moqui_lib::object::ValueMap::from_iter([
        ("location".to_string(), Value::string(location)),
        ("name".to_string(), Value::string(name)),
        ("method".to_string(), Value::string(method.to_ascii_lowercase())),
        ("path".to_string(), Value::string(format!("{path}/{name}"))),
        (
            "serviceName".to_string(),
            transition.single_service.as_deref().map_or(Value::Null, Value::string),
        ),
        (
            "pathParameters".to_string(),
            Value::List(transition.path_parameters.iter().map(Value::string).collect()),
        ),
        ("parameterMap".to_string(), Value::Map(parameters)),
    ])))
}

fn walk(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &ScreenDef,
    item: Option<(&str, String, Option<i64>)>,
    level: usize,
    path: &mut Vec<String>,
) -> Info {
    let own = Totals {
        subscreens: count(def.subscreens.items.len()),
        non_placeholder: 0,
        forms: count(def.forms.len()),
        trees: count(def.trees.len()),
        sections: count(def.sections.len()),
        transitions: count(def.transitions.len()),
        with_actions: count(
            def.transitions
                .iter()
                .filter(|transition| transition.service_actions.is_some() || transition.actions.is_some())
                .count(),
        ),
    };
    let is_non_placeholder = own.forms > 0 || own.sections > 0 || own.transitions > 4;
    let mut items: Vec<_> = def.subscreens.items.iter().collect();
    items.sort_by(|left, right| left.name.cmp(&right.name));
    // Java sorts by menu index, a missing index first, keeping the name order otherwise.
    items.sort_by_key(|item| item.menu_index);
    let mut children = Vec::new();
    if level < MAX_DEPTH {
        for child in items {
            if path.contains(&child.location) {
                continue;
            }
            let Ok(child_def) = facade.screen_def(factory, &child.location) else {
                tracing::info!(
                    "While getting ScreenInfo screen not found for {} at: {}",
                    child.name,
                    child.location
                );
                continue;
            };
            path.push(child.location.clone());
            children.push(walk(
                facade,
                factory,
                &child_def,
                Some((child.name.as_str(), child.menu_title.clone(), child.menu_index)),
                level + 1,
                path,
            ));
            path.pop();
        }
    }
    let mut all = Totals::default();
    for child in &children {
        all.subscreens += 1 + child.all.subscreens;
        all.non_placeholder += i64::from(child_is_non_placeholder(child)) + child.all.non_placeholder;
        all.forms += child.own.forms + child.all.forms;
        all.trees += child.own.trees + child.all.trees;
        all.sections += child.own.sections + child.all.sections;
        all.transitions += child.own.transitions + child.all.transitions;
        all.with_actions += child.own.with_actions + child.all.with_actions;
    }
    let mut row = IndexMap::default();
    let name = item
        .as_ref()
        .map_or_else(|| def.screen_name.clone(), |(name, _, _)| (*name).to_string());
    row.insert("name".to_string(), Value::string(&name));
    row.insert("level".to_string(), Value::I64(count(level)));
    row.insert("location".to_string(), Value::string(&def.location));
    row.insert(
        "ssi".to_string(),
        match &item {
            Some((name, title, index)) => {
                let mut ssi = IndexMap::default();
                ssi.insert("name".to_string(), Value::string(*name));
                ssi.insert("menuTitle".to_string(), Value::string(title));
                ssi.insert("menuIndex".to_string(), index.map_or(Value::Null, Value::I64));
                Value::Map(ssi)
            }
            None => Value::Null,
        },
    );
    row.insert("isNonPlaceholder".to_string(), Value::Bool(is_non_placeholder));
    Info {
        row,
        level,
        own,
        all,
        children,
    }
}

fn child_is_non_placeholder(info: &Info) -> bool {
    info.row.get("isNonPlaceholder") == Some(&Value::Bool(true))
}

fn flatten(info: Info, levels: usize, out: &mut Vec<moqui_lib::object::ValueMap>) {
    let mut row = info.row;
    let pairs = [
        ("subscreens", info.own.subscreens),
        ("allSubscreens", info.all.subscreens),
        // Java does not count the non-placeholder subscreens of one level, only of all levels.
        ("subscreensNonPlaceholder", info.own.non_placeholder),
        ("allSubscreensNonPlaceholder", info.all.non_placeholder),
        ("forms", info.own.forms),
        ("allSubscreensForms", info.all.forms),
        ("trees", info.own.trees),
        ("allSubscreensTrees", info.all.trees),
        ("sections", info.own.sections),
        ("allSubscreensSections", info.all.sections),
        ("transitions", info.own.transitions),
        ("allSubscreensTransitions", info.all.transitions),
        ("transitionsWithActions", info.own.with_actions),
        ("allSubscreensTransitionsWithActions", info.all.with_actions),
    ];
    for (name, number) in pairs {
        row.insert(name.to_string(), Value::I64(number));
    }
    out.push(row);
    // `addChildrenToList`: the root lists its children, and each lists its own while the list goes deeper.
    for child in info.children {
        if child.level <= levels.max(1) {
            flatten(child, levels, out);
        }
    }
}

fn count(number: usize) -> i64 {
    i64::try_from(number).unwrap_or(i64::MAX)
}

/// The subscreens of the screen at `location`, one level, as the screen builder's tree shows them (plans/design-dev-
/// aide.md 11.1): each with its name, location, menu title and index, whether it is in the menu and the default (`isDefault`), how it
/// is mounted (`directory`, `item <file>:<line>`, `conf <file>:<line>`, `row`), its render modes and authentication,
/// and how many subscreens it has. A subscreen that does not load is listed with its error.
///
/// # Errors
///
/// Returns an error when the screen at `location` does not load.
pub fn screen_children(
    factory: &ExecutionContextFactory,
    location: &str,
) -> Result<Vec<moqui_lib::object::ValueMap>, Error> {
    let facade = ScreenFacade::of(factory);
    let def = facade.screen_def(factory, location)?;
    let mut items: Vec<_> = def.subscreens.items.iter().collect();
    items.sort_by(|left, right| left.name.cmp(&right.name));
    items.sort_by_key(|item| item.menu_index.unwrap_or(i64::MAX));
    Ok(items
        .into_iter()
        .map(|item| {
            let mut row = IndexMap::default();
            row.insert("name".to_string(), Value::string(&item.name));
            row.insert("location".to_string(), Value::string(&item.location));
            row.insert("menuTitle".to_string(), Value::string(&item.menu_title));
            row.insert("menuIndex".to_string(), item.menu_index.map_or(Value::Null, Value::I64));
            row.insert("menuInclude".to_string(), Value::Bool(item.menu_include));
            row.insert(
                "menuIncludeWhen".to_string(),
                item.menu_include_when.as_deref().map_or(Value::Null, Value::string),
            );
            row.insert(
                "isDefault".to_string(),
                Value::Bool(def.subscreens.default_item.as_deref() == Some(item.name.as_str())),
            );
            row.insert("mountedBy".to_string(), Value::string(&item.mounted_by));
            match facade.screen_def(factory, &item.location) {
                Ok(child) => {
                    row.insert(
                        "renderModes".to_string(),
                        Value::List(child.render_modes.iter().map(Value::string).collect()),
                    );
                    row.insert(
                        "authentication".to_string(),
                        Value::string(format!("{:?}", child.authentication)),
                    );
                    row.insert(
                        "subscreenCount".to_string(),
                        Value::I64(i64::try_from(child.subscreens.items.len()).unwrap_or(0)),
                    );
                }
                Err(err) => {
                    row.insert("error".to_string(), Value::string(err.to_string()));
                }
            }
            row
        })
        .collect())
}
