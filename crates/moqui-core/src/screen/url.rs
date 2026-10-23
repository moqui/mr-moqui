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

//! `ScreenUrlInfo`: a URL path resolved against the screen tree.
//!
//! The walk follows `ScreenUrlInfo.initUrl`. Each path name is a subscreen, a transition, or a
//! screen with a render-mode extension. After the last name, `default-item` and
//! `conditional-default` keep descending. Not ported: `ScreenPathAlias` and `SubscreensDefault`
//! rows, and the `//` sparse path. A name that is not a subscreen or a transition but is a file under the
//! screen's directory ends the walk at that file, as in Java ([`static_files`](super::static_files)).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::cache::{canonical_render_mode, ScreenFacade};
use super::definition::{ParameterDef, ScreenDef, TransitionDef};
use crate::context::{Error, ErrorKind, ExecutionContextFactory};

#[derive(Clone, Debug)]
pub struct ScreenUrlInfo {
    /// Every screen from the root to the target, including defaults.
    pub path_defs: Vec<Arc<ScreenDef>>,
    /// Index in `path_defs` of the first screen that renders. The ones before it only run actions.
    pub render_path_difference: usize,
    /// Path names that led to the target, with defaults appended and no-sub-path names inserted.
    pub full_path: Vec<String>,
    /// Path names before any default was followed.
    pub minimal_path: Vec<String>,
    /// Screen names only, up to a transition. A transition response resolves from here.
    pub pre_transition_path: Vec<String>,
    /// Names after the target screen or transition.
    pub extra_path: Vec<String>,
    pub target_screen: Arc<ScreenDef>,
    /// Actual transition name, without a render-mode extension.
    pub target_transition_name: Option<String>,
    pub target_transition_extension: Option<String>,
    /// Canonical render mode from a screen name extension, such as `html-print` for `.xsl-fo`.
    pub target_render_mode: Option<String>,
    pub last_standalone: i32,
    pub always_use_full_path: bool,
    pub begin_transaction: bool,
    pub transaction_timeout: Option<i64>,
    /// Screen `parameter` entries along the path, first definition winning.
    pub parameter_items: BTreeMap<String, ParameterDef>,
    pub menu_image: Option<(String, String)>,
    /// False when a `conditional-default` ran, so the result must not be reused.
    pub reusable: bool,
    /// Parameters from a `?name=value` tail on the path.
    pub inline_parameters: BTreeMap<String, String>,
    /// The location of a static file under a screen, when the path ends at one.
    pub file_resource: Option<String>,
}

impl ScreenUrlInfo {
    /// Resolve a path such as `/a/b.csv?x=1`. A leading slash is not required.
    ///
    /// # Errors
    ///
    /// Returns `ErrorKind::NotFound` when a name matches no subscreen or transition, or an error
    /// from loading a screen or running a `conditional-default`.
    pub fn parse(
        factory: &ExecutionContextFactory,
        root_location: &str,
        path: &str,
        last_standalone: i32,
    ) -> Result<Arc<Self>, Error> {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let path = path.split('#').next().unwrap_or(path);
        let segments = split_path(path);
        let mut info = Self::resolve(factory, root_location, &segments, last_standalone)?;
        let pairs = crate::web::form_pairs(&query.replace("&amp;", "&"));
        if !pairs.is_empty() {
            // Inline parameters belong to this path only, so the shared cached walk is copied first.
            let own = Arc::make_mut(&mut info);
            for (name, value) in pairs {
                own.inline_parameters.insert(name, value);
            }
        }
        Ok(info)
    }

    /// Resolve already-split path names. A reusable walk is shared from the cache, not copied.
    ///
    /// # Errors
    ///
    /// Same as [`ScreenUrlInfo::parse`].
    pub fn resolve(
        factory: &ExecutionContextFactory,
        root_location: &str,
        segments: &[String],
        last_standalone: i32,
    ) -> Result<Arc<Self>, Error> {
        let facade = ScreenFacade::of(factory);
        // A path that took no conditional default resolves the same way every time, as Java's `screen.url` cache.
        let key = format!("{root_location}\u{1}{}\u{1}{last_standalone}", segments.join("/"));
        if let Some(cached) = facade.cached_url(factory, &key) {
            return Ok(cached);
        }
        let root = facade.screen_def(factory, root_location)?;
        let mut walk = Walk {
            facade: &facade,
            factory,
            render_start: 0,
            last_standalone,
            info: Self {
                path_defs: vec![Arc::clone(&root)],
                render_path_difference: 0,
                full_path: segments.to_vec(),
                minimal_path: Vec::new(),
                pre_transition_path: Vec::new(),
                extra_path: segments.to_vec(),
                target_screen: Arc::clone(&root),
                target_transition_name: None,
                target_transition_extension: None,
                target_render_mode: None,
                last_standalone,
                always_use_full_path: false,
                begin_transaction: root.begin_transaction,
                transaction_timeout: root.transaction_timeout,
                parameter_items: BTreeMap::new(),
                menu_image: None,
                reusable: true,
                inline_parameters: BTreeMap::new(),
                file_resource: None,
            },
        };
        walk.run(root)?;
        let info = Arc::new(walk.info);
        if info.reusable {
            facade.put_url(key, Arc::clone(&info));
        }
        Ok(info)
    }

