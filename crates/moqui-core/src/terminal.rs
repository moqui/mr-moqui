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

//! The browser terminal's `thread` kind's policy (plans/Terminal-Fancy2.md §4.7, D9, D16): a host session opened,
//! attached and listed by an agent conversation. The host has the pty, the ring, the holder, the history, the viewers
//! and the audit; who may open one, in which directory, as which account and with what environment is a policy a crate
//! gives ([`crate::extension::Extension::terminal_policies`], or [`register_policy`] at run time). With none, `thread`
//! is not available.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::context::ExecutionContextFactory;

/// What an opening of a thread terminal asks for: the conversation and the client's own id of the terminal, and what
/// the component keeps for itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadOpening {
    pub username: String,
    pub conversation_id: String,
    pub client_terminal_id: String,
    /// The directory asked for (a work root's path, or one inside it).
    pub directory: String,
    pub work_root_id: String,
    pub provider_instance_id: String,
}

/// What a policy allows for an opening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadPlan {
    /// The directory, checked (inside the work root).
    pub directory: PathBuf,
    /// An OS account it runs as (Agent Code's `run-as`), through the host's sudo-gated `term-exec`; none is the
    /// server's own.
    pub run_as: Option<String>,
    /// Variables it adds to the program's environment.
    pub env: Vec<(String, String)>,
    /// It needs a dev instance, as a host session does.
    pub needs_dev_instance: bool,
    /// The program and its arguments; empty is the shell.
    pub argv: Vec<String>,
}

/// A crate's policy for thread terminals.
pub trait ThreadTerminalPolicy: Send + Sync {
    /// Whether the user on this thread may open `opening`, and how; `Err` with why not. Called as that user.
    ///
    /// # Errors
    ///
    /// Why the opening is refused.
    fn plan(&self, opening: &ThreadOpening) -> Result<ThreadPlan, String>;
}

#[derive(Default)]
struct Policies {
    linked_read: std::sync::atomic::AtomicBool,
    list: RwLock<Vec<Arc<dyn ThreadTerminalPolicy>>>,
}

fn store(factory: &ExecutionContextFactory) -> Arc<Policies> {
    let policies = factory.extension(|_| Policies::default());
    if !policies.linked_read.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let mut list = policies.list.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        for extension in crate::extension::linked() {
            list.extend((extension.terminal_policies)());
        }
    }
    policies
}

/// Add a policy to this factory (a component's, or a test's).
pub fn register_policy(factory: &ExecutionContextFactory, policy: Arc<dyn ThreadTerminalPolicy>) {
    store(factory)
        .list
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(policy);
}

/// The policies of this factory: the linked crates' and those registered.
#[must_use]
pub fn policies(factory: &ExecutionContextFactory) -> Vec<Arc<dyn ThreadTerminalPolicy>> {
    store(factory)
        .list
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}
