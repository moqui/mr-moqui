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
//! screen with a render-mode extension. After the last name, `default-item`, `conditional-default`,
//! and `SubscreensDefault` rows keep descending. `ScreenPathAlias` rewrites the path first, and a
//! `//` path is a search by screen name. A name that is not a subscreen or a transition but is a
//! file under the screen's directory ends the walk at that file, as in Java
//! ([`static_files`](super::static_files)).

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::cache::{canonical_render_mode, ScreenFacade};
use super::definition::{ParameterDef, ScreenDef, TransitionDef};
use crate::context::{Error, ErrorKind, ExecutionContextFactory};

/// How deep `//` name search goes before it stops, the same cap as the screen-info walk. A screen that mounts an
/// ancestor must not hang the request.
const FIND_DEPTH: usize = 24;

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
        // A fragment is not part of the screen path. Java leaves it; a request path has none.
        let path = path.split('#').next().unwrap_or(path);
        let prepared = prepare_screen_path(factory, root_location, &[], path)?;
        let mut info = Self::resolve_with(factory, root_location, &prepared.names, last_standalone, prepared.cache)?;
        if !prepared.inline.is_empty() {
            // Inline parameters belong to this path only, so the shared cached walk is copied first.
            let own = Arc::make_mut(&mut info);
            for (name, value) in prepared.inline {
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
        Self::resolve_with(factory, root_location, segments, last_standalone, true)
    }

    /// As [`ScreenUrlInfo::resolve`], without storing the walk when `cache` is false.
    pub(crate) fn resolve_cached(
        factory: &ExecutionContextFactory,
        root_location: &str,
        segments: &[String],
        last_standalone: i32,
        cache: bool,
    ) -> Result<Arc<Self>, Error> {
        Self::resolve_with(factory, root_location, segments, last_standalone, cache)
    }

    /// As [`ScreenUrlInfo::resolve`]. `cache` is false when a `ScreenPathAlias` read failed, so that walk is not stored.
    fn resolve_with(
        factory: &ExecutionContextFactory,
        root_location: &str,
        segments: &[String],
        last_standalone: i32,
        cache: bool,
    ) -> Result<Arc<Self>, Error> {
        let facade = ScreenFacade::of(factory);
        // a render from overlaid text resolves its own path, with the overlaid definitions
        let cache = cache && !super::overlay::active();
        // A path that took no conditional default resolves the same way every time, as Java's `screen.url` cache.
        let key = format!("{root_location}\u{1}{}\u{1}{last_standalone}", segments.join("/"));
        if cache {
            if let Some(cached) = facade.cached_url(factory, &key) {
                return Ok(cached);
            }
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
        if cache && info.reusable {
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
                let found = self.has_child(&wrap_def, &name)
                    || super::static_files::file_under(
                        self.factory,
                        self.facade,
                        &wrap_def.location,
                        &self.info.extra_path,
                    )
                    .is_some()
                    || before_dot
                        .as_deref()
                        .is_some_and(|candidate| self.has_child(&wrap_def, candidate));
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
                // the screen's own choice, not a widget expression: the server-static check's wrapped `ec` is not for it
                let _real_ec = crate::script::WidgetWrap::set(false);
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
                if let Some(file) = super::static_files::file_under(
                    self.factory,
                    self.facade,
                    &last.location,
                    std::slice::from_ref(&name),
                ) {
                    self.info.file_resource = Some(file);
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

/// A screen path after `ScreenPathAlias`, `//` search, and cleanup. `cache` is false when the alias read failed.
#[derive(Clone, Debug)]
pub struct ScreenPath {
    pub names: Vec<String>,
    /// Parameters from a `?` query and from `~name=value` segments.
    pub inline: BTreeMap<String, String>,
    pub cache: bool,
}

/// Resolve `screen_path` the way Java's `parseSubScreenPath` does: alias, then `//` or cleanup of `.`, `..`, and `~`.
///
/// `from` is the active screen path. An empty `from` is a request. A `//` name that exists nowhere is
/// `ErrorKind::NotFound`.
///
/// # Errors
///
/// Returns `ErrorKind::NotFound` when a `//` search finds nothing, or an error from loading a screen during that
/// search.
pub fn prepare_screen_path(
    factory: &ExecutionContextFactory,
    root_location: &str,
    from: &[String],
    screen_path: &str,
) -> Result<ScreenPath, Error> {
    let mut screen_path = screen_path.to_string();
    let mut cache = true;
    match screen_path_alias(factory, &alias_key(from, &screen_path)) {
        Ok(Some(rewritten)) => screen_path = rewritten,
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(
                "could not read ScreenPathAlias for {}: {err}",
                alias_key(from, &screen_path)
            );
            cache = false;
        }
    }
    let mut inline = BTreeMap::new();
    // Java strips a query only when `?` is not the first character.
    if let Some(index) = screen_path.rfind('?').filter(|index| *index > 0) {
        let query = screen_path[index + 1..].replace("&amp;", "&");
        for (name, value) in crate::web::form_pairs(&query) {
            inline.insert(name, value);
        }
        screen_path.truncate(index);
    }
    if let Some(trimmed) = screen_path.strip_prefix("//") {
        let names = cleanup_path_names(trimmed.split('/'), &mut inline);
        let facade = ScreenFacade::of(factory);
        let key = format!("{root_location}\u{1}{screen_path}");
        let expanded = if let Some(cached) = facade.cached_sparse(factory, &key) {
            cached
        } else {
            let root = facade.screen_def(factory, root_location)?;
            let found = find_subscreen_path(&facade, factory, &root, &names, 0);
            facade.put_sparse(factory, key, found.clone());
            found
        };
        let Some(names) = expanded else {
            return Err(Error::of_kind(
                ErrorKind::NotFound,
                format!("screen path not found: {screen_path}"),
            ));
        };
        return Ok(ScreenPath { names, inline, cache });
    }
    let mut names = if screen_path.starts_with('/') {
        Vec::new()
    } else {
        from.to_vec()
    };
    names.extend(screen_path.split('/').map(str::to_string));
    Ok(ScreenPath {
        names: cleanup_path_names(names.iter().map(String::as_str), &mut inline),
        inline,
        cache,
    })
}

fn alias_key(from: &[String], screen_path: &str) -> String {
    if screen_path.starts_with('/') || from.is_empty() {
        return screen_path.to_string();
    }
    let mut key = String::new();
    for name in from {
        key.push('/');
        key.push_str(name);
    }
    if !screen_path.is_empty() {
        key.push('/');
        key.push_str(screen_path);
    }
    key
}

/// `moqui.screen.ScreenPathAlias` for `alias_path`: the `screenPath` of the newest row in the from/thru window.
/// `Ok(None)` is no row, or a database without the screen tables. An error is any other read failure.
fn screen_path_alias(factory: &ExecutionContextFactory, alias_path: &str) -> Result<Option<String>, Error> {
    let Some(entity) = factory.entity() else {
        return Ok(None);
    };
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let rows = match entity
        .find("moqui.screen.ScreenPathAlias")
        .and_then(|find| find.eq("aliasPath", Value::string(alias_path)).use_cache(true).list())
    {
        Ok(rows) => rows,
        Err(_) if super::definition::screen_entity_absent(&entity, "moqui.screen.ScreenPathAlias") => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let now = Value::Timestamp(jiff::Timestamp::now());
    let mut open = Vec::new();
    for row in rows.values() {
        let from = row.get("fromDate").cloned().unwrap_or(Value::Null);
        let thru = row.get("thruDate").cloned().unwrap_or(Value::Null);
        let started = is_empty(&from)
            || moqui_lib::object::compare_obj(&from, &now).is_ok_and(|order| order != std::cmp::Ordering::Greater);
        let still_open = is_empty(&thru)
            || moqui_lib::object::compare_obj(&thru, &now).is_ok_and(|order| order == std::cmp::Ordering::Greater);
        if !started || !still_open {
            continue;
        }
        let screen = row.get("screenPath").map(to_plain_string).unwrap_or_default();
        if screen.is_empty() {
            continue;
        }
        let from_ms = match &from {
            Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
            _ => None,
        };
        open.push((from_ms, screen));
    }
    open.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    Ok(open.into_iter().next().map(|(_, screen)| screen))
}

/// Drop empty segments and `.`, pop for `..`, and store `~name=value` in `inline`. A tilde that is not `name=value`
/// is dropped. Java's `cleanupPathNameList`.
fn cleanup_path_names<'a>(
    input: impl IntoIterator<Item = &'a str>,
    inline: &mut BTreeMap<String, String>,
) -> Vec<String> {
    let mut clean = Vec::new();
    for path_name in input {
        if path_name.is_empty() || path_name == "." {
            continue;
        }
        if path_name == ".." {
            clean.pop();
            continue;
        }
        if let Some(rest) = path_name.strip_prefix('~') {
            // Java `split("=")` drops trailing empties and keeps the pair only when there are two parts.
            let mut parts: Vec<&str> = rest.split('=').collect();
            while parts.last().is_some_and(|part| part.is_empty()) {
                parts.pop();
            }
            if parts.len() == 2 {
                inline.insert(parts[0].to_string(), crate::web::percent_decode(parts[1]));
            }
            continue;
        }
        clean.push(path_name.to_string());
    }
    clean
}

/// `ScreenDefinition.findSubscreenPath`. `None` is not found. A direct child whose rest does not match is not found,
/// without searching further. `allow-extra-path` is not a match.
fn find_subscreen_path(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    screen: &ScreenDef,
    remaining: &[String],
    depth: usize,
) -> Option<Vec<String>> {
    if remaining.is_empty() || depth >= FIND_DEPTH {
        return None;
    }
    let cur = &remaining[0];
    let rest = &remaining[1..];
    if let Some(item) = screen.subscreens.get(cur) {
        if rest.is_empty() {
            return Some(vec![cur.clone()]);
        }
        let sub = match facade.screen_def(factory, &item.location) {
            Ok(def) => def,
            Err(err) => {
                tracing::error!("Error finding subscreens under screen at {}: {err}", item.location);
                return None;
            }
        };
        let mut sub_path = find_subscreen_path(facade, factory, &sub, rest, depth + 1)?;
        sub_path.insert(0, cur.clone());
        return Some(sub_path);
    }
    if screen.has_transition(cur) {
        return Some(remaining.to_vec());
    }
    for item in &screen.subscreens.items {
        let sub = match facade.screen_def(factory, &item.location) {
            Ok(def) => def,
            Err(err) => {
                tracing::error!(
                    "Error finding subscreens under screen {} at {}: {err}",
                    item.name,
                    item.location
                );
                continue;
            }
        };
        let Some(grand) = sub.subscreens.get(cur) else {
            continue;
        };
        if rest.is_empty() {
            return Some(vec![item.name.clone(), cur.clone()]);
        }
        let grand_def = match facade.screen_def(factory, &grand.location) {
            Ok(def) => def,
            Err(err) => {
                tracing::error!("Error finding subscreens under screen at {}: {err}", grand.location);
                return None;
            }
        };
        let mut sub_path = find_subscreen_path(facade, factory, &grand_def, rest, depth + 1)?;
        sub_path.insert(0, cur.clone());
        sub_path.insert(0, item.name.clone());
        return Some(sub_path);
    }
    for item in &screen.subscreens.items {
        let sub = match facade.screen_def(factory, &item.location) {
            Ok(def) => def,
            Err(err) => {
                tracing::error!(
                    "Error finding subscreens under screen {} at {}: {err}",
                    item.name,
                    item.location
                );
                continue;
            }
        };
        let Some(mut sub_path) = find_subscreen_path(facade, factory, &sub, remaining, depth + 1) else {
            continue;
        };
        sub_path.insert(0, item.name.clone());
        return Some(sub_path);
    }
    super::static_files::file_under(factory, facade, &screen.location, remaining).map(|_| remaining.to_vec())
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
///
/// A leading `/` starts at the root. `.` is the current screen and `..` the parent. `//` searches by screen name
/// from `root_location`. `ScreenPathAlias` is applied first. The names are what the walk sees; `inline` holds `?`
/// and `~` parameters.
///
/// # Errors
///
/// Same as [`prepare_screen_path`].
pub fn resolve_screen_path(
    factory: &ExecutionContextFactory,
    root_location: &str,
    from: &[String],
    url: &str,
) -> Result<ScreenPath, Error> {
    prepare_screen_path(factory, root_location, from, url)
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
