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

//! The screen facade: `screen-facade` conf, render modes, and the screen-definition cache.
//!
//! One `ScreenFacade` lives on each factory as an extension. Definitions sit in a `moqui_lib`
//! `MCache` keyed by location. A cached definition is used while every file it was read from
//! still has the modified time it had at load. The files are checked at most once per check
//! interval (`screen-facade` `check-interval-ms`, default 2 s), and never when `instance_purpose` is
//! `production`, as Java's `screen.location` cache returns a hit without looking at the file.
//!
//! Resolved paths (`ScreenUrlInfo`) are kept in a second cache, as Java's `screen.url`, while the
//! definitions they hold are current.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

use moqui_lib::cache::{CacheConfig, MCache};
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::definition::ScreenDef;
use super::url::ScreenUrlInfo;
use crate::context::{Error, ExecutionContextFactory};

/// The default check interval for a screen file outside production.
const DEFAULT_CHECK_INTERVAL_MS: i64 = 2000;
const MAX_URL_ENTRIES: usize = 20_000;
/// Theme icons are data, so an edit shows within this time without a restart.
const ICON_EXPIRE_MS: i64 = 300_000;

thread_local! {
    /// Locations this thread is loading, so an include cycle does not wait on its own load lock.
    static LOADING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// A load in progress on this thread. When dropped, also by a panic, it takes the location out of [`LOADING`] when it
/// put it there, and out of the facade's lock map when this thread holds that location's lock.
struct LoadingMark<'a> {
    locks: &'a Mutex<HashMap<String, Arc<Mutex<()>>>>,
    key: &'a str,
    marked: bool,
    locked: bool,
}

impl<'a> LoadingMark<'a> {
    fn new(locks: &'a Mutex<HashMap<String, Arc<Mutex<()>>>>, key: &'a str, mark: bool, locked: bool) -> Self {
        if mark {
            LOADING.with(|loading| loading.borrow_mut().insert(key.to_string()));
        }
        Self {
            locks,
            key,
            marked: mark,
            locked,
        }
    }
}

impl Drop for LoadingMark<'_> {
    fn drop(&mut self) {
        if self.marked {
            LOADING.with(|loading| loading.borrow_mut().remove(self.key));
        }
        if self.locked {
            lock(self.locks).remove(self.key);
        }
    }
}

pub(super) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// How a render mode writes a failed section, and whether the finished level is a JSON document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    /// Escaped text written where the section was.
    Text,
    /// Same failure text as [`OutputKind::Text`]. A mode with an HTML mime type.
    Html,
    /// A Vue template. What the failed section wrote is dropped, and a brace in the message is a character reference.
    Vue,
    /// One JSON array. A failed section is an `error` node, and the level is wrapped in an envelope.
    Json,
}

impl OutputKind {
    /// The `output-kind` attribute of a `screen-text-output`: `text`, `html`, `vue` or `json`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "text" => Some(Self::Text),
            "html" => Some(Self::Html),
            "vue" => Some(Self::Vue),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// A mode whose conf entry has no `output-kind`. `application/json` is [`OutputKind::Json`], `text/html` is
    /// [`OutputKind::Html`].
    pub fn from_mime(mime: &str) -> Self {
        let mime = mime.split(';').next().unwrap_or(mime).trim();
        if mime.eq_ignore_ascii_case("application/json") {
            Self::Json
        } else if mime.eq_ignore_ascii_case("text/html") {
            Self::Html
        } else {
            Self::Text
        }
    }
}

/// `screen-text-output`: how one render mode is sent and which macro module writes it.
#[derive(Clone, Debug)]
pub struct TextOutput {
    pub mime_type: String,
    pub macro_template_location: String,
    pub always_standalone: bool,
    /// A `screen-output` mode: the macro module fills a document the render sends as bytes, such as `xlsx`.
    pub binary: bool,
    pub kind: OutputKind,
}

/// One `subscreens-item` from a conf `screen-facade/screen` element.
#[derive(Clone, Debug)]
pub struct ConfItem {
    attributes: Vec<(String, String)>,
}

impl ConfItem {
    pub(super) fn node(&self) -> MNode {
        let node = MNode::new("subscreens-item");
        for (key, value) in &self.attributes {
            node.set_attribute(key.clone(), value.clone());
        }
        node
    }
}

