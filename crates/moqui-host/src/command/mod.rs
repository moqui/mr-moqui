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

//! The `moqui>` command line (plans/Terminal-Fancy.md §6): the loop that reads a line, runs it with [`shell`] and writes
//! its output, on any byte stream. SSH's admin shell runs it on a channel. `apps` at the prompt runs the terminal client
//! on the same stream, and the prompt comes back when it ends.

use std::sync::Weak;

use moqui_core::context::ExecutionContextFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) mod line;
pub(crate) mod shell;

/// The longest command line, and the longest `js` block.
const MAX_LINE: usize = 1024 * 1024;
const PROMPT: &str = "moqui> ";
/// The prompt of a `js` block's lines.
const BLOCK_PROMPT: &str = "... ";

/// What `apps` gets on a channel: the client, or why not.
#[cfg(feature = "tui")]
pub(crate) type AppsAccess = Result<crate::term::Start, &'static str>;
#[cfg(not(feature = "tui"))]
pub(crate) type AppsAccess = Result<(), &'static str>;

/// Run `apps` on the stream, or write why it cannot run. Returns the exit status.
pub(crate) async fn run_apps<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    access: AppsAccess,
    newline: &str,
) -> u32 {
    match access {
        #[cfg(feature = "tui")]
        Ok(mut start) => {
            let ended = crate::term::serve(stream, &mut start).await;
            if let Some(note) = ended.note {
                let _ = stream.write_all(format!("apps: {note}{newline}").as_bytes()).await;
            }
            ended.status
        }
        #[cfg(not(feature = "tui"))]
        Ok(()) => 1,
        Err(reason) => {
            let _ = stream.write_all(format!("{reason}{newline}").as_bytes()).await;
            1
        }
    }
}

pub(crate) fn with_newlines(text: &str, newline: &str) -> String {
    if newline == "\n" {
        text.to_string()
    } else {
        text.replace('\n', newline)
    }
}

/// The commands, for completion.
const COMMANDS: [&str; 10] = [
    "apps", "cache", "entity", "exit", "help", "js", "service", "sessions", "sh", "status",
];
/// The history kept for a user (a preference), and its longest.
const HISTORY_PREFERENCE: &str = "TERMINAL_HISTORY";
const HISTORY_MAX: usize = 500;
/// Completion lists at most this many choices.
const CHOICES_SHOWN: usize = 60;
/// Bracketed paste on, and off: on while the prompt reads, off while another program has the terminal.
const PASTE_ON: &[u8] = b"\x1b[?2004h";
const PASTE_OFF: &[u8] = b"\x1b[?2004l";

