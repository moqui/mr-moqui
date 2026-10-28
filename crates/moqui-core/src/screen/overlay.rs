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

//! The request-scoped location overlay (plans/design-dev-aide.md F14): a screen renders from unsaved or proposed text
//! standing at its location, for the renders on this thread while the overlay is held, so relative paths, subscreens
//! and `screen-extend` files resolve as they will after the save (the Preview of a proposed change set, the dev
//! dialects, later the builder's buffer). Nothing is written. While one is held, screen definitions and resolved paths
//! neither come from nor go to the caches, so a draft never reaches another request and the cache never hides a draft.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The texts that stand at their locations: each by its `component://` location and by its file path, as a screen is
/// read by location and a `screen-extend` file by path.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    texts: HashMap<String, Rc<str>>,
}

impl Overlay {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `text` at `location` (`component://...`) and, when given, at its file path.
    pub fn insert(&mut self, location: &str, path: Option<&std::path::Path>, text: &str) {
        let text: Rc<str> = Rc::from(text);
        self.texts.insert(location.to_string(), Rc::clone(&text));
        if let Some(path) = path {
            self.texts.insert(path.to_string_lossy().into_owned(), text);
        }
    }

    /// `text` at `location` (`component://<component>/<path>`) and at the file path a screen-extend lookup builds
    /// for it from the component's directory.
    pub fn insert_location(&mut self, factory: &crate::context::ExecutionContextFactory, location: &str, text: &str) {
        let path = location
            .strip_prefix("component://")
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(name, rest)| {
                factory
                    .components()
                    .iter()
                    .find(|component| component.name == name)
                    .map(|component| component.location.join(rest))
            });
        self.insert(location, path.as_deref(), text);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

/// Run `body` with `overlay` in effect on this thread; the overlay held before is back after it.
pub fn with<R>(overlay: Overlay, body: impl FnOnce() -> R) -> R {
    struct Restore(Option<Overlay>);
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

/// Whether an overlay is in effect on this thread: the screen caches are passed over.
#[must_use]
pub fn active() -> bool {
    ACTIVE.with(|active| active.borrow().as_ref().is_some_and(|overlay| !overlay.is_empty()))
}

/// The overlaid text of a location or file path, when there is one.
#[must_use]
pub fn text_of(key: &str) -> Option<Rc<str>> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .and_then(|overlay| overlay.texts.get(key).cloned())
    })
}

/// What turns a preview token (a request's `moquiPreview` parameter) into the overlay of that render, for the current
/// user: the dev-aide's change sets and canvases. `None` when the token is not the user's or names nothing.
pub type Provider =
    dyn Fn(&std::sync::Arc<crate::context::ExecutionContextFactory>, &str) -> Option<Overlay> + Send + Sync;

static PROVIDER: std::sync::OnceLock<Box<Provider>> = std::sync::OnceLock::new();

/// Set the preview provider, once per process (the dev-aide's).
pub fn set_provider(provider: Box<Provider>) {
    if PROVIDER.set(provider).is_err() {
        tracing::debug!("the screen preview provider was set already");
    }
}

/// The overlay a preview token names, on a dev instance with a provider.
#[must_use]
pub fn for_token(factory: &std::sync::Arc<crate::context::ExecutionContextFactory>, token: &str) -> Option<Overlay> {
    if token.is_empty() || !crate::devtools::is_dev_instance() {
        return None;
    }
    // a screen-xml canvas of the user's conversation
    #[cfg(feature = "llm")]
    if let Some(artifact_id) = token.strip_prefix("canvas:") {
        return crate::llm::ui::screen_xml::canvas_overlay(factory, artifact_id);
    }
    PROVIDER.get().and_then(|provider| provider(factory, token))
}
