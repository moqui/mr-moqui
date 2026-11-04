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

//! What the app's schemes share (plans/Mobile-Desktop.md §4 "One transport seam"): the client's two (`client::scheme`)
//! and the node's in process (`node::scheme`). Chromium calls a scheme's handler on the main thread; each request goes
//! to a worker thread. The workers and the handling CEF's HTML answers need are `moqui_client_tauri::scheme`'s, shared
//! with the mobile app (tested there).

pub(crate) use moqui_client_tauri::scheme::{document_request, has_script_policy, plain, run};