    /// The transition the path ends on, from the target screen.
    pub fn transition(&self, method: Option<&str>) -> Option<&TransitionDef> {
        let name = self.target_transition_name.as_deref()?;
        self.target_screen.transition(name, method)
    }

    /// Screens that render, from `render_path_difference`.
    pub fn render_defs(&self) -> &[Arc<ScreenDef>] {
        &self.path_defs[self.render_path_difference.min(self.path_defs.len())..]
    }

    /// The path as a URL path, with default names.
    pub fn full_path_url(&self) -> String {
        join_path(&self.full_path)
    }

    /// The path as a URL path without default names, or with them when `always-use-full-path`.
    pub fn minimal_path_url(&self) -> String {
        if self.always_use_full_path {
            self.full_path_url()
        } else {
            join_path(&self.minimal_path)
        }
    }
}

struct Walk<'a> {
    facade: &'a ScreenFacade,
    factory: &'a ExecutionContextFactory,
    render_start: usize,
    last_standalone: i32,
    info: ScreenUrlInfo,
}

impl Walk<'_> {
    fn run(&mut self, root: Arc<ScreenDef>) -> Result<(), Error> {
        let mut last = root;
        let mut index = 0;
        while index < self.info.full_path.len() {
            let mut name = self.info.full_path[index].clone();
            let mut extension = None;
            let mut before_dot = None;
            if let Some(dot) = name.find('.').filter(|dot| *dot > 0) {
                let candidate = &name[dot + 1..];
                if self.facade.is_render_mode_valid(candidate) {
                    extension = Some(candidate.to_string());
                    before_dot = Some(name[..dot].to_string());
                }
            }

            // A no-sub-path screen wraps a child that exists under it. The wrapper joins the path.
            let wrappers: Vec<_> = last
                .subscreens
                .items
                .iter()
                .filter(|item| item.no_sub_path)
                .cloned()
                .collect();
            for wrapper in wrappers {
                let Ok(wrap_def) = self.facade.screen_def(self.factory, &wrapper.location) else {
                    continue;
                };
                let found = [Some(name.as_str()), before_dot.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|candidate| self.has_child(&wrap_def, candidate));
                if found {
                    self.enter(&wrap_def);
                    self.info.full_path.insert(index, wrapper.name.clone());
                    index += 1;
                    last = wrap_def;
                    break;
                }
            }

            let mut item = self.screen_item(&last, &name);
            if item.is_none() {
                if last.has_transition(&name) {
                    self.info.extra_path.remove(0);
                    self.info.target_transition_name = Some(name);
                    break;
                }
                if let Some(file) =
                    super::static_files::file_under(self.factory, self.facade, &last.location, &self.info.extra_path)
                {
                    self.info.file_resource = Some(file);
                    break;
                }
                if let Some(before) = &before_dot {
                    item = self.screen_item(&last, before);
                    if item.is_some() {
                        let mode = canonical_render_mode(extension.as_deref().unwrap_or("")).to_string();
                        if self.facade.is_always_standalone(&mode) {
                            self.last_standalone = 1;
                        }
                        self.info.target_render_mode = Some(mode);
                        self.info.full_path[index].clone_from(before);
                        name.clone_from(before);
                    } else if last.has_transition(before) {
                        self.info.extra_path.remove(0);
                        self.info.target_transition_name = Some(before.clone());
                        self.info.target_transition_extension = extension;
                        break;
                    }
                }
                if item.is_none() {
                    if last.allow_extra_path {
                        break;
                    }
                    return Err(not_found(&name, &last));
                }
            }
            let location = item.map(|item| item.location).unwrap_or_default();
            let next = self.facade.screen_def(self.factory, &location)?;
            self.info.always_use_full_path |= last_always_full(&next);
            for parameter in &next.parameters {
                self.info
                    .parameter_items
                    .entry(parameter.name.clone())
                    .or_insert_with(|| parameter.clone());
            }
            self.enter(&next);
            self.info.pre_transition_path.push(name);
            self.info.extra_path.remove(0);
            last = next;
            index += 1;
        }

        self.info.minimal_path = self.info.full_path.clone();
        let mut default_count = 0;
        while self.info.target_render_mode.is_none()
            && self.info.target_transition_name.is_none()
            && self.info.file_resource.is_none()
            && last.subscreens.default_item.is_some()
        {
            if last.subscreens.always_use_full_path {
                self.info.always_use_full_path = true;
            }
            let mut chosen = self.subscreens_default(&last)?;
            for (condition, item) in &last.subscreens.conditional_defaults {
                self.info.reusable = false;
                if crate::script::eval_condition(condition)? {
                    chosen = Some(item.clone());
                    break;
                }
            }
            let Some(name) = chosen.or_else(|| last.subscreens.default_item.clone()) else {
                break;
            };
            let Some(item) = last.subscreens.get(&name) else {
                if last.has_transition(&name) {
                    self.info.target_transition_name = Some(name.clone());
                    self.info.full_path.push(name);
                    break;
                }
                return Err(not_found(&name, &last));
            };
            let location = item.location.clone();
            let next = self.facade.screen_def(self.factory, &location)?;
            self.enter(&next);
            self.info.full_path.push(name.clone());
            self.info.pre_transition_path.push(name);
            default_count += 1;
            last = next;
        }

        self.info.target_screen = Arc::clone(&last);
        if self.last_standalone > 0 {
            let limit = usize::try_from(self.last_standalone).unwrap_or(usize::MAX);
            while self.render_len() > limit {
                self.render_start += 1;
            }
        }
        self.info.last_standalone = self.last_standalone;
        self.info.render_path_difference = self.render_start;
        // Only the explicit path picks the menu image, so a default subscreen cannot override it.
        let render = self.info.render_defs();
        let limit = render.len().saturating_sub(default_count + 1);
        let mut image = None;
        for (position, def) in render.iter().enumerate() {
            if let Some(menu_image) = &def.menu_image {
                image = Some((menu_image.clone(), def.menu_image_type.clone()));
            }
            if position >= limit && image.is_some() {
                break;
            }
        }
        self.info.menu_image = image;
        Ok(())
    }

    fn render_len(&self) -> usize {
        self.info.path_defs.len().saturating_sub(self.render_start)
    }

    /// Add `next` to the path and apply the standalone rules to what renders.
    fn enter(&mut self, next: &Arc<ScreenDef>) {
        if next.standalone {
            self.render_start = self.info.path_defs.len();
        } else {
            while self.last_standalone < 0
                && usize::try_from(-i64::from(self.last_standalone)).unwrap_or(0) > self.render_start
                && self.render_len() > 0
            {
                self.render_start += 1;
            }
        }
        self.info.begin_transaction |= next.begin_transaction;
        if let Some(timeout) = next.transaction_timeout {
            if self.info.transaction_timeout.is_none_or(|existing| timeout > existing) {
                self.info.transaction_timeout = Some(timeout);
            }
        }
        self.info.path_defs.push(Arc::clone(next));
    }

    fn screen_item(&self, from: &ScreenDef, name: &str) -> Option<super::definition::SubscreensItem> {
        let item = from.subscreens.get(name)?;
        self.facade
            .is_screen(self.factory, &item.location)
            .then(|| item.clone())
    }

    /// The `moqui.screen.SubscreensDefault` rows of a screen, in order, the last whose condition holds (Java's
    /// ScreenUrlInfo). A conditional row makes the path one that cannot be cached.
    fn subscreens_default(&mut self, screen: &ScreenDef) -> Result<Option<String>, Error> {
        let Some(entity) = self.factory.entity() else {
            return Ok(None);
        };
        let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
        let rows = match entity.find("moqui.screen.SubscreensDefault").and_then(|find| {
            find.eq("screenLocation", moqui_lib::object::Value::string(&screen.location))
                .order_by("defaultSeqId", false)
                .use_cache(true)
                .list()
        }) {
            Ok(rows) => rows,
            // A database without the screen tables has no rows.
            Err(_) if super::definition::screen_entity_absent(&entity, "moqui.screen.SubscreensDefault") => {
                return Ok(None)
            }
            // Any other failure is not "no rows", so this path must be resolved again.
            Err(err) => {
                tracing::warn!("could not read SubscreensDefault rows for {}: {err}", screen.location);
                self.info.reusable = false;
                return Ok(None);
            }
        };
        let mut chosen = None;
        for row in rows.values() {
            let text = |name: &str| {
                row.get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            let condition = text("conditionExpression");
            if !condition.is_empty() {
                self.info.reusable = false;
                if !crate::script::eval_condition(&condition)? {
                    continue;
                }
            }
            let name = text("subscreenName");
            if !name.is_empty() {
                chosen = Some(name);
            }
        }
        Ok(chosen)
    }

    fn has_child(&self, screen: &ScreenDef, name: &str) -> bool {
        screen.has_transition(name) || self.screen_item(screen, name).is_some()
    }
}