#[derive(Clone, Debug, Default)]
pub struct ConfScreen {
    pub items: Vec<ConfItem>,
    pub default_subscreen: Option<String>,
}

/// The parsed `screen-facade` conf.
#[derive(Clone, Debug, Default)]
pub struct ScreenConf {
    pub text_output: BTreeMap<String, TextOutput>,
    pub screens: BTreeMap<String, ConfScreen>,
    pub boundary_comments: bool,
    pub default_paginate_rows: i64,
    pub default_autocomplete_rows: i64,
}

/// The render mode name the macros see: `xsl-fo` is `html-print`.
pub fn canonical_render_mode(mode: &str) -> &str {
    if mode == "xsl-fo" {
        "html-print"
    } else {
        mode
    }
}

impl ScreenConf {
    /// The modes are the `screen-text-output` and `screen-output` entries of the merged conf; there are no others.
    pub(super) fn parse(conf_text: &str) -> Self {
        let mut conf = Self {
            default_paginate_rows: 20,
            default_autocomplete_rows: 20,
            ..Self::default()
        };
        let Ok(root) = MNode::parse_text("moqui-conf", conf_text) else {
            return conf;
        };
        let Some(facade) = root.first_named("screen-facade") else {
            return conf;
        };
        conf.boundary_comments = facade.attribute("boundary-comments").as_deref() == Some("true");
        if let Some(rows) = facade
            .attribute("default-paginate-rows")
            .and_then(|text| text.parse().ok())
        {
            conf.default_paginate_rows = rows;
        }
        if let Some(rows) = facade
            .attribute("default-autocomplete-rows")
            .and_then(|text| text.parse().ok())
        {
            conf.default_autocomplete_rows = rows;
        }
        let text_outputs = facade
            .children_named("screen-text-output")
            .into_iter()
            .map(|node| (node, false));
        let binary_outputs = facade
            .children_named("screen-output")
            .into_iter()
            .map(|node| (node, true));
        for (output, binary) in text_outputs.chain(binary_outputs) {
            let Some(kind) = output.attribute("type").filter(|text| !text.is_empty()) else {
                continue;
            };
            // `xsl-fo` is the print mode, so its entry configures `html-print`.
            let kind = canonical_render_mode(&kind).to_string();
            let existing = conf.text_output.get(&kind).cloned();
            let mime_type = output
                .attribute("mime-type")
                .filter(|text| !text.is_empty())
                .or_else(|| existing.as_ref().map(|found| found.mime_type.clone()))
                .unwrap_or_else(|| "text/html".to_string());
            // `output-kind`, else the kind of an earlier entry of the type (`html-print` for `xsl-fo`), else the mime type.
            let named_kind = output
                .attribute("output-kind")
                .filter(|text| !text.is_empty())
                .and_then(|name| {
                    let found = OutputKind::from_name(&name);
                    if found.is_none() {
                        tracing::warn!("Render mode {kind} has unknown output-kind {name}, using its mime type");
                    }
                    found
                });
            let output_kind = named_kind
                .or_else(|| existing.as_ref().map(|found| found.kind))
                .unwrap_or_else(|| OutputKind::from_mime(&mime_type));
            conf.text_output.insert(
                kind,
                TextOutput {
                    mime_type,
                    macro_template_location: output
                        .attribute("macro-template-location")
                        .filter(|text| !text.is_empty())
                        .or_else(|| existing.as_ref().map(|found| found.macro_template_location.clone()))
                        .unwrap_or_default(),
                    always_standalone: output.attribute("always-standalone").as_deref() == Some("true"),
                    // `widget-render-class` names a Java class; the type picks the module here.
                    binary,
                    kind: output_kind,
                },
            );
        }
        for screen in facade.children_named("screen") {
            let Some(location) = screen.attribute("location").filter(|text| !text.is_empty()) else {
                continue;
            };
            let entry = conf.screens.entry(location).or_default();
            for item in screen.children_named("subscreens-item") {
                entry.items.push(ConfItem {
                    attributes: item.attributes(),
                });
            }
            if let Some(default) = screen.attribute("default-subscreen").filter(|text| !text.is_empty()) {
                entry.default_subscreen = Some(default);
            }
        }
        conf
    }
}

