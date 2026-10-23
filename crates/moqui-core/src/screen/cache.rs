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
//! interval (`screen_check_interval_ms`, default 2 s), and never when `instance_purpose` is
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
    moqui_lib::clock::now_millis()
}

/// `screen-text-output`: how one render mode is sent and which macro module writes it.
#[derive(Clone, Debug)]
pub struct TextOutput {
    pub mime_type: String,
    pub macro_template_location: String,
    pub always_standalone: bool,
    /// A `screen-output` mode: the macro module fills a document the render sends as bytes, such as `xlsx`.
    pub binary: bool,
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

/// The `screen-output` (binary) modes: type, mime type, macro module. Java named a `widget-render-class` for each; here
/// the module fills a document that the render sends as bytes.
const BUILT_IN_BINARY_MODES: [(&str, &str, &str); 1] = [(
    "xlsx",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "ScreenXlsx.js",
)];

const BUILT_IN_MODES: [(&str, &str, &str, bool); 8] = [
    ("html", "text/html", "ScreenHtml.js", false),
    ("text", "text/plain", "ScreenText.js", false),
    ("csv", "text/csv", "ScreenCsv.js", true),
    ("xml", "text/xml", "ScreenXml.js", false),
    ("html-print", "text/html", "ScreenHtmlPrint.js", true),
    ("qvt", "text/html", "ScreenQvt.js", true),
    ("qjs", "application/javascript", "ScreenPlain.js", true),
    ("qvue", "text/html", "ScreenPlain.js", true),
];

/// The render mode name the macros see: `xsl-fo` is `html-print`.
pub fn canonical_render_mode(mode: &str) -> &str {
    if mode == "xsl-fo" {
        "html-print"
    } else {
        mode
    }
}

impl ScreenConf {
    /// Built-in modes first, so a factory with no `screen-facade` still renders. Conf entries
    /// replace them by type.
    pub(super) fn parse(conf_text: &str) -> Self {
        let mut conf = Self {
            default_paginate_rows: 20,
            default_autocomplete_rows: 20,
            ..Self::default()
        };
        for (mode, mime, module, standalone) in BUILT_IN_MODES {
            conf.text_output.insert(
                mode.to_string(),
                TextOutput {
                    mime_type: mime.to_string(),
                    macro_template_location: format!("framework/template/screen/{module}"),
                    always_standalone: standalone,
                    binary: false,
                },
            );
        }
        for (mode, mime, module) in BUILT_IN_BINARY_MODES {
            conf.text_output.insert(
                mode.to_string(),
                TextOutput {
                    mime_type: mime.to_string(),
                    macro_template_location: format!("framework/template/screen/{module}"),
                    always_standalone: true,
                    binary: true,
                },
            );
        }
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
            conf.text_output.insert(
                kind,
                TextOutput {
                    mime_type: output
                        .attribute("mime-type")
                        .filter(|text| !text.is_empty())
                        .or_else(|| existing.as_ref().map(|found| found.mime_type.clone()))
                        .unwrap_or_else(|| "text/html".to_string()),
                    macro_template_location: output
                        .attribute("macro-template-location")
                        .filter(|text| !text.is_empty())
                        .or_else(|| existing.as_ref().map(|found| found.macro_template_location.clone()))
                        .unwrap_or_default(),
                    always_standalone: output.attribute("always-standalone").as_deref() == Some("true"),
                    // `widget-render-class` names a Java class; the type picks the module here.
                    binary,
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

pub struct ScreenFacade {
    conf: ScreenConf,
    definitions: MCache<String, Arc<ScreenDef>>,
    urls: MCache<String, Arc<ScreenUrlInfo>>,
    /// The full path a `//` screen path finds, or `None` when it finds none, as Java's `screenFindPathCache`.
    find_paths: MCache<String, Option<Vec<String>>>,
    /// The icon class for a text by theme, as Java's `themeIconByTextByTheme`.
    icons: MCache<String, Option<String>>,
    /// Milliseconds between checks of a definition's files; negative is never.
    check_interval_ms: AtomicI64,
    /// One lock per location being loaded, so two requests do not load the same screen.
    loading: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ScreenFacade {
    fn new(factory: &ExecutionContextFactory) -> Self {
        Self {
            conf: ScreenConf::parse(factory.conf_text()),
            definitions: MCache::new("screen.definition", CacheConfig::new().background(false)),
            urls: MCache::new(
                "screen.url",
                CacheConfig::new().background(false).max_entries(MAX_URL_ENTRIES),
            ),
            find_paths: MCache::new(
                "screen.find.path",
                CacheConfig::new().background(false).max_entries(MAX_URL_ENTRIES),
            ),
            icons: MCache::new(
                "screen.theme.icon",
                CacheConfig::new()
                    .background(false)
                    .max_entries(MAX_URL_ENTRIES)
                    .expire_creation_ms(ICON_EXPIRE_MS),
            ),
            check_interval_ms: AtomicI64::new(configured_check_interval()),
            loading: Mutex::new(HashMap::new()),
        }
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
            // Resolved paths hold the old definition.
            let _ = self.urls.clear();
            let _ = self.find_paths.clear();
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

    pub(super) fn cached_find_path(&self, key: &str) -> Option<Option<Vec<String>>> {
        self.find_paths.get(&key.to_string()).ok().flatten()
    }

    pub(super) fn put_find_path(&self, key: String, path: Option<Vec<String>>) {
        let _ = self.find_paths.put(key, path);
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
        let _ = self.find_paths.clear();
        let _ = self.icons.clear();
    }

    /// Cache hit and miss counts, for tests and the status page.
    pub fn cache_stats(&self) -> moqui_lib::cache::CacheStats {
        self.definitions.stats()
    }

    /// The definition, URL, and theme icon caches as the cache admin screen lists them.
    pub fn cache_infos(&self) -> Vec<moqui_lib::cache::CacheInfo> {
        vec![
            self.definitions.info(),
            self.urls.info(),
            self.find_paths.info(),
            self.icons.info(),
        ]
    }

    /// Clear the one of [`Self::cache_infos`] named `name`. False when none has that name. A definition clears
    /// with the resolved paths that point at it.
    pub fn clear_named(&self, name: &str) -> bool {
        if name == self.definitions.info().name {
            self.clear();
        } else if name == self.urls.info().name {
            let _ = self.urls.clear();
        } else if name == self.find_paths.info().name {
            let _ = self.find_paths.clear();
        } else if name == self.icons.info().name {
            let _ = self.icons.clear();
        } else {
            return false;
        }
        true
    }
}

/// `screen_check_interval_ms` when set, else never in production and two seconds otherwise.
fn configured_check_interval() -> i64 {
    let configured = moqui_lib::env::get_prop_or_env("screen_check_interval_ms");
    if let Ok(millis) = configured.trim().parse::<i64>() {
        return millis;
    }
    if moqui_lib::env::get_prop_or_env("instance_purpose") == "production" {
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
