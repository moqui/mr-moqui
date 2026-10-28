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

//! Share a loaded [`EntityModel`] across threads.
//!
//! The model holds no `MNode`, so it is `Send` and `Sync` on its own; the assertion below keeps
//! it that way. On a dev instance the model is swapped when entity files change
//! (plans/design-dev-aide.md F5): each facade call takes the model once, as an `Arc`, so a find in
//! flight keeps the model it started with while a new one is linked and put in its place.

use std::sync::{Arc, RwLock};

use moqui_db::model::EntityModel;

/// Loaded entity model, immutable once loaded; [`Self::swap`] puts a newer one in its place.
pub(crate) struct SharedModel {
    model: RwLock<Arc<EntityModel>>,
    /// The configuration the model was loaded with, as text (an `MNode` is not `Send`), to load it again.
    conf: String,
}

impl SharedModel {
    pub(crate) fn new(model: EntityModel, conf: String) -> Self {
        Self {
            model: RwLock::new(Arc::new(model)),
            conf,
        }
    }

    /// The current model.
    pub(crate) fn model(&self) -> Arc<EntityModel> {
        Arc::clone(&self.model.read().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Put `model` in place of the current one, which calls holding it keep until they finish.
    pub(crate) fn swap(&self, model: EntityModel) -> Arc<EntityModel> {
        let mut current = self.model.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut *current, Arc::new(model))
    }

    pub(crate) fn conf_text(&self) -> &str {
        &self.conf
    }
}

const _: fn() = || {
    fn shared<T: Send + Sync>() {}
    shared::<SharedModel>();
};