/// The user's command history, oldest first: the `TERMINAL_HISTORY` preference, a JSON list.
async fn load_history(factory: &Weak<ExecutionContextFactory>, username: &str) -> Vec<String> {
    let username = username.to_string();
    let loaded = crate::worker::on_pool(factory, move |_| {
        moqui_core::user::internal_login_user(&username)?;
        moqui_core::user::get_preference(HISTORY_PREFERENCE)
    })
    .await;
    match loaded {
        Ok(Some(text)) => serde_json::from_str::<Vec<String>>(&text).unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn save_history(factory: &Weak<ExecutionContextFactory>, username: &str, history: &[String]) {
    let Ok(text) = serde_json::to_string(history) else {
        return;
    };
    let username = username.to_string();
    let saved = crate::worker::on_pool(factory, move |_| {
        moqui_core::user::internal_login_user(&username)?;
        moqui_core::user::set_preference(HISTORY_PREFERENCE, Some(&text))
    })
    .await;
    if let Err(err) = saved {
        tracing::debug!("the command history was not kept: {}", err.message());
    }
}

/// The choices for the word being completed in `before` (the line up to the cursor).
async fn choices(factory: &Weak<ExecutionContextFactory>, username: &str, before: &str) -> (String, Vec<String>) {
    let words: Vec<&str> = before.split_whitespace().collect();
    let word = if before.ends_with(char::is_whitespace) {
        String::new()
    } else {
        words.last().map(|word| (*word).to_string()).unwrap_or_default()
    };
    let position = if word.is_empty() { words.len() } else { words.len() - 1 };
    let all: Vec<String> = match (position, words.first().copied()) {
        (0, _) => COMMANDS.iter().map(|command| (*command).to_string()).collect(),
        (1, Some("cache")) => vec!["clear".to_string()],
        (1, Some(command @ ("service" | "entity"))) => {
            let command = command.to_string();
            let username = username.to_string();
            crate::worker::on_pool(factory, move |factory| {
                moqui_core::user::internal_login_user(&username)?;
                Ok(if command == "service" {
                    moqui_core::service::info::known_service_names()
                } else {
                    factory
                        .entity()
                        .map(|facade| moqui_core::entity::info::all_entity_names(&facade, ""))
                        .unwrap_or_default()
                })
            })
            .await
            .unwrap_or_default()
        }
        (1, Some("sh")) => sh_profiles(factory, username).await,
        _ => Vec::new(),
    };
    let matching = all.into_iter().filter(|choice| choice.starts_with(&word)).collect();
    (word, matching)
}

/// The host profiles `sh` may run, for completion.
async fn sh_profiles(_factory: &Weak<ExecutionContextFactory>, _username: &str) -> Vec<String> {
    Vec::new()
}

/// Complete the word before the cursor: what the choices share is added, and with several, they are listed.
async fn complete<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    editor: &mut line::LineEditor,
    factory: &Weak<ExecutionContextFactory>,
    username: &str,
) -> std::io::Result<()> {
    let (word, choices) = choices(factory, username, &editor.before_cursor()).await;
    let mut out = Vec::new();
    match choices.as_slice() {
        [] => {}
        [only] => editor.insert(&format!("{} ", &only[word.len()..]), &mut out),
        several => {
            let shared = line::common_prefix(several);
            if shared.len() > word.len() {
                editor.insert(&shared[word.len()..], &mut out);
            } else {
                let mut list = several
                    .iter()
                    .take(CHOICES_SHOWN)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("  ");
                if several.len() > CHOICES_SHOWN {
                    list.push_str(&format!("  ... ({} in all)", several.len()));
                }
                out.extend_from_slice(b"\r\n");
                out.extend_from_slice(list.as_bytes());
                out.extend_from_slice(b"\r\n");
                editor.redraw(&mut out);
            }
        }
    }
    stream.write_all(&out).await
}

/// The interactive loop: a prompt, a line, its output. With a PTY the client does not echo, so the line editor
/// ([`line`]) echoes and edits: the cursor keys, history (kept per user), Tab completion and a bracketed paste. With
/// none (an SSH exec) lines are only gathered. `apps` runs the terminal client on the stream, and the prompt comes back
/// when it ends. Returns the exit status.
pub(crate) async fn interactive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut stream: S,
    factory: &Weak<ExecutionContextFactory>,
    username: &str,
    pty: bool,
    apps: AppsAccess,
) -> u32 {
    let newline = if pty { "\r\n" } else { "\n" };
    let history = if pty {
        load_history(factory, username).await
    } else {
        Vec::new()
    };
    let mut editor = line::LineEditor::new(PROMPT, pty, MAX_LINE, history, HISTORY_MAX);
    let greeting = format!(
        "MR-Moqui command line for {username}: help lists the commands, apps opens the applications, sh a shell{newline}"
    );
    let paste_on = if pty { PASTE_ON } else { b"" };
    if stream.write_all(greeting.as_bytes()).await.is_err()
        || stream.write_all(paste_on).await.is_err()
        || stream.write_all(PROMPT.as_bytes()).await.is_err()
    {
        return 1;
    }
    let mut block: Option<Vec<String>> = None;
    let mut block_len = 0_usize;
    let mut kept = editor.history().len();
    let mut buffer = [0u8; 4096];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let mut echo: Vec<u8> = Vec::new();
        let edits = editor.feed(&buffer[..read], &mut echo);
        if !echo.is_empty() && stream.write_all(&echo).await.is_err() {
            return 1;
        }
        for edit in edits {
            let text = match edit {
                line::Edit::Line(text) => text,
                line::Edit::TooLong => {
                    block = None;
                    editor.set_prompt(PROMPT);
                    let message = format!("the line is longer than {MAX_LINE} bytes and was dropped");
                    if refuse(&mut stream, &message, newline).await.is_err() {
                        return 1;
                    }
                    continue;
                }
                line::Edit::Cancel => {
                    block = None;
                    editor.set_prompt(PROMPT);
                    if pty && stream.write_all(PROMPT.as_bytes()).await.is_err() {
                        return 1;
                    }
                    continue;
                }
                line::Edit::End => {
                    if pty {
                        save_history(factory, username, editor.history()).await;
                    }
                    return 0;
                }
                line::Edit::Complete => {
                    if pty && complete(&mut stream, &mut editor, factory, username).await.is_err() {
                        return 1;
                    }
                    continue;
                }
            };
            if pty && editor.history().len() != kept {
                kept = editor.history().len();
                save_history(factory, username, editor.history()).await;
            }
            // A `js` block gathers lines until a line with only `.`.
            if let Some(lines) = block.as_mut() {
                block_len = block_len.saturating_add(text.len() + 1);
                if block_len > MAX_LINE {
                    block = None;
                    editor.set_prompt(PROMPT);
                    let message = format!("the js block is longer than {MAX_LINE} bytes and was dropped");
                    if refuse(&mut stream, &message, newline).await.is_err() {
                        return 1;
                    }
                    continue;
                }
                if text.trim() == "." {
                    let code = lines.join("\n");
                    block = None;
                    editor.set_prompt(PROMPT);
                    let outcome = shell::run(factory, username, &format!("js {code}")).await;
                    if write_outcome(&mut stream, &outcome.text, newline).await.is_err() {
                        return 1;
                    }
                } else {
                    lines.push(text);
                    if stream.write_all(BLOCK_PROMPT.as_bytes()).await.is_err() {
                        return 1;
                    }
                    continue;
                }
            } else if shell::starts_block(&text) {
                block = Some(Vec::new());
                block_len = 0;
                editor.set_prompt(BLOCK_PROMPT);
                if stream
                    .write_all(format!("end with a line of .{newline}{BLOCK_PROMPT}").as_bytes())
                    .await
                    .is_err()
                {
                    return 1;
                }
                continue;
            } else if text.trim() == "apps" {
                // `apps` takes the stream until it ends; a second `apps` gets a fresh window watch.
                #[cfg(feature = "tui")]
                let access = match &apps {
                    Ok(start) => Ok(start.again()),
                    Err(reason) => Err(*reason),
                };
                #[cfg(not(feature = "tui"))]
                let access = apps.clone();
                let _ = stream.write_all(if pty { PASTE_OFF } else { b"" }).await;
                run_apps(&mut stream, access, newline).await;
                let _ = stream.write_all(paste_on).await;
            } else {
                let outcome = shell::run(factory, username, &text).await;
                if outcome.exit {
                    let _ = stream.write_all(format!("bye{newline}").as_bytes()).await;
                    if pty {
                        let _ = stream.write_all(PASTE_OFF).await;
                    }
                    return 0;
                }
                if write_outcome(&mut stream, &outcome.text, newline).await.is_err() {
                    return 1;
                }
            }
            if stream.write_all(PROMPT.as_bytes()).await.is_err() {
                return 1;
            }
        }
    }
    0
}

/// An input the shell refuses: the message, then a fresh prompt.
async fn refuse<W: AsyncWriteExt + Unpin>(stream: &mut W, message: &str, newline: &str) -> std::io::Result<()> {
    stream
        .write_all(format!("error: {message}{newline}{PROMPT}").as_bytes())
        .await
}

async fn write_outcome<W: AsyncWriteExt + Unpin>(stream: &mut W, text: &str, newline: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    stream.write_all(with_newlines(text, newline).as_bytes()).await?;
    stream.write_all(newline.as_bytes()).await
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
