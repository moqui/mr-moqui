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

//! A program on a pseudo-terminal (plans/Terminal-Fancy.md §7.2, D8), on `rustix` with no `unsafe`. The server opens
//! the pty and starts this binary as `moqui term-exec` with the pty's slave as its stdin, stdout and stderr; the helper
//! makes itself a session leader with the pty as its controlling terminal and then becomes the program, so the pid the
//! server holds is the program's. The program's argv comes in [`EXEC_ENV`], not in the helper's own arguments: the
//! Desktop app's binary reads any `--type=` argument as a browser helper process. The master is read and written on
//! the edge runtime, non-blocking; the program's end is a read of 0 or `EIO`, after what was left is read out.

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use rustix::fs::{Mode, OFlags};
use rustix::process::{Pid, Signal};
use rustix::pty::OpenptFlags;
use rustix::termios::Winsize;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The program's argv and its `arg0`, as JSON, for `moqui term-exec`.
pub(crate) const EXEC_ENV: &str = "MOQUI_TERM_EXEC";
/// The command that runs the helper, as a JSON list (its program and the arguments before the program's): this binary
/// and `term-exec` unless the property names another, as a test binary does.
const HELPER_PROPERTY: &str = "terminal_exec_helper";

/// The program `term-exec` becomes: `{"argv": [...], "arg0": "-bash"}`, `arg0` the name it sees as its own (a login
/// shell's starts with `-`).
struct ExecSpec {
    argv: Vec<String>,
    arg0: Option<String>,
}

impl ExecSpec {
    fn to_json(&self) -> String {
        serde_json::json!({"argv": self.argv, "arg0": self.arg0}).to_string()
    }

    fn from_json(text: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let argv = value
            .get("argv")
            .and_then(serde_json::Value::as_array)
            .ok_or("no argv")?
            .iter()
            .map(|arg| arg.as_str().map(str::to_string).ok_or("an argument is not text"))
            .collect::<Result<Vec<String>, &str>>()?;
        let arg0 = value
            .get("arg0")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        Ok(Self { argv, arg0 })
    }
}

/// `moqui term-exec`: become the session leader with stdin as the controlling terminal, then the program [`EXEC_ENV`]
/// names. Answers only when it could not: the exit status, after a message on stderr (the pty, so the user sees it).
pub fn term_exec() -> i32 {
    let fail = |message: String| {
        eprintln!("{message}");
        127
    };
    let stdin = io::stdin();
    if !rustix::termios::isatty(stdin.as_fd()) {
        return fail("term-exec runs a program on a terminal; its stdin is not one".to_string());
    }
    let Some(spec) = std::env::var_os(EXEC_ENV) else {
        return fail(format!("term-exec needs {EXEC_ENV}"));
    };
    let spec = match ExecSpec::from_json(&spec.to_string_lossy()) {
        Ok(spec) => spec,
        Err(err) => return fail(format!("term-exec: {EXEC_ENV} is not a program: {err}")),
    };
    let Some((program, args)) = spec.argv.split_first() else {
        return fail("term-exec: no program".to_string());
    };
    if let Err(err) = rustix::process::setsid() {
        return fail(format!("term-exec: setsid: {err}"));
    }
    if let Err(err) = rustix::process::ioctl_tiocsctty(stdin.as_fd()) {
        return fail(format!("term-exec: the terminal did not become this session's: {err}"));
    }
    let mut command = std::process::Command::new(program);
    command.args(args).env_remove(EXEC_ENV);
    if let Some(arg0) = &spec.arg0 {
        command.arg0(arg0);
    }
    // exec answers only with the error
    let err = command.exec();
    fail(format!("{program}: {err}"))
}

/// A started program: the pty's master both ways, and how to resize, hang up and end it.
pub(crate) struct Started {
    pub reader: PtyReader,
    pub writer: PtyWriter,
    pub master: Arc<AsyncFd<OwnedFd>>,
    pub pid: Option<Pid>,
    pub child: std::process::Child,
}

fn winsize(cols: u16, rows: u16) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// The pty's master: close-on-exec (a flag of `openpt` on Linux; set after it elsewhere, or every later child of the
/// server would hold it), not the server's own terminal.
fn open_master() -> io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
    #[cfg(not(target_os = "linux"))]
    let master = {
        let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        master
    };
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    Ok(master)
}

