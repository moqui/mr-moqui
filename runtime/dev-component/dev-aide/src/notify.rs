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

//! Reload reports as notifications (plans/design-dev-aide.md 6.4): each report of the framework (a save, or the file
//! watcher seeing an edit from outside) goes to the dev-aide's users as a `DevReload` notification, which the open
//! Workspace pages take to refresh their tree, their status and the tabs of the files. The listener is installed by the
//! first dev-aide service a factory runs (the Workspace asks for its roots when it opens), so the component needs no tool
//! factory: a binary without the dev tools still boots with the component's screens.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::devtools::ReloadReport;
use moqui_lib::object::Value;

/// The notification topic of reload reports.
pub const RELOAD_TOPIC: &str = "DevReload";

/// The groups whose users get the reports.
const GROUPS: &[&str] = &["ADMIN", "DEV_AIDE_USERS", "DEV_AIDE_EXTENDERS"];

/// Marks a factory whose reload reports are sent.
struct Listening;

/// Send `factory`'s reload reports from now on, once per factory.
pub(crate) fn ensure(factory: &Arc<ExecutionContextFactory>) {
    factory.extension::<Listening>(|factory| {
        moqui_core::devtools::on_reload(factory, send);
        Listening
    });
}

/// The dev-aide's users: the members of its groups.
fn dev_users(facade: &moqui_core::entity::EntityFacade) -> Vec<String> {
    let mut users = Vec::new();
    for group in GROUPS {
        let Ok(rows) = facade
            .find("moqui.security.UserGroupMember")
            .and_then(|find| find.eq("userGroupId", Value::string(*group)).list())
        else {
            continue;
        };
        for row in rows.iter() {
            if let Some(user) = row.get("userId").map(moqui_lib::object::to_plain_string) {
                if !users.contains(&user) {
                    users.push(user);
                }
            }
        }
    }
    users
}

/// `json` on `topic` to the dev-aide's users but `except`, neither kept nor alerted.
fn send_to_users(facade: &moqui_core::entity::EntityFacade, topic: &str, json: String, except: Option<&str>) {
    let users: Vec<String> = dev_users(facade)
        .into_iter()
        .filter(|user| Some(user.as_str()) != except)
        .collect();
    if users.is_empty() {
        return;
    }
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(topic)
        .user_ids(users)
        .message_json(json)
        .persist(false)
        .show_alert(false)
        .send();
    if let Err(err) = sent {
        tracing::warn!("Could not send the Dev AIDE {topic} notification: {err}");
    }
}

/// The report as a notification to the dev-aide's users, on its own context.
fn send(factory: &Arc<ExecutionContextFactory>, report: &ReloadReport) {
    if report.files.is_empty() {
        return;
    }
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let Some(facade) = factory.entity() else {
        return;
    };
    send_to_users(
        &facade,
        RELOAD_TOPIC,
        serde_json::to_string(report).unwrap_or_default(),
        None,
    );
}

/// The notification topic of who has which files open on a shared dev server (D19).
pub const PRESENCE_TOPIC: &str = "DevPresence";
/// The notification topic of a save on a shared dev server, to the other users.
pub const SAVED_TOPIC: &str = "DevSaved";

/// How long an editor's presence lasts without the page saying so again.
const PRESENCE_TTL: Duration = Duration::from_secs(120);

struct Present {
    username: String,
    root: String,
    paths: Vec<String>,
    at: Instant,
}

fn presence() -> &'static Mutex<HashMap<String, Present>> {
    static PRESENCE: OnceLock<Mutex<HashMap<String, Present>>> = OnceLock::new();
    PRESENCE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The editors of `root` but `except`, each with the open files, the expired ones dropped.
pub(crate) fn editors(root: &str, except: &str) -> serde_json::Value {
    let mut present = presence().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    present.retain(|_, entry| entry.at.elapsed() < PRESENCE_TTL);
    let editors: Vec<serde_json::Value> = present
        .iter()
        .filter(|(user, entry)| entry.root == root && user.as_str() != except && !entry.paths.is_empty())
        .map(|(user, entry)| serde_json::json!({ "userId": user, "username": entry.username, "paths": entry.paths }))
        .collect();
    serde_json::json!({ "rootId": root, "editors": editors })
}

/// The current user's open files of `root` (empty when the page closes), told to the other users when they changed.
pub(crate) fn set_presence(
    facade: &moqui_core::entity::EntityFacade,
    user: &str,
    username: &str,
    root: &str,
    paths: Vec<String>,
) {
    let changed = {
        let mut present = presence().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = present
            .get(user)
            .is_none_or(|entry| entry.root != root || entry.paths != paths);
        present.insert(
            user.to_string(),
            Present {
                username: username.to_string(),
                root: root.to_string(),
                paths,
                at: Instant::now(),
            },
        );
        changed
    };
    if changed {
        send_to_users(facade, PRESENCE_TOPIC, editors(root, "").to_string(), Some(user));
    }
}

/// A save told to the other users, whose clean tabs of the file take the new text through the reload report.
pub(crate) fn announce_save(
    facade: &moqui_core::entity::EntityFacade,
    user: &str,
    username: &str,
    root: &str,
    path: &str,
) {
    let json = serde_json::json!({ "rootId": root, "path": path, "userId": user, "username": username });
    send_to_users(facade, SAVED_TOPIC, json.to_string(), Some(user));
}

/// The notification topic of a long git command's progress lines.
pub const PROGRESS_TOPIC: &str = "DevGitProgress";

/// A progress line of operation `operation` to `user`, on its own context (it comes from the command's reader thread).
pub(crate) fn send_progress(factory: &Arc<ExecutionContextFactory>, user: &str, operation: &str, line: &str) {
    if user.is_empty() {
        return;
    }
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let json = serde_json::json!({ "operationId": operation, "line": line });
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(PROGRESS_TOPIC)
        .user_ids(vec![user.to_string()])
        .message_json(json.to_string())
        .persist(false)
        .show_alert(false)
        .send();
    if let Err(err) = sent {
        tracing::warn!("Could not send the Dev AIDE git progress: {err}");
    }
}