fn last_always_full(screen: &ScreenDef) -> bool {
    screen.subscreens.always_use_full_path
}

fn not_found(name: &str, under: &ScreenDef) -> Error {
    Error::of_kind(
        ErrorKind::NotFound,
        format!("screen path not found: {name} under {}", under.location),
    )
    .with_missing_path(name)
}

pub(crate) fn split_path(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_path(names: &[String]) -> String {
    let mut out = String::new();
    for name in names {
        out.push('/');
        out.push_str(name);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// A `lastStandalone` value: an integer, `true` for 1, anything else `default`.
pub fn parse_last_standalone(text: &str, default: i32) -> i32 {
    let text = text.trim();
    if text.is_empty() {
        return default;
    }
    if text == "true" {
        return 1;
    }
    text.parse().unwrap_or(default)
}

/// Resolve a transition response `screen-path` against the screens the transition sits under.
/// A leading `/` starts at the root. `.` is the current screen and `..` the parent.
pub fn resolve_screen_path(from: &[String], url: &str) -> Vec<String> {
    let mut names: Vec<String> = if url.starts_with('/') {
        Vec::new()
    } else {
        from.to_vec()
    };
    for part in url.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                names.pop();
            }
            other => names.push(other.to_string()),
        }
    }
    names
}

/// Resolve a transition response or link `screen-path` from the root screen at `root_location`, as Java's
/// `ScreenUrlInfo.parseSubScreenPath`: a leading `//` finds the screen by name anywhere under the root
/// ([`find_subscreen_path`]), so `//marble/Shipment` is `apps/marble/Shipment`. Other paths are [`resolve_screen_path`].
/// A `//` path that finds nothing is left from the root, where it does not resolve.
pub fn resolve_screen_path_at(
    factory: &ExecutionContextFactory,
    root_location: &str,
    from: &[String],
    url: &str,
) -> Vec<String> {
    let Some(rest) = url.strip_prefix("//") else {
        return resolve_screen_path(from, url);
    };
    let names = resolve_screen_path(&[], rest);
    let facade = ScreenFacade::of(factory);
    let key = format!("{root_location}\u{1}{url}");
    let found = facade.cached_find_path(&key).unwrap_or_else(|| {
        let found = facade
            .screen_def(factory, root_location)
            .ok()
            .and_then(|root| find_subscreen_path(&facade, factory, &root, &names));
        facade.put_find_path(key, found.clone());
        found
    });
    found.unwrap_or(names)
}

