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

pub(crate) mod shell;

/// The longest command line, and the longest `js` block.
const MAX_LINE: usize = 1024 * 1024;
const PROMPT: &str = "moqui> ";

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

/// The interactive loop: a prompt, a line, its output. With a PTY the client does not echo, so this echoes and
/// handles backspace, ctrl-C (drop the line) and ctrl-D (end on an empty line). `apps` runs the terminal client on
/// the stream, and the prompt comes back when it ends. Returns the exit status.
pub(crate) async fn interactive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut stream: S,
    factory: &Weak<ExecutionContextFactory>,
    username: &str,
    pty: bool,
    apps: AppsAccess,
) -> u32 {
    let newline = if pty { "\r\n" } else { "\n" };
    let greeting = format!("MR-Moqui admin shell for {username}; type help{newline}{PROMPT}");
    if stream.write_all(greeting.as_bytes()).await.is_err() {
        return 1;
    }
    let mut line: Vec<u8> = Vec::new();
    let mut block: Option<Vec<String>> = None;
    // Bytes past MAX_LINE are not kept; the line is refused when it ends.
    let mut overflow = false;
    let mut block_len = 0_usize;
    let mut buffer = [0u8; 1024];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return 0,
            Ok(read) => read,
        };
        for &byte in &buffer[..read] {
            let mut echo: Vec<u8> = Vec::new();
            let mut finished = None;
            match byte {
                b'\r' | b'\n' => {
                    if byte == b'\n' && pty {
                        continue;
                    }
                    if pty {
                        echo.extend_from_slice(b"\r\n");
                    }
                    finished = Some(String::from_utf8_lossy(&line).into_owned());
                    line.clear();
                }
                0x7f | 0x08 => {
                    if line.pop().is_some() && pty {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                0x03 => {
                    line.clear();
                    overflow = false;
                    block = None;
                    if pty {
                        echo.extend_from_slice(format!("^C{newline}{PROMPT}").as_bytes());
                    }
                }
                0x04 if line.is_empty() => return 0,
                byte if byte >= 0x20 || byte == b'\t' => {
                    if line.len() < MAX_LINE {
                        line.push(byte);
                        if pty {
                            echo.push(byte);
                        }
                    } else {
                        overflow = true;
                    }
                }
                _ => {}
            }
            if !echo.is_empty() && stream.write_all(&echo).await.is_err() {
                return 1;
            }
            let Some(text) = finished else {
                continue;
            };
            if std::mem::take(&mut overflow) {
                block = None;
                let message = format!("the line is longer than {MAX_LINE} bytes and was dropped");
                if refuse(&mut stream, &message, newline).await.is_err() {
                    return 1;
                }
                continue;
            }
            // A `js` block gathers lines until a line with only `.`.
            if let Some(lines) = block.as_mut() {
                block_len = block_len.saturating_add(text.len() + 1);
                if block_len > MAX_LINE {
                    block = None;
                    let message = format!("the js block is longer than {MAX_LINE} bytes and was dropped");
                    if refuse(&mut stream, &message, newline).await.is_err() {
                        return 1;
                    }
                    continue;
                }
                if text.trim() == "." {
                    let code = lines.join("\n");
                    block = None;
                    let outcome = shell::run(factory, username, &format!("js {code}")).await;
                    if write_outcome(&mut stream, &outcome.text, newline).await.is_err() {
                        return 1;
                    }
                } else {
                    lines.push(text);
                    if stream.write_all(b"... ").await.is_err() {
                        return 1;
                    }
                    continue;
                }
            } else if shell::starts_block(&text) {
                block = Some(Vec::new());
                block_len = 0;
                if stream
                    .write_all(format!("end with a line of .{newline}... ").as_bytes())
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
                let access = apps;
                run_apps(&mut stream, access, newline).await;
            } else {
                let outcome = shell::run(factory, username, &text).await;
                if outcome.exit {
                    let _ = stream.write_all(format!("bye{newline}").as_bytes()).await;
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
