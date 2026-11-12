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

//! The client side of git-receive-pack, a push to another server (protocol v0, one stateful conversation, as over
//! SSH): read the server's refs, send a command for each ref to change and one pack of the objects it lacks, and
//! read its `report-status`. A push mirror forces each ref to the local one and deletes nothing.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use gix::ObjectId;

use super::pktline::{self, Packet};
use super::{io_error, pack_out, plan};
use crate::store::GitStore;
use moqui_core::context::Error;

/// What a push mirror sends when its `pushMirrorRefs` is empty: every branch and tag, under the same names.
const ALL: [(&str, &str); 2] = [("refs/heads/*", "refs/heads/*"), ("refs/tags/*", "refs/tags/*")];

/// One mapping of local refs to remote ones: a full ref name, or a prefix ending in `*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefMap {
    pub from: String,
    pub to: String,
}

fn full_name(name: &str) -> String {
    if name.starts_with("refs/") {
        name.to_string()
    } else {
        format!("refs/heads/{name}")
    }
}

/// `pushMirrorRefs`: `from:to` pairs separated by spaces or commas, a name without `refs/` being a branch
/// (`master:mirror` is `refs/heads/master:refs/heads/mirror`), and `refs/heads/*:refs/heads/*` mapping a prefix.
/// Empty is every branch and tag.
pub(crate) fn parse_ref_maps(text: &str) -> Result<Vec<RefMap>, Error> {
    let entries: Vec<&str> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|entry| !entry.is_empty())
        .collect();
    if entries.is_empty() {
        return Ok(ALL
            .iter()
            .map(|(from, to)| RefMap {
                from: (*from).to_string(),
                to: (*to).to_string(),
            })
            .collect());
    }
    entries
        .into_iter()
        .map(|entry| {
            let entry = entry.trim_start_matches('+');
            let (from, to) = entry.split_once(':').unwrap_or((entry, entry));
            let (from, to) = (full_name(from), full_name(to));
            if from.ends_with('*') != to.ends_with('*') || from.matches('*').count() > 1 || to.matches('*').count() > 1
            {
                return Err(Error::new(format!(
                    "push mirror ref {entry}: a * must end both sides, once"
                )));
            }
            Ok(RefMap { from, to })
        })
        .collect()
}

/// The remote refs `maps` make of the local `refs`, each with the local id; a later map does not replace a ref an
/// earlier one made.
pub(crate) fn mapped(maps: &[RefMap], refs: &[(String, ObjectId)]) -> Vec<(String, ObjectId)> {
    let mut out: Vec<(String, ObjectId)> = Vec::new();
    for map in maps {
        for (name, id) in refs {
            let remote = match (map.from.strip_suffix('*'), map.to.strip_suffix('*')) {
                (Some(from), Some(to)) => name.strip_prefix(from).map(|rest| format!("{to}{rest}")),
                _ => (*name == map.from).then(|| map.to.clone()),
            };
            if let Some(remote) = remote {
                if !out.iter().any(|(made, _)| *made == remote) {
                    out.push((remote, *id));
                }
            }
        }
    }
    out
}

/// The server's refs and capabilities.
#[derive(Debug, Default)]
pub(crate) struct RemoteRefs {
    pub refs: HashMap<String, ObjectId>,
    pub caps: HashSet<String>,
}

fn parse_id(text: &str) -> Result<ObjectId, Error> {
    ObjectId::from_hex(text.trim().as_bytes()).map_err(|_| Error::new(format!("not an object id: {text}")))
}