/// Java's `ScreenDefinition.findSubscreenPath`: the full path under `screen` for the sparse `names`. The first name
/// is a subscreen of `screen`, a transition of it, a subscreen of one of its subscreens, or else is searched for
/// depth first under each subscreen; the rest must follow it directly.
fn find_subscreen_path(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    screen: &ScreenDef,
    names: &[String],
) -> Option<Vec<String>> {
    let (first, rest) = names.split_first()?;
    let child = |item: &super::definition::SubscreensItem| facade.screen_def(factory, &item.location).ok();
    // The rest of the path under `item`, with `prefix` before it.
    let below = |item: &super::definition::SubscreensItem, prefix: Vec<String>| {
        if rest.is_empty() {
            return Some(prefix);
        }
        let mut path = find_subscreen_path(facade, factory, &*child(item)?, rest)?;
        path.splice(0..0, prefix);
        Some(path)
    };
    if let Some(item) = screen.subscreens.get(first) {
        return below(item, vec![first.clone()]);
    }
    if screen.has_transition(first) {
        return Some(names.to_vec());
    }
    let children: Vec<_> = screen
        .subscreens
        .items
        .iter()
        .filter_map(|item| Some((item, child(item)?)))
        .collect();
    for (item, def) in &children {
        if let Some(grandchild) = def.subscreens.get(first) {
            return below(grandchild, vec![item.name.clone(), first.clone()]);
        }
    }
    for (item, def) in &children {
        if let Some(mut path) = find_subscreen_path(facade, factory, def, names) {
            path.insert(0, item.name.clone());
            return Some(path);
        }
    }
    super::static_files::file_under(factory, facade, &screen.location, names).map(|_| names.to_vec())
}

/// The URL path for `names`, percent-encoding what a path name cannot hold.
pub fn encode_path(names: &[String]) -> String {
    let mut out = String::new();
    for name in names {
        out.push('/');
        for byte in name.bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~:@!$'()*+,;=".contains(&byte) {
                out.push(char::from(byte));
            } else {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}
