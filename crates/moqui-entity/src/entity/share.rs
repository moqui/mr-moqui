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
//! flight keeps the model it started with while a new one is linked and put in its place. Before the swap, the relink
//! checks the tables against the new model on its own thread alone ([`SharedModel::with_staged`]), so a column the new
//! model reads exists before any other thread can read it (plans/Component-Deploy.md, Phase 0).

use std::cell::RefCell;
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

    /// The current model, or on a thread in [`Self::with_staged`] the model staged there.
    pub(crate) fn model(&self) -> Arc<EntityModel> {
        if let Some(staged) = self.staged() {
            return staged;
        }
        Arc::clone(&moqui_lib::sync::read(&self.model))
    }

    /// Put `model` in place of the current one, which calls holding it keep until they finish.
    pub(crate) fn swap(&self, model: Arc<EntityModel>) -> Arc<EntityModel> {
        let mut current = moqui_lib::sync::write(&self.model);
        std::mem::replace(&mut *current, model)
    }

    /// Run `work` with `model` read in place of the current model on this thread only; the other threads keep the
    /// current one.
    pub(crate) fn with_staged<R>(&self, model: &Arc<EntityModel>, work: impl FnOnce() -> R) -> R {
        /// Unstages on the way out, also when `work` panics.
        struct Unstage;
        impl Drop for Unstage {
            fn drop(&mut self) {
                STAGED.with(|staged| staged.borrow_mut().pop());
            }
        }
        STAGED.with(|staged| staged.borrow_mut().push((self.key(), Arc::clone(model))));
        let _unstage = Unstage;
        work()
    }

    fn staged(&self) -> Option<Arc<EntityModel>> {
        STAGED.with(|staged| {
            let staged = staged.borrow();
            let key = self.key();
            staged
                .iter()
                .rev()
                .find(|(held, _)| *held == key)
                .map(|(_, model)| Arc::clone(model))
        })
    }

    /// This cell's address, which tells the cells of two facades in one process apart while both live.
    fn key(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    pub(crate) fn conf_text(&self) -> &str {
        &self.conf
    }
}

thread_local! {
    /// The models staged on this thread by [`SharedModel::with_staged`], with the address of their cell.
    static STAGED: RefCell<Vec<(usize, Arc<EntityModel>)>> = const { RefCell::new(Vec::new()) };
}

const _: fn() = || {
    fn shared<T: Send + Sync>() {}
    shared::<SharedModel>();
};