/// Read a receive-pack advertisement, up to its flush. An empty repository advertises `capabilities^{}`.
pub(crate) fn read_advertisement(input: &mut dyn Read) -> Result<RemoteRefs, Error> {
    let mut remote = RemoteRefs::default();
    loop {
        match pktline::read(input).map_err(io_error)? {
            None => return Err(Error::new("the server closed the connection before it listed its refs")),
            Some(Packet::Flush) => return Ok(remote),
            Some(Packet::Data(data)) => {
                let (line, caps) = match data.iter().position(|byte| *byte == 0) {
                    Some(at) => (&data[..at], Some(&data[at + 1..])),
                    None => (&data[..], None),
                };
                if let Some(caps) = caps {
                    let caps = String::from_utf8_lossy(caps);
                    remote.caps.extend(caps.split_whitespace().map(str::to_string));
                }
                let line = String::from_utf8_lossy(line.strip_suffix(b"\n").unwrap_or(line)).into_owned();
                if let Some(message) = line.strip_prefix("ERR ") {
                    return Err(Error::new(format!("the server refused the push: {message}")));
                }
                // a shallow repository lists its shallow commits after its refs
                if line.starts_with("version ") || line.starts_with("shallow ") {
                    continue;
                }
                let Some((id, name)) = line.split_once(' ') else {
                    return Err(Error::new(format!("protocol error: expected a ref, got '{line}'")));
                };
                if name == "capabilities^{}" || name == ".have" {
                    continue;
                }
                remote.refs.insert(name.to_string(), parse_id(id)?);
            }
            Some(other) => return Err(Error::new(format!("protocol error: unexpected {other:?} in the refs"))),
        }
    }
}

/// What became of a push.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PushOutcome {
    /// The remote refs changed.
    pub pushed: Vec<String>,
    /// The remote refs the server refused, with its reason.
    pub refused: Vec<(String, String)>,
    /// The remote refs already where the local ones are.
    pub current: usize,
    pub objects: usize,
}

/// Push `refs` (remote name, local id) over one receive-pack conversation: `input` is what the server writes,
/// `output` what it reads. A ref already at its id is left out; when none is left nothing is sent but a flush.
pub(crate) fn push(
    store: &dyn GitStore,
    refs: &[(String, ObjectId)],
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> Result<PushOutcome, Error> {
    let remote = read_advertisement(input)?;
    let mut outcome = PushOutcome::default();
    let mut commands = Vec::new();
    for (name, id) in refs {
        let old = remote.refs.get(name).copied();
        if old == Some(*id) {
            outcome.current += 1;
        } else {
            commands.push((old, *id, name.clone()));
        }
    }
    if commands.is_empty() {
        pktline::flush(output).map_err(io_error)?;
        output.flush().map_err(io_error)?;
        return Ok(outcome);
    }
    let requested = requested(&remote)?;
    let zero = ObjectId::null(gix::hash::Kind::Sha1);
    for (index, (old, new, name)) in commands.iter().enumerate() {
        let mut line = format!("{} {new} {name}", old.unwrap_or(zero)).into_bytes();
        if index == 0 {
            line.push(0);
            line.extend_from_slice(requested.as_bytes());
        }
        line.push(b'\n');
        pktline::data(output, &line).map_err(io_error)?;
    }
    pktline::flush(output).map_err(io_error)?;
    let remote_ids: Vec<ObjectId> = remote.refs.values().copied().collect();
    let haves: Vec<ObjectId> = store.contains_many(&remote_ids)?.into_iter().collect();
    let wants: Vec<ObjectId> = commands.iter().map(|(_, new, _)| *new).collect();
    let planned = plan::plan(store, &wants, &haves, &[], &HashSet::new(), &[])?;
    outcome.objects = planned.objects.len();
    let deltas = pack_out::Deltas {
        ofs: remote.caps.contains("ofs-delta"),
        thin: None,
    };
    pack_out::write_pack(store, &planned.objects, deltas, super::pack_threads(), output)?;
    output.flush().map_err(io_error)?;
    read_report(input, &mut outcome)?;
    Ok(outcome)
}

/// The capabilities a push asks for, of those the server offers: `report-status` (required), `ofs-delta`, and, as git
/// does, the client's `agent` only when the server names its own.
pub(crate) fn requested(remote: &RemoteRefs) -> Result<String, Error> {
    if !remote.caps.contains("report-status") {
        return Err(Error::new("the server does not offer report-status"));
    }
    let mut requested = String::from("report-status");
    if remote.caps.contains("ofs-delta") {
        requested.push_str(" ofs-delta");
    }
    if remote.caps.iter().any(|cap| cap.starts_with("agent=")) {
        requested.push_str(&format!(" agent={}", super::agent()));
    }
    Ok(requested)
}

/// `unpack ok` (or why the server could not read the pack), then `ok <ref>` or `ng <ref> <reason>` each.
fn read_report(input: &mut dyn Read, outcome: &mut PushOutcome) -> Result<(), Error> {
    let first = pktline::read(input).map_err(io_error)?.and_then(|packet| packet.text());
    match first.as_deref() {
        Some("unpack ok") => {}
        Some(other) => {
            let reason = other.strip_prefix("unpack ").unwrap_or(other);
            return Err(Error::new(format!("the server could not unpack the push: {reason}")));
        }
        None => return Err(Error::new("the server closed the connection without a report")),
    }
    loop {
        match pktline::read(input).map_err(io_error)? {
            None | Some(Packet::Flush) => return Ok(()),
            Some(packet) => {
                let line = packet.text().unwrap_or_default();
                if let Some(name) = line.strip_prefix("ok ") {
                    outcome.pushed.push(name.to_string());
                } else if let Some(rest) = line.strip_prefix("ng ") {
                    let (name, reason) = rest.split_once(' ').unwrap_or((rest, ""));
                    outcome.refused.push((name.to_string(), reason.to_string()));
                }
            }
        }
    }
}

/// An SSH URL's parts: `ssh://[user@]host[:port]/path` or `[user@]host:path`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SshUrl {
    pub user: Option<String>,
    /// `host` or `host:port`.
    pub host: String,
    pub path: String,
}

