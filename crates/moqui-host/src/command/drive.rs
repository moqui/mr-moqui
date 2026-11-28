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

//! `capture`, `send` and `wait` at the `moqui>` prompt (plans/Terminal-Fancy2.md §4.3): the user's other sessions
//! read, typed into and waited on from the prompt, as `moqui term` does from a shell, with what attaching asks (the
//! kind's permission, the place, a recent sign-in).

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::attach;
use super::host::HostAccess;
use crate::term::drive::{matched, Until};
use crate::term::screen::Scope;
use crate::term::session::{Kind, Session};

/// The longest a `wait` at the prompt waits.
const WAIT_MAX: Duration = Duration::from_secs(300);

/// The session `id` names, when the prompt's user may drive it.
async fn target(access: &HostAccess, id: &str) -> Result<Arc<Session>, String> {
    let Some(factory) = access.factory.upgrade() else {
        return Err("the server is stopping".to_string());
    };
    let session = attach::find(&factory, id, &access.username)?;
    if session.kind == Kind::Apps {
        return Err("an apps session has no screen to read".to_string());
    }
    if session.id == access.session_id {
        return Err("that is this command line".to_string());
    }
    attach::may_attach(access, session.kind).await?;
    Ok(session)
}

/// The value after `--name` in `words`, and `words` without both.
fn take<'a>(words: &mut Vec<&'a str>, name: &str) -> Option<&'a str> {
    let at = words.iter().position(|word| *word == name)?;
    words.remove(at);
    (at < words.len()).then(|| words.remove(at))
}

fn flag(words: &mut Vec<&str>, name: &str) -> bool {
    let found = words.iter().position(|word| *word == name);
    if let Some(at) = found {
        words.remove(at);
    }
    found.is_some()
}

/// `capture <id> [last N | scrollback]`: the session's screen as text.
pub(crate) async fn capture(access: &HostAccess, rest: &str) -> String {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let Some(id) = words.first() else {
        return "capture <sessionId> [last N | scrollback]".to_string();
    };
    let scope = match words.get(1).copied() {
        Some("scrollback") => Scope::Scrollback,
        Some("last") => Scope::Last(words.get(2).and_then(|n| n.parse().ok()).unwrap_or(10)),
        _ => Scope::Visible,
    };
    let session = match target(access, id).await {
        Ok(session) => session,
        Err(why) => return format!("capture: {why}"),
    };
    match session.screen(scope, false).await {
        Some(read) => read.lines.join("\n"),
        None => "capture: that session has no screen".to_string(),
    }
}

/// `send <id> [--keys Enter,C-c] text...`: the text as typed, then the keys.
pub(crate) async fn send(access: &HostAccess, rest: &str) -> String {
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    let keys: Vec<String> = take(&mut words, "--keys")
        .map(|keys| {
            keys.split(',')
                .filter(|key| !key.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if words.is_empty() {
        return "send <sessionId> [--keys Enter,C-c,...] [text...]".to_string();
    }
    let id = words.remove(0);
    let session = match target(access, id).await {
        Ok(session) => session,
        Err(why) => return format!("send: {why}"),
    };
    match session.send_keys(words.join(" "), keys).await {
        Ok(_) => String::new(),
        Err(why) => format!("send: {why}"),
    }
}

/// `wait <id> [--status s,s] [--pattern RE] [--prompt] [--quiet-ms N] [--ended] [--timeout S]`: until one holds;
/// prints what matched and the session's last lines.
pub(crate) async fn wait(access: &HostAccess, rest: &str) -> String {
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    let statuses: Vec<String> = take(&mut words, "--status")
        .map(|list| list.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    let pattern = take(&mut words, "--pattern").unwrap_or_default();
    let quiet = take(&mut words, "--quiet-ms").and_then(|ms| ms.parse::<i64>().ok());
    let timeout = take(&mut words, "--timeout")
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(WAIT_MAX, Duration::from_secs)
        .min(WAIT_MAX);
    let prompt = flag(&mut words, "--prompt");
    let ended = flag(&mut words, "--ended");
    let Some(id) = words.first() else {
        return "wait <sessionId> [--status s,s] [--pattern RE] [--prompt] [--quiet-ms N] [--ended] [--timeout S]"
            .to_string();
    };
    let until = match Until::parse(statuses, pattern, prompt, quiet, ended) {
        Ok(until) => until,
        Err(why) => return format!("wait: {why}"),
    };
    let session = match target(access, id).await {
        Ok(session) => session,
        Err(why) => return format!("wait: {why}"),
    };
    let deadline = Instant::now() + timeout;
    let from_event = session.last_event();
    let found = loop {
        let screen = match until.pattern {
            Some(_) => {
                session
                    .screen(Scope::Last(usize::from(session.info().rows.max(1))), false)
                    .await
            }
            None => None,
        };
        if let Some(found) = matched(&session, &until, from_event, screen.as_ref()) {
            break found;
        }
        if Instant::now() >= deadline {
            break "timed out";
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let tail = session
        .screen(Scope::Last(5), false)
        .await
        .map(|read| read.lines.join("\n"))
        .unwrap_or_default();
    format!("wait: {found}\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::{flag, take};

    #[test]
    fn the_prompt_takes_its_options_out_of_the_words() {
        let mut words = vec!["s1", "--keys", "Enter", "ls", "--prompt"];
        assert_eq!(take(&mut words, "--keys"), Some("Enter"));
        assert!(flag(&mut words, "--prompt"));
        assert_eq!(words, ["s1", "ls"]);
    }
}
