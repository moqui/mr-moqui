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

//! What the runner reads of the workspace after the steps: which paths a report, artifact or cache pattern names
//! ([`find_in`]), a report's bytes ([`report`]) and an artifact or cache packed as a tar.gz ([`pack`]). Without a
//! step user the runner reads them itself: the steps ran as its own user. With one (`--step-user`, the runner root)
//! each is done by a child of this binary running as the step user, `moqui ci-run collect` ([`run_as`],
//! [`collect_child`]), so the runner never lists, resolves or reads the workspace the steps wrote, and a link a step
//! made, or swapped in while the runner looked, reads only what the step could read (plans/archive/Security-Review.md
//! SEC-GIT-81).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{archive, copy_regular, find, Matches, Options};

/// The longest the runner spends reading a job's reports, artifacts and caches, all of them together.
pub const COLLECT_LIMIT: Duration = Duration::from_secs(30 * 60);

/// The arguments of the child a `--step-user` runner reads the workspace with: `moqui ci-run collect <what>`.
pub const COLLECT_ARGS: [&str; 2] = ["ci-run", "collect"];

/// What a collect child is asked for. A collect child runs on Unix, where a step can run as another user.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Collect {
    /// The paths a pattern names ([`find`]).
    Find,
    /// One report file's bytes ([`copy_regular`]).
    Report,
    /// Files and directories as a tar.gz ([`archive::pack_into`]).
    Pack,
    /// Nothing: that the child runs at all (`poll --step-user` at its start).
    Probe,
}

#[cfg(unix)]
impl Collect {
    fn word(self) -> &'static str {
        match self {
            Self::Find => "find",
            Self::Report => "report",
            Self::Pack => "pack",
            Self::Probe => "probe",
        }
    }

    fn of(word: &str) -> Option<Self> {
        [Self::Find, Self::Report, Self::Pack, Self::Probe]
            .into_iter()
            .find(|kind| kind.word() == word)
    }
}

/// The program and arguments of a collect child; [`Collector::this_binary`] but in tests.
#[derive(Clone, Debug)]
pub struct Collector {
    pub program: PathBuf,
    pub args: Vec<std::ffi::OsString>,
}

impl Collector {
    /// This binary's `ci-run collect`. On Linux the binary is run by `/proc/self/exe`, so the step user runs it when a
    /// directory on its path is closed to that user (`/root/.cargo/bin`) and when the file was replaced since the
    /// runner started; elsewhere by its path.
    ///
    /// # Errors
    ///
    /// When this binary's path is not known.
    pub fn this_binary() -> Result<Self, String> {
        let program = if cfg!(target_os = "linux") && Path::new("/proc/self/exe").exists() {
            PathBuf::from("/proc/self/exe")
        } else {
            std::env::current_exe().map_err(|err| format!("this program's path is not known: {err}"))?
        };
        Ok(Self {
            program,
            args: COLLECT_ARGS.iter().map(|arg| (*arg).into()).collect(),
        })
    }
}

/// The paths `pattern` names in the workspace, read as the step user if there is one.
pub(super) fn find_in(options: &Options, pattern: &str, deadline: Instant) -> Result<Matches, String> {
    #[cfg(unix)]
    if let Some(user) = &options.step_user {
        let out = super::Scratch::new(&options.workspace, "found");
        // the pattern is text relative to the workspace, not a path to make absolute
        let mut request = request(&options.workspace, &[])?;
        request.extend_from_slice(pattern.as_bytes());
        request.push(0);
        in_child(options, user, Collect::Find, &request, &out.0, deadline)?;
        let mut bytes = Vec::new();
        let file = std::fs::File::open(&out.0).map_err(|err| format!("{}: {err}", out.0.display()))?;
        std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_INPUT), &mut bytes)
            .map_err(|err| format!("{}: {err}", out.0.display()))?;
        return decode_matches(&bytes);
    }
    let _ = deadline;
    Ok(find(&options.workspace, pattern))
}

/// The report `file` copied to the new file `out`, read as the step user if there is one.
pub(super) fn report(options: &Options, file: &Path, out: &Path, deadline: Instant) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(user) = &options.step_user {
        let request = request(&options.workspace, &[file])?;
        return in_child(options, user, Collect::Report, &request, out, deadline);
    }
    let _ = deadline;
    let mut written = super::create_new(out)?;
    copy_regular(&options.workspace, file, &mut written)
}