/// Expanded `//` paths, and misses, for one root screen. Dropped when screen configuration changes.
struct SparsePaths {
    generation: u64,
    paths: HashMap<String, Option<Vec<String>>>,
}

pub struct ScreenFacade {
    conf: ScreenConf,
    definitions: MCache<String, Arc<ScreenDef>>,
    urls: MCache<String, Arc<ScreenUrlInfo>>,
    /// `//` name searches, keyed by root location and the sparse path. A miss is stored as `None`.
    sparse: Mutex<SparsePaths>,
    /// The icon class for a text by theme, as Java's `themeIconByTextByTheme`.
    icons: MCache<String, Option<String>>,
    /// Milliseconds between checks of a definition's files; negative is never.
    check_interval_ms: AtomicI64,
    /// One lock per location being loaded, so two requests do not load the same screen.
    loading: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// `sri.getShellProviders` by session, user, locale and theme ([`super::shell::shell_providers`]).
    shell_providers: Mutex<HashMap<String, ShellProvidersEntry>>,
}

/// The providers of the apps a user may open, and the screens they were read from: the entry is used while each of
/// them is current, for at most [`SHELL_PROVIDERS_TTL`].
struct ShellProvidersEntry {
    value: Value,
    defs: Vec<Arc<ScreenDef>>,
    made: std::time::Instant,
}

/// How long the providers of a session are kept: a grant or an app's link target that changes shows by then.
const SHELL_PROVIDERS_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// The most sessions whose providers are kept; past it the oldest go.
const SHELL_PROVIDERS_MAX: usize = 2000;

impl ScreenFacade {
    fn new(factory: &ExecutionContextFactory) -> Self {
        Self {
            conf: ScreenConf::parse(factory.conf_text()),
            definitions: MCache::new("screen.definition", CacheConfig::new().background(false)),
            urls: MCache::new(
                "screen.url",
                CacheConfig::new().background(false).max_entries(MAX_URL_ENTRIES),
            ),
            sparse: Mutex::new(SparsePaths {
                generation: 0,
                paths: HashMap::new(),
            }),
            icons: MCache::new(
                "screen.theme.icon",
                CacheConfig::new()
                    .background(false)
                    .max_entries(MAX_URL_ENTRIES)
                    .expire_creation_ms(ICON_EXPIRE_MS),
            ),
            check_interval_ms: AtomicI64::new(configured_check_interval(factory)),
            loading: Mutex::new(HashMap::new()),
            shell_providers: Mutex::new(HashMap::new()),
        }
    }

    /// The providers kept for `key`, while every screen they were read from is current and they are not older than
    /// [`SHELL_PROVIDERS_TTL`].
    pub(super) fn cached_shell_providers(&self, factory: &ExecutionContextFactory, key: &str) -> Option<Value> {
        let (value, defs) = {
            let mut cache = lock(&self.shell_providers);
            let entry = cache.get(key)?;
            if entry.made.elapsed() > SHELL_PROVIDERS_TTL {
                cache.remove(key);
                return None;
            }
            (entry.value.clone(), entry.defs.clone())
        };
        // checked outside the lock: a check may read the screen files
        if defs.iter().all(|def| self.is_current(factory, def)) {
            Some(value)
        } else {
            lock(&self.shell_providers).remove(key);
            None
        }
    }