/// The helper's command line: `terminal_exec_helper` (a JSON list), else this binary and `term-exec`.
fn helper() -> io::Result<(String, Vec<String>)> {
    let configured = moqui_lib::env::get_prop_or_env(HELPER_PROPERTY);
    if !configured.is_empty() {
        let list: Vec<String> = serde_json::from_str(&configured)
            .map_err(|err| io::Error::other(format!("{HELPER_PROPERTY} is not a JSON list: {err}")))?;
        if let Some((program, args)) = list.split_first() {
            return Ok((program.clone(), args.to_vec()));
        }
    }
    let exe = std::env::current_exe()?;
    Ok((exe.to_string_lossy().into_owned(), vec!["term-exec".to_string()]))
}

/// Start `argv` (with `arg0` as its own name) in `directory` with exactly `env`, on a new pty of `cols` by `rows`.
/// Must be called in the edge runtime's context (`crate::edge::handle().enter()`), which reads the master.
pub(crate) fn start(
    argv: &[String],
    arg0: Option<&str>,
    directory: &Path,
    env: &[(String, String)],
    cols: u16,
    rows: u16,
) -> io::Result<Started> {
    let master = open_master()?;
    rustix::termios::tcsetwinsize(&master, winsize(cols, rows))?;
    let name = rustix::pty::ptsname(&master, Vec::new())?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let spec = ExecSpec {
        argv: argv.to_vec(),
        arg0: arg0.map(str::to_string),
    }
    .to_json();
    let (helper, helper_args) = helper()?;
    let child = {
        let mut command = std::process::Command::new(helper);
        command
            .args(helper_args)
            .env_clear()
            .envs(env.iter().map(|(name, value)| (name, value)))
            .env(EXEC_ENV, spec)
            .current_dir(directory)
            .stdin(std::process::Stdio::from(slave.try_clone()?))
            .stdout(std::process::Stdio::from(slave.try_clone()?))
            .stderr(std::process::Stdio::from(slave));
        // the command, and the server's copies of the slave with it, go when this block ends, or the program's end
        // would never be seen
        command.spawn()?
    };
    rustix::io::ioctl_fionbio(&master, true)?;
    // AsyncFd::new is deprecated for a T that may not own its descriptor (tokio#8587); an OwnedFd owns it, and is
    // closed only when the AsyncFd is dropped, so it is sound here without the unsafe register.
    #[allow(deprecated)]
    let master = Arc::new(AsyncFd::new(master)?);
    let pid = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    Ok(Started {
        reader: PtyReader(Arc::clone(&master)),
        writer: PtyWriter(Arc::clone(&master)),
        master,
        pid,
        child,
    })
}

/// The window's new size.
pub(crate) fn resize(master: &AsyncFd<OwnedFd>, cols: u16, rows: u16) {
    if let Err(err) = rustix::termios::tcsetwinsize(master.get_ref(), winsize(cols, rows)) {
        tracing::debug!("a terminal's size was not set: {err}");
    }
}

/// A signal to the program's process group (it is its session's leader, so its group is its pid).
pub(crate) fn signal(pid: Option<Pid>, signal: Signal) {
    if let Some(pid) = pid {
        if let Err(err) = rustix::process::kill_process_group(pid, signal) {
            tracing::debug!("a terminal program's group was not signalled: {err}");
        }
    }
}

/// The master read as the program's output; `EIO` (Linux, after the last slave closed) is the end.
pub(crate) struct PtyReader(Arc<AsyncFd<OwnedFd>>);

impl AsyncRead for PtyReader {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|fd| rustix::io::read(fd.get_ref(), &mut *unfilled).map_err(io::Error::from)) {
                Ok(Ok(read)) => {
                    buf.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) if err.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => {
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => {}
            }
        }
    }
}

/// The master written as the program's input.
pub(crate) struct PtyWriter(Arc<AsyncFd<OwnedFd>>);

impl AsyncWrite for PtyWriter {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            match guard.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(io::Error::from)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => {}
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
