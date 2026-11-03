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

//! Who owns the item being handled (plans/Edge-Node.md §10, E12): the server itself, or a paired local node. An
//! all-in-one node is a full server that also takes a paired node's link, so each engine (mail, chat, calls, SIP)
//! decides an item's owner when it takes the item, and runs the work for a paired node's item under [`with`]. The
//! service facade reads [`scoped`] to send a delegated service's call down that node's link, and runs the same service
//! here for the server's own items ([`super::delegate`]).
//!
//! The scope is a thread-local: core is synchronous, on `std::thread`. A caller on a Tokio task sets it inside the job
//! that runs the service calls on a request worker (`moqui_host::worker::on_pool`), never around an `.await`, so the
//! scope is on the thread the calls run on and ends with the job. It does not reach an async service call or a job
//! another thread runs.

use std::cell::RefCell;
use std::sync::Arc;

use crate::context::{Error, ExecutionContextFactory};

/// The owner of an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The server itself: its own accounts, bridges, numbers and calls, handled as on a normal node.
    Own,
    /// A paired local node, by its `pairedNodeId`: its items go over its link.
    Paired(Arc<str>),
}

impl Owner {
    /// The owner of a paired node's items.
    pub fn paired(node: &str) -> Self {
        Self::Paired(Arc::from(node))
    }

    /// The paired node, `None` for the server's own.
    pub fn node(&self) -> Option<&str> {
        match self {
            Self::Own => None,
            Self::Paired(node) => Some(node),
        }
    }
}

thread_local! {
    static SCOPE: RefCell<Option<Owner>> = const { RefCell::new(None) };
}

/// Puts the scope back as it was when dropped, also when `f` panics.
struct Restore(Option<Owner>);

impl Drop for Restore {
    fn drop(&mut self) {
        let before = self.0.take();
        SCOPE.with(|scope| *scope.borrow_mut() = before);
    }
}

/// Run `f` with `owner` as the owner of what it handles on this thread; the scope before is back after it. Scopes
/// nest: the innermost holds.
pub fn with<R>(owner: Owner, f: impl FnOnce() -> R) -> R {
    let before = SCOPE.with(|scope| scope.borrow_mut().replace(owner));
    let _restore = Restore(before);
    f()
}

/// The owner [`with`] set on this thread, `None` outside any scope.
pub fn scoped() -> Option<Owner> {
    SCOPE.with(|scope| scope.borrow().clone())
}

/// The owner of what this thread handles on `factory`'s node: the scope [`with`] set, else by the node's role.
/// - A dedicated edge node has no items of its own: the only active paired node (an error when none, or several, are
///   paired), as its queue picks a node for an item that names none.
/// - An all-in-one node and a normal node: [`Owner::Own`].
pub fn current(factory: &ExecutionContextFactory) -> Result<Owner, Error> {
    if let Some(owner) = scoped() {
        return Ok(owner);
    }
    if !super::settings(factory).edge_node() {
        return Ok(Owner::Own);
    }
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("this edge node has no entity facade to find its paired node"))?;
    Ok(Owner::paired(&super::queue::only_paired_node(&facade)?))
}

#[cfg(test)]
#[path = "owner_tests.rs"]
mod tests;