    pub(super) fn put_shell_providers(&self, key: String, value: Value, defs: Vec<Arc<ScreenDef>>) {
        let mut cache = lock(&self.shell_providers);
        if cache.len() >= SHELL_PROVIDERS_MAX {
            cache.retain(|_, entry| entry.made.elapsed() <= SHELL_PROVIDERS_TTL);
            if cache.len() >= SHELL_PROVIDERS_MAX {
                if let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.made)
                    .map(|(key, _)| key.clone())
                {
                    cache.remove(&oldest);
                }
            }
        }
        cache.insert(
            key,
            ShellProvidersEntry {
                value,
                defs,
                made: std::time::Instant::now(),
            },
        );
    }

    /// Drop the providers kept for a session (its keys start with `prefix`): a login or a logout in it.
    pub fn forget_shell_providers(&self, prefix: &str) {
        lock(&self.shell_providers).retain(|key, _| !key.starts_with(prefix));
    }

    /// Check the files of a cached definition at most once per `interval`, or never for `None`.
    pub fn set_check_interval(&self, interval: Option<std::time::Duration>) {
        let millis = interval.map_or(-1, |interval| i64::try_from(interval.as_millis()).unwrap_or(i64::MAX));
        self.check_interval_ms.store(millis, Ordering::Relaxed);
    }

    /// The facade for `factory`, made on first use.
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(Self::new)
    }

    pub fn conf(&self) -> &ScreenConf {
        &self.conf
    }

    /// Whether `mode` is a render mode with a `screen-text-output`. `xsl-fo` counts as `html-print`.
    pub fn is_render_mode_valid(&self, mode: &str) -> bool {
        self.conf.text_output.contains_key(canonical_render_mode(mode))
    }

    pub fn text_output(&self, mode: &str) -> Option<&TextOutput> {
        self.conf.text_output.get(canonical_render_mode(mode))
    }

    pub fn mime_type(&self, mode: &str) -> Option<&str> {
        self.text_output(mode).map(|output| output.mime_type.as_str())
    }

    /// Whether `mode` is a `screen-output` mode whose render is bytes, not text.
    pub fn is_render_mode_binary(&self, mode: &str) -> bool {
        self.text_output(mode).is_some_and(|output| output.binary)
    }

    pub fn is_always_standalone(&self, mode: &str) -> bool {
        self.text_output(mode).is_some_and(|output| output.always_standalone)
    }

    pub fn output_kind(&self, mode: &str) -> OutputKind {
        self.text_output(mode)
            .map(|output| output.kind)
            .unwrap_or(OutputKind::Text)
    }

    /// The definition at `location`, from the cache when no source file has changed.
    ///
    /// # Errors
    ///
    /// Returns an error when the screen is missing or does not load. A missing screen is
    /// `ErrorKind::NotFound`.
    pub fn screen_def(&self, factory: &ExecutionContextFactory, location: &str) -> Result<Arc<ScreenDef>, Error> {
        self.screen_def_at(factory, location, 0)
    }

    pub(super) fn screen_def_at(
        &self,
        factory: &ExecutionContextFactory,
        location: &str,
        depth: usize,
    ) -> Result<Arc<ScreenDef>, Error> {
        let key = location.to_string();
        if let Some(cached) = self.definitions.get(&key)? {
            if self.is_current(factory, &cached) {
                return Ok(cached);
            }
        }
        // A cycle of includes comes back to a location this thread is loading: the depth limit stops it.
        let (reentrant, nested) = LOADING.with(|loading| {
            let loading = loading.borrow();
            (loading.contains(location), !loading.is_empty())
        });
        let location_lock = (!reentrant).then(|| Arc::clone(lock(&self.loading).entry(key.clone()).or_default()));
        // Only a load that holds no other location's lock waits for one, so two threads loading screens that include
        // each other cannot wait on each other. An include whose location another thread is loading is loaded again here.
        let held = location_lock.as_ref().and_then(|mutex| {
            if nested {
                match mutex.try_lock() {
                    Ok(guard) => Some(guard),
                    Err(TryLockError::Poisoned(err)) => Some(err.into_inner()),
                    Err(TryLockError::WouldBlock) => None,
                }
            } else {
                Some(lock(mutex))
            }
        });
        // Another request may have loaded it while this one waited.
        let stale = match self.definitions.get(&key)? {
            Some(cached) if held.is_some() && self.is_current(factory, &cached) => return Ok(cached),
            Some(_) => true,
            None => false,
        };
        let mark = LoadingMark::new(&self.loading, &key, !reentrant, held.is_some());
        let loaded = ScreenDef::load(self, factory, location, depth).map(Arc::new);
        let stored = loaded.and_then(|loaded| {
            // A put in the same millisecond as the last one keeps the old value, so clear the key first.
            self.definitions.remove(&key)?;
            self.definitions.put(key.clone(), Arc::clone(&loaded))?;
            Ok(loaded)
        });
        // Waiters find the new definition in the cache once the lock entry is gone.
        drop(mark);
        drop(held);
        let loaded = stored?;
        if stale {
            // Resolved paths and `//` searches hold the old definition.
            let _ = self.urls.clear();
            self.clear_sparse();
        }
        Ok(loaded)
    }

    /// Whether every file `def` was read from still has its modified time, checked at most once per interval.
    pub(super) fn is_current(&self, factory: &ExecutionContextFactory, def: &ScreenDef) -> bool {
        // A write to the SubscreensItem rows since this definition read them makes it stale at once.
        if def.config_generation != super::definition::screen_config_generation(factory) {
            return false;
        }
        let interval = self.check_interval_ms.load(Ordering::Relaxed);
        if interval < 0 {
            return true;
        }
        let now = now_millis();
        if now.saturating_sub(def.checked_at.load(Ordering::Relaxed)) < interval {
            return true;
        }
        let current = def
            .sources
            .iter()
            .chain(def.generated.dependencies().iter())
            .all(|source| source.is_current(factory));
        if current {
            def.checked_at.store(now, Ordering::Relaxed);
        }
        current
    }

    /// A resolved path from the cache, when every definition it holds is current.
    pub(super) fn cached_url(&self, factory: &ExecutionContextFactory, key: &str) -> Option<Arc<ScreenUrlInfo>> {
        let found = self.urls.get(&key.to_string()).ok().flatten()?;
        if found.path_defs.iter().all(|def| self.is_current(factory, def)) {
            Some(found)
        } else {
            let _ = self.urls.remove(&key.to_string());
            None
        }
    }

    pub(super) fn put_url(&self, key: String, info: Arc<ScreenUrlInfo>) {
        let _ = self.urls.put(key, info);
    }

    /// An expanded `//` path, or `None` for a cached miss. `None` of the outer option means the cache has no entry,
    /// or screen configuration changed since it was stored.
    pub(super) fn cached_sparse(&self, factory: &ExecutionContextFactory, key: &str) -> Option<Option<Vec<String>>> {
        let generation = super::definition::screen_config_generation(factory);
        let mut cache = lock(&self.sparse);
        if cache.generation != generation {
            cache.paths.clear();
            cache.generation = generation;
            return None;
        }
        cache.paths.get(key).cloned()
    }

    pub(super) fn put_sparse(&self, factory: &ExecutionContextFactory, key: String, names: Option<Vec<String>>) {
        let generation = super::definition::screen_config_generation(factory);
        let mut cache = lock(&self.sparse);
        if cache.generation != generation {
            cache.paths.clear();
            cache.generation = generation;
        }
        cache.paths.insert(key, names);
    }

    fn clear_sparse(&self) {
        let mut cache = lock(&self.sparse);
        cache.paths.clear();
    }

    pub(super) fn theme_icon(&self, theme: &str, text: &str) -> Option<Option<String>> {
        self.icons.get(&format!("{theme}\u{1}{text}")).ok().flatten()
    }

    pub(super) fn put_theme_icon(&self, theme: &str, text: &str, icon: Option<String>) {
        let _ = self.icons.put(format!("{theme}\u{1}{text}"), icon);
    }

    /// Whether `location` is a screen file. A load error counts as not a screen.
    pub fn is_screen(&self, factory: &ExecutionContextFactory, location: &str) -> bool {
        if self.definitions.contains_key(&location.to_string()).unwrap_or(false) {
            return true;
        }
        // As in Java, only an XML file can be a screen, so a static file is not read here.
        if !location.ends_with(".xml") {
            return false;
        }
        let Ok(resource) = factory.resource().get(location) else {
            return false;
        };
        if !resource.exists() || !resource.is_file() {
            return false;
        }
        resource
            .read_text()
            .ok()
            .and_then(|text| MNode::parse_root_only(location, &text).ok())
            .is_some_and(|root| root.name() == "screen")
    }

    /// Drop every cached definition and resolved path.
    pub fn clear(&self) {
        let _ = self.definitions.clear();
        let _ = self.urls.clear();
        let _ = self.icons.clear();
        lock(&self.shell_providers).clear();
    }

    /// Cache hit and miss counts, for tests and the status page.
    pub fn cache_stats(&self) -> moqui_lib::cache::CacheStats {
        self.definitions.stats()
    }

    /// The definition, URL, and theme icon caches as the cache admin screen lists them.
    pub fn cache_infos(&self) -> Vec<moqui_lib::cache::CacheInfo> {
        vec![self.definitions.info(), self.urls.info(), self.icons.info()]
    }

    /// Clear the one of [`Self::cache_infos`] named `name`. False when none has that name. A definition clears
    /// with the resolved paths that point at it.
    pub fn clear_named(&self, name: &str) -> bool {
        if name == self.definitions.info().name {
            self.clear();
        } else if name == self.urls.info().name {
            let _ = self.urls.clear();
        } else if name == self.icons.info().name {
            let _ = self.icons.clear();
        } else {
            return false;
        }
        true
    }
}

