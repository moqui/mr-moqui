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

//! CalcGrid's socket frames in a host built without the `xlsx` feature (no moqui-calc): no frame is taken, and a
//! `calc` frame goes on to the topic commands, which ignore it.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

use super::Slot;

pub(super) fn handle(_factory: &Arc<ExecutionContextFactory>, _slot: &Arc<Slot>, _text: &str) -> bool {
    false
}

pub(super) fn socket_closed(_factory: &ExecutionContextFactory, _slot_id: u64) {}

pub(crate) fn install(_factory: &Arc<ExecutionContextFactory>, _shutdown: tokio::sync::watch::Receiver<bool>) {}
