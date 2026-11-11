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

//! A session that wants its user while nobody looks sends a notification (plans/Terminal-Fancy.md §11.1), on the
//! `TerminalAttention` topic, kept and alerting, so the page, Web Push, the Desktop app and a phone show "claude in
//! mr-mantle is waiting". What notifies is the user's (the `TERMINAL_CONFIG` preference): a session that is blocked
//! (on by default), one that is done, a command that ran longer than a set time, a bell. A session's own setting
//! (`all`, `off`) comes first.

use std::sync::{Arc, Weak};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::Value;

use super::attention::Why;
use super::session::{Attend, Notice};

pub(crate) const TOPIC: &str = "TerminalAttention";
const SETTINGS_PREFERENCE: &str = "TERMINAL_CONFIG";

/// The user's notify settings, from `TERMINAL_CONFIG`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub blocked: bool,
    pub done: bool,
    pub bell: bool,
    /// A command that ran this long notifies when it finishes; 0 never.
    pub long_command_seconds: u64,
}

impl Settings {
    pub fn of(json: Option<&str>) -> Self {
        let value: serde_json::Value = json
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        let flag = |key: &str, default: bool| value.get(key).and_then(serde_json::Value::as_bool).unwrap_or(default);
        Self {
            blocked: flag("notifyBlocked", true),
            done: flag("notifyDone", false),
            bell: flag("notifyBell", false),
            long_command_seconds: value
                .get("notifyLongCommandSeconds")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        }
    }

    /// Whether `notice` notifies.
    pub fn wants(&self, notice: &Notice) -> bool {
        let attention = &notice.attention;
        match notice.notify.as_str() {
            "off" => return false,
            "all" => {
                return attention.why != Why::LongCommand || attention.took.is_some_and(|took| took.as_secs() >= 10)
            }
            _ => {}
        }
        match attention.why {
            Why::Blocked => self.blocked,
            Why::Done => self.done,
            Why::Bell => self.bell,
            Why::LongCommand => {
                self.long_command_seconds > 0
                    && attention
                        .took
                        .is_some_and(|took| took.as_secs() >= self.long_command_seconds)
            }
        }
    }
}

/// The notification's title: what the session is and where, and what it wants.
pub(crate) fn title_of(notice: &Notice) -> String {
    let dir = notice.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    let name = if notice.title.is_empty() {
        "A terminal"
    } else {
        notice.title.as_str()
    };
    let what = if dir.is_empty() || name.contains(dir) {
        name.to_string()
    } else {
        format!("{name} in {dir}")
    };
    match notice.attention.why {
        Why::Blocked => format!("{what} is waiting"),
        Why::Done => format!("{what} is done"),
        Why::Bell => format!("{what} rang"),
        Why::LongCommand => {
            let took = notice.attention.took.map_or(0, |took| took.as_secs());
            format!("{what}: the command finished after {}m {}s", took / 60, took % 60)
        }
    }
}

/// The sender of the sessions' notifications on `factory`.
pub(crate) fn hook(factory: &Arc<ExecutionContextFactory>) -> Attend {
    let weak = Arc::downgrade(factory);
    Arc::new(move |notice: Notice| {
        let weak = Weak::clone(&weak);
        crate::edge::handle().spawn(async move { send(weak, notice).await });
    })
}

async fn send(factory: Weak<ExecutionContextFactory>, notice: Notice) {
    let id = notice.id.clone();
    let sent = crate::worker::on_pool(&factory, move |_| {
        moqui_core::user::internal_login_user(&notice.username)?;
        let settings = Settings::of(
            moqui_core::user::get_preference(SETTINGS_PREFERENCE)
                .ok()
                .flatten()
                .as_deref(),
        );
        if !settings.wants(&notice) {
            return Ok(());
        }
        let Some(user_id) = moqui_core::user::current_user_id()? else {
            return Ok(());
        };
        let title = title_of(&notice);
        let mut message = moqui_lib::object::ValueMap::default();
        message.insert("sessionId".to_string(), Value::string(&notice.id));
        message.insert("why".to_string(), Value::string(notice.attention.why.as_str()));
        message.insert("text".to_string(), Value::string(&notice.attention.text));
        message.insert("sessionTitle".to_string(), Value::string(&notice.title));
        let link = format!("/qapps/Terminal?session={}", notice.id);
        moqui_core::notify::NotificationMessage::new()
            .topic(TOPIC)
            .user_id(user_id)
            .title(title)
            .link(link)
            .message_map(message)
            .send()
    })
    .await;
    if let Err(err) = sent {
        tracing::error!(
            "terminal session {id}: its notification was not sent: {}",
            err.message()
        );
    }
}

#[cfg(test)]
#[path = "notice_tests.rs"]
mod tests;