/// `files` packed as a tar.gz into the new file `out`, read as the step user if there is one.
pub(super) fn pack(options: &Options, files: &[PathBuf], out: &Path, deadline: Instant) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(user) = &options.step_user {
        let paths: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
        let request = request(&options.workspace, &paths)?;
        return in_child(options, user, Collect::Pack, &request, out, deadline);
    }
    let _ = deadline;
    archive::pack(&options.workspace, files, out)
}

/// That `user` runs a collect child of this binary, which `poll --step-user` asks before its first job: the
/// child's scratch files go in `dir`, a directory of the runner's.
///
/// # Errors
///
/// Why it did not run.
#[cfg(unix)]
pub fn probe(user: &super::StepUser, dir: &Path) -> Result<(), String> {
    let out = super::Scratch::in_dir(dir, "probe");
    run_as(
        user,
        &Collector::this_binary()?,
        Collect::Probe,
        &[],
        &out.0,
        dir,
        Instant::now() + Duration::from_secs(60),
    )
}

/// [`run_as`] this binary as the step user, its input and error files beside the workspace, in the job's directory,
/// the runner's.
#[cfg(unix)]
fn in_child(
    options: &Options,
    user: &super::StepUser,
    kind: Collect,
    request: &[u8],
    out: &Path,
    deadline: Instant,
) -> Result<(), String> {
    let dir = options.workspace.parent().unwrap_or(&options.workspace);
    run_as(user, &Collector::this_binary()?, kind, request, out, dir, deadline)
}

/// The most a child reads of its input, and the runner of a [`Collect::Find`] child's answer.
#[cfg(unix)]
const MAX_INPUT: u64 = 64 * 1024 * 1024;

/// A collect child's input: the workspace and then `paths`, each made absolute against the runner's working directory
/// (the child runs in `/`), NUL-terminated.
#[cfg(unix)]
fn request(workspace: &Path, paths: &[&Path]) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt as _;

    let mut bytes = Vec::new();
    for path in std::iter::once(&workspace).chain(paths) {
        let path = std::path::absolute(path).map_err(|err| format!("{}: {err}", path.display()))?;
        bytes.extend_from_slice(path.as_os_str().as_bytes());
        bytes.push(0);
    }
    Ok(bytes)
}

/// The NUL-terminated items of a child's input or a [`Collect::Find`] answer.
#[cfg(unix)]
fn items(bytes: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;

    bytes
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
        .map(|item| PathBuf::from(std::ffi::OsStr::from_bytes(item)))
        .collect()
}

/// A [`Matches`] as a [`Collect::Find`] child writes it: each path NUL-terminated after one byte that says what it is
/// (`f` file, `l` link, `d` directory, `o` other, `r` refused).
#[cfg(unix)]
fn encode_matches(matches: &Matches) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;

    use super::Kind;

    let mut bytes = Vec::new();
    let tagged = matches
        .found
        .iter()
        .map(|(path, kind)| {
            let tag = match kind {
                Kind::File => b'f',
                Kind::Link => b'l',
                Kind::Dir => b'd',
                Kind::Other => b'o',
            };
            (tag, path)
        })
        .chain(matches.refused.iter().map(|path| (b'r', path)));
    for (tag, path) in tagged {
        bytes.push(tag);
        bytes.extend_from_slice(path.as_os_str().as_bytes());
        bytes.push(0);
    }
    bytes
}

/// [`encode_matches`] read back.
#[cfg(unix)]
pub(super) fn decode_matches(bytes: &[u8]) -> Result<Matches, String> {
    use std::os::unix::ffi::OsStrExt as _;

    use super::Kind;

    let mut matches = Matches::default();
    for item in bytes.split(|byte| *byte == 0).filter(|item| !item.is_empty()) {
        let (tag, path) = item.split_first().ok_or("an empty answer")?;
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path));
        let kind = match tag {
            b'f' => Kind::File,
            b'l' => Kind::Link,
            b'd' => Kind::Dir,
            b'o' => Kind::Other,
            b'r' => {
                matches.refused.push(path);
                continue;
            }
            other => return Err(format!("an answer of kind {} is not known", char::from(*other))),
        };
        matches.found.push((path, kind));
    }
    Ok(matches)
}

