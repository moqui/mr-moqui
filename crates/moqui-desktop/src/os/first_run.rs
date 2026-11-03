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

//! First run (plans/Mobile-Desktop.md §4 configuration 1): once the first user (the initial admin an empty runtime
//! asks for) is signed in, the window opens the attach wizard (`/qapps/system/EdgeNode/Pair`, plans/Edge-Node.md
//! §5.7) to pair the node with an edge node. It does this once, kept by a marker file in the app config directory, and
//! not on a node that is already paired or that takes links itself. The wizard has its own way out, so it can be
//! skipped.
//!
//! The host does not see the sign-in; it sees the window load a page under `/qapps`, which the node serves only to a
//! signed-in user (`qapps.xml` sends anyone else to `/Login`, outside `/qapps`).

use moqui_core::edge::{NodeRole, NODE_ROLE, NODE_URL};
use tauri::Url;

/// The marker file in the app config directory: the first run is done.
pub(crate) const MARKER: &str = "first-run-done";

/// The attach wizard's path.
pub(crate) const WIZARD_PATH: &str = "/qapps/system/EdgeNode/Pair";

/// Whether the window, having loaded `url`, shows a signed-in page of the node at `node`: a page under `/qapps` on its
/// origin, other than the edge node screens (the wizard itself, or the user went there).
pub(crate) fn signed_in_page(url: &Url, node: &Url) -> bool {
    let path = url.path();
    url.origin() == node.origin()
        && (path == "/qapps" || path.starts_with("/qapps/"))
        && !path.starts_with("/qapps/system/EdgeNode")
}

/// The attach wizard on the node at `node`.
pub(crate) fn wizard_url(node: &Url) -> Url {
    let mut url = node.clone();
    url.set_path(WIZARD_PATH);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Whether the node needs pairing: it has no `edge_node_url` (pairing writes it to `runtime/.env`, §5.1) and its
/// `node_role` is not one that takes links (an edge or all-in-one node). `property` is the node's own value where the
/// host can read it (the node in process), `env_file` the runtime's `.env` (for a sidecar node too).
pub(crate) fn needs_pairing(env_file: &str, property: impl Fn(&str) -> String) -> bool {
    let dotenv = moqui_lib::env::parse_dotenv(env_file);
    let value = |name: &str| {
        let own = property(name);
        if !own.trim().is_empty() {
            return own;
        }
        dotenv
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    value(NODE_URL).trim().is_empty() && !NodeRole::parse(&value(NODE_ROLE)).takes_links()
}

#[cfg(test)]
#[path = "first_run_tests.rs"]
mod tests;