/// The conf's `screen-facade` `check-interval-ms` when set, else never in production and two seconds otherwise.
fn configured_check_interval(factory: &ExecutionContextFactory) -> i64 {
    if let Ok(millis) = factory
        .conf_setting("screen-facade", "check-interval-ms")
        .parse::<i64>()
    {
        return millis;
    }
    if crate::web::is_production() {
        -1
    } else {
        DEFAULT_CHECK_INTERVAL_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_a_load_leaves_no_loading_mark_or_lock_entry() {
        let locks: Mutex<HashMap<String, Arc<Mutex<()>>>> = Mutex::new(HashMap::new());
        let key = "component://x/Panics.xml";
        locks.lock().unwrap().insert(key.to_string(), Arc::default());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _mark = LoadingMark::new(&locks, key, true, true);
            assert!(LOADING.with(|loading| loading.borrow().contains(key)));
            panic!("load failed");
        }));
        assert!(caught.is_err());
        assert!(LOADING.with(|loading| loading.borrow().is_empty()));
        assert!(locks.lock().unwrap().is_empty());
    }

    /// Two screens whose transitions include each other, in a new directory: their locations.
    fn mutual_includes(tag: &str) -> (std::path::PathBuf, String, String) {
        let dir = std::env::temp_dir().join(format!("moqui-screen-mutual-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (x, y) = (dir.join("X.xml"), dir.join("Y.xml"));
        let (x, y) = (x.to_string_lossy().into_owned(), y.to_string_lossy().into_owned());
        let screen = |other: &str| {
            format!(
                "<screen require-authentication=\"false\"><transition-include name=\"t\" location=\"{other}\"/><widgets/></screen>"
            )
        };
        std::fs::write(&x, screen(&y)).unwrap();
        std::fs::write(&y, screen(&x)).unwrap();
        (dir, x, y)
    }

    /// Load `location` on a new thread: the receiver gets the result, with the error as text.
    fn load_on_thread(
        factory: &Arc<ExecutionContextFactory>,
        location: &str,
        start: Option<Arc<std::sync::Barrier>>,
    ) -> std::sync::mpsc::Receiver<Result<(), String>> {
        let (factory, location) = (Arc::clone(factory), location.to_string());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            if let Some(start) = start {
                start.wait();
            }
            let loaded = ScreenFacade::of(&factory).screen_def(&factory, &location);
            let _ = sender.send(loaded.map(|_| ()).map_err(|err| err.to_string()));
        });
        receiver
    }

    #[test]
    fn an_include_whose_location_another_thread_is_loading_does_not_wait_for_it() {
        let context = ExecutionContextFactory::test_context();
        let factory = context.factory();
        let facade = ScreenFacade::of(factory);
        let (dir, x, y) = mutual_includes("held");
        // Another thread is loading Y, and Y includes X.
        let y_lock = Arc::clone(facade.loading.lock().unwrap().entry(y.clone()).or_default());
        let held = y_lock.lock().unwrap();
        let result = load_on_thread(factory, &x, None)
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the load of X waits on the lock of Y");
        drop(held);
        let err = result.unwrap_err();
        assert!(err.contains("nested too deeply"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_threads_loading_screens_that_include_each_other_both_finish() {
        let context = ExecutionContextFactory::test_context();
        let factory = context.factory();
        let (dir, x, y) = mutual_includes("race");
        for _ in 0..20 {
            let start = Arc::new(std::sync::Barrier::new(2));
            let first = load_on_thread(factory, &x, Some(Arc::clone(&start)));
            let second = load_on_thread(factory, &y, Some(start));
            for receiver in [first, second] {
                let result = receiver
                    .recv_timeout(std::time::Duration::from_secs(60))
                    .expect("a load of mutually including screens hangs");
                assert!(result.is_err());
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