/// Run `collector` with `kind` as `user`: its uid and gid, no environment, `/` as its directory, `input` on its
/// standard input, its standard output the new file `out`, its error text in a file. The input and error files are
/// made in `dir`, the runner's, so no pipe can hold the runner. Killed at `deadline`.
#[cfg(unix)]
pub(super) fn run_as(
    user: &super::StepUser,
    collector: &Collector,
    kind: Collect,
    input: &[u8],
    out: &Path,
    dir: &Path,
    deadline: Instant,
) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    use std::os::unix::process::CommandExt as _;
    use std::process::Command;

    if Instant::now() >= deadline {
        return Err("the time to read the job's files has passed".into());
    }
    let list = super::Scratch::in_dir(dir, "collect-input");
    let errors = super::Scratch::in_dir(dir, "collect-errors");
    // the runner's files are made new, never through a name another user made first (SEC-GIT-98)
    super::create_new(&list.0)?
        .write_all(input)
        .map_err(|err| format!("{}: {err}", list.0.display()))?;
    let input = super::open_regular(&list.0)?;
    let mut child = Command::new(&collector.program)
        .args(&collector.args)
        .arg(kind.word())
        .current_dir("/")
        .env_clear()
        .stdin(input)
        .stdout(super::create_new(out)?)
        .stderr(super::create_new(&errors.0)?)
        .uid(user.uid)
        .gid(user.gid)
        .spawn()
        .map_err(|err| format!("{} did not start as {}: {err}", collector.program.display(), user.name))?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("reading as {} ran out of time and was ended", user.name));
            }
            Err(err) => break Err(format!("reading as {}: {err}", user.name)),
        }
    }?;
    if status.success() {
        return Ok(());
    }
    let mut said = String::new();
    if let Ok(file) = std::fs::File::open(&errors.0) {
        let _ = file.take(2048).read_to_string(&mut said);
    }
    Err(said
        .lines()
        .next()
        .filter(|line| !line.trim().is_empty())
        .map_or_else(|| format!("reading as {} failed: {status}", user.name), str::to_string))
}

/// `moqui ci-run collect find|report|pack|probe`, the child of [`run_as`]: [`serve`] on its standard input and
/// output. The exit code: 0, or [`super::EXIT_ERROR`] with why on standard error.
#[cfg(unix)]
pub fn collect_child(kind: &str) -> i32 {
    use std::os::fd::AsFd as _;

    let run = || -> Result<(), String> {
        let stdout = std::io::stdout()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|err| format!("standard output: {err}"))?;
        let mut out = std::io::BufWriter::new(std::fs::File::from(stdout));
        serve(kind, &mut std::io::stdin().lock(), &mut out)?;
        std::io::Write::flush(&mut out).map_err(|err| format!("standard output: {err}"))
    };
    match run() {
        Ok(()) => super::EXIT_SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            super::EXIT_ERROR
        }
    }
}

/// What a collect child does: `input` holds the workspace and then the paths ([`Collect::Report`]: one file,
/// [`Collect::Pack`]: files and directories) or the pattern ([`Collect::Find`]), NUL-terminated; the answer goes to
/// `out`.
///
/// # Errors
///
/// When the request is not one, or what it names is refused or does not read.
#[cfg(unix)]
pub(super) fn serve(kind: &str, input: &mut dyn std::io::Read, out: &mut dyn std::io::Write) -> Result<(), String> {
    use std::io::Read as _;

    let kind =
        Collect::of(kind).ok_or_else(|| format!("moqui ci-run collect: {kind} is not find, report, pack or probe"))?;
    if kind == Collect::Probe {
        return Ok(());
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("the request did not read: {err}"))?;
    let mut items = items(&bytes).into_iter();
    let workspace = items.next().ok_or("no workspace was given")?;
    let rest: Vec<PathBuf> = items.collect();
    match kind {
        Collect::Find => {
            let [pattern] = rest.as_slice() else {
                return Err("find takes one pattern".into());
            };
            let pattern = pattern.to_str().ok_or("the pattern is not UTF-8")?;
            out.write_all(&encode_matches(&find(&workspace, pattern)))
                .map_err(|err| format!("the answer was not written: {err}"))
        }
        Collect::Report => {
            let [file] = rest.as_slice() else {
                return Err("a report is one file".into());
            };
            copy_regular(&workspace, file, out)
        }
        Collect::Pack => archive::pack_into(&workspace, &rest, out),
        Collect::Probe => Ok(()),
    }
}
