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

//! The request-scoped location overlay for services (plans/design-dev-aide.md F14, 9.4): service files and script
//! files from unsaved or proposed text, standing at their locations for the calls on this thread while the overlay is
//! held. The service files' definitions are made as a reload of them would make them ([`super::define`]'s
//! `overlaid_definitions`), over the registry as it is, and the registry is not changed: a lookup on this thread
//! ([`super::define::find`]) takes the overlay's definition first, so the service tried and every service it calls
//! that the files define, or that implements or includes one of them, run as the text has them, while every other
//! thread sees the registry. A script file's text stands at its `component://` location for the scripts read on this
//! thread (`script::runtime::facades::read_module`), so a script service runs its buffer's text. The Dev AIDE's Try
//! holds one inside the sim only, so nothing a draft does is kept.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use moqui_lib::node::MNode;

use super::define::ServiceDef;
use crate::context::Error;

/// One file of an overlay: its `component://` location, its path, and its text.
#[derive(Clone, Debug)]
pub struct OverlayFile {
    pub location: String,
    pub path: PathBuf,
    pub text: String,
}

/// The services and script texts an overlay's files define, by name and by location.
#[derive(Debug, Default)]
pub struct ServiceOverlay {
    /// Each service the files define or defined, and what implements or includes one of them; `None` for one the files
    /// no longer define.
    services: BTreeMap<String, Option<Arc<ServiceDef>>>,
    /// The texts of the files that are not service files (a script service's `.js`), by location.
    scripts: HashMap<String, Arc<str>>,
    /// The services the files define.
    defined: BTreeSet<String>,
    /// The registration failures of the service files and their services.
    failures: Vec<String>,
}

/// Whether `path` is a service definition file: an `.xml` file under a `service` directory, not a rules or REST file.
fn is_service_file(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    name.ends_with(".xml")
        && ![".secas.xml", ".emecas.xml", ".gitecas.xml", ".rest.xml"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        && path.components().any(|part| part.as_os_str() == "service")
}

impl ServiceOverlay {
    /// The overlay of `files`, over the entered context's service registry: the service files' definitions and the
    /// other files' texts. A file that is neither a service file nor a script (a screen, an entity file) is left out.
    /// The runtime runs a `type="script"` service's location as JavaScript, so only `.js` texts are held.
    ///
    /// # Errors
    ///
    /// Returns an error when no context is entered, when a service file does not parse as XML (naming the file), or
    /// when the text defines a Rust service anew or points one at another function (it would run server code by a
    /// name of the text's choosing).
    pub fn build(files: &[OverlayFile]) -> Result<Self, Error> {
        let mut parsed = Vec::new();
        let mut scripts = HashMap::new();
        for file in files {
            if is_service_file(&file.path) {
                let root = MNode::parse_text(&file.path.display().to_string(), &file.text)
                    .map_err(|err| Error::new(format!("{} does not parse: {err}", file.location)))?;
                parsed.push((file.path.clone(), root));
            } else if file.location.ends_with(".js") {
                scripts.insert(file.location.clone(), Arc::from(file.text.as_str()));
            }
        }
        let mut overlay = Self {
            scripts,
            ..Self::default()
        };
        if !parsed.is_empty() {
            let made = super::define::overlaid_definitions(parsed)?;
            // a Rust function is code of the server's, run by its registered name and authorization: the text may not
            // name one anew, or another one, for a service
            for (name, def) in &made.services {
                let Some(def) = def else {
                    continue;
                };
                if !matches!(
                    def.kind,
                    super::define::ServiceKind::Rust | super::define::ServiceKind::Native
                ) {
                    continue;
                }
                let registered = super::define::find(name);
                // the type too: a native service's conf runner is chosen by it, at the same location
                if registered.is_none_or(|have| {
                    have.kind != def.kind || have.location != def.location || have.service_type != def.service_type
                }) {
                    return Err(Error::new(format!(
                        "service {name} is a Rust service its text defines anew or changes: save it to run it, \
                         unsaved text runs only XML Actions and scripts"
                    )));
                }
            }
            overlay.services = made.services;
            overlay.defined = made.defined;
            overlay.failures = made.failures;
        }
        Ok(overlay)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.services.is_empty() && self.scripts.is_empty()
    }

    /// Whether the overlay's files define `name`.
    #[must_use]
    pub fn defines(&self, name: &str) -> bool {
        self.defined.contains(name)
    }

    /// The services the files define.
    #[must_use]
    pub fn defined(&self) -> Vec<String> {
        self.defined.iter().cloned().collect()
    }

    /// The registration failures of the files and their services (an expression that is not JavaScript, an
    /// `implements` that names nothing).
    #[must_use]
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// The locations of the script texts it holds.
    #[must_use]
    pub fn script_locations(&self) -> Vec<String> {
        let mut locations: Vec<String> = self.scripts.keys().cloned().collect();
        locations.sort();
        locations
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<ServiceOverlay>>> = const { RefCell::new(None) };
}

/// Run `body` with `overlay` in effect on this thread; the overlay held before is back after it, a panic included.
pub fn with<R>(overlay: Arc<ServiceOverlay>, body: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<ServiceOverlay>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            ACTIVE.with(|active| *active.borrow_mut() = previous);
        }
    }
    let previous = ACTIVE.with(|active| active.borrow_mut().replace(overlay));
    let _restore = Restore(previous);
    body()
}

/// Whether an overlay holds this thread: what is made from service definitions then (a screen's forms and units) is
/// not kept for other threads (`screen::generate`).
#[must_use]
pub fn held() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

/// The held overlay's definition of `name`: `Some(None)` when its files no longer define it, `None` when the overlay
/// has nothing of it (or none is held), so the registry answers.
pub(crate) fn find(name: &str) -> Option<Option<Arc<ServiceDef>>> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .and_then(|overlay| overlay.services.get(name).cloned())
    })
}

/// The held overlay's text of the script at `location`.
pub(crate) fn script_text(location: &str) -> Option<Arc<str>> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .and_then(|overlay| overlay.scripts.get(location).cloned())
    })
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