pub(crate) fn parse_ssh_url(url: &str) -> Result<SshUrl, Error> {
    let url = url.trim();
    let bad = || {
        Error::new(format!(
            "push mirror URL {url} is not an SSH URL (ssh://git@host/owner/repo.git or git@host:owner/repo.git)"
        ))
    };
    let (authority, path) = if let Some(rest) = url.strip_prefix("ssh://") {
        let at = rest.find('/').ok_or_else(bad)?;
        (&rest[..at], rest[at..].to_string())
    } else if url.contains("://") {
        return Err(bad());
    } else {
        let (authority, path) = url.split_once(':').ok_or_else(bad)?;
        (authority, path.to_string())
    };
    let (user, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user.to_string()), host.to_string()),
        None => (None, authority.to_string()),
    };
    if host.is_empty() || path.trim_matches('/').is_empty() {
        return Err(bad());
    }
    Ok(SshUrl { user, host, path })
}

/// The command a push runs on the server, its path quoted as git quotes it.
pub(crate) fn receive_pack_command(path: &str) -> String {
    format!("git-receive-pack '{}'", path.replace('\'', "'\\''"))
}

/// End a push as git does: close the command's input and read to its end. The server runs its `post-receive` hooks
/// after the report, and a connection closed under them can cut them short. The exit status, when the server sent it.
pub(crate) fn finish(exec: &mut dyn moqui_core::sftp::SshExec) -> Result<Option<u32>, Error> {
    exec.finish_input()?;
    while !exec.read(64 * 1024)?.is_empty() {}
    Ok(exec.exit_status())
}

/// A command on an SSH server as the two ends of a conversation: `&ExecIo` reads its output and writes its input.
pub(crate) struct ExecIo<'a>(pub std::cell::RefCell<&'a mut dyn moqui_core::sftp::SshExec>);

impl Read for &ExecIo<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let data = self.0.borrow_mut().read(buf.len()).map_err(std::io::Error::other)?;
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}

impl Write for &ExecIo<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(buf).map_err(std::io::Error::other)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
