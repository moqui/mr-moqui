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

//! What keeps an account's store and its server in step (plans/Mobile-Desktop.md §6, MD4):
//!
//! - [`push`]: the chat pushes of `/notws` folded into the store, and my own message's echo found.
//! - [`outbox`]: the operations and drafts a page keeps in the store, their checks and limits.
//! - [`flush`]: the outbox sent from a thread of its own, for every account signed in here.
//! - [`mirror`]: the current account's chat and mail read ahead into its store.
//! - [`feeds`]: the server's changes feeds (MD5) folded into the store.

pub mod feeds;
pub mod flush;
pub mod mirror;
pub mod outbox;
pub mod push;
