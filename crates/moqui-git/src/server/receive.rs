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

//! The wire format of git-receive-pack, the server side of a push (protocol v0/v1 only, as in git: v2 has no push):
//! the ref advertisement, the client's commands (old id, new id, ref), its push options, and the report of what
//! became of each ref (`report-status`), wrapped in side-band 1 when the client asked for `side-band-64k`. What a
//! push does is in `server::mod` (`GitProvider::push`).

use std::collections::HashSet;
use std::io::{Read, Write};

use gix::ObjectId;

use super::pktline::{self, Packet, MAX_BAND_64K};
use super::{io_error, Advertised};
use moqui_core::context::Error;

fn capabilities() -> String {
    format!(
        "report-status delete-refs side-band-64k quiet atomic ofs-delta push-options object-format=sha1 agent={}",
        super::agent()
    )
}

/// The advertisement: every ref (not HEAD), the capabilities after the first; an empty repository advertises only
/// its capabilities.
pub(crate) fn advertise(out: &mut dyn Write, advertised: &Advertised) -> Result<(), Error> {
    let caps = capabilities();
    let with_caps = |id: &ObjectId, name: &str| {
        let mut bytes = format!("{id} {name}").into_bytes();
        bytes.push(0);
        bytes.extend_from_slice(caps.as_bytes());
        bytes.push(b'\n');
        bytes
    };
    match advertised.refs.split_first() {
        Some((first, rest)) => {
            pktline::data(out, &with_caps(&first.id, &first.name)).map_err(io_error)?;
            for reference in rest {
                pktline::text(out, &format!("{} {}", reference.id, reference.name)).map_err(io_error)?;
            }
        }
        None => pktline::data(
            out,
            &with_caps(&ObjectId::null(gix::hash::Kind::Sha1), "capabilities^{}"),
        )
        .map_err(io_error)?,
    }
    pktline::flush(out).map_err(io_error)
}

/// One ref a push changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Command {
    /// What the client saw: `None` to create the ref.
    pub old: Option<ObjectId>,
    /// `None` to delete the ref.
    pub new: Option<ObjectId>,
    pub name: String,
}

/// A push's commands and what came with them.
#[derive(Clone, Debug, Default)]
pub(crate) struct PushRequest {
    pub commands: Vec<Command>,
    pub caps: HashSet<String>,
    /// The client's shallow commits, when it is a shallow clone.
    pub shallow: Vec<ObjectId>,
    pub options: Vec<String>,
}

impl PushRequest {
    pub(crate) fn has(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }
}

fn id_or_none(text: &str) -> Result<Option<ObjectId>, Error> {
    let id = ObjectId::from_hex(text.as_bytes()).map_err(|_| Error::new(format!("not an object id: {text}")))?;
    Ok((!id.is_null()).then_some(id))
}

/// Read the commands and push options; `None` when the client sent none (nothing to push, or a probe).
pub(crate) fn read_request(input: &mut dyn Read) -> Result<Option<PushRequest>, Error> {
    let mut request = PushRequest::default();
    loop {
        match pktline::read(input).map_err(io_error)? {
            None | Some(Packet::Flush) => break,
            Some(Packet::Data(data)) => {
                let (line, caps) = match data.iter().position(|byte| *byte == 0) {
                    Some(at) => (&data[..at], Some(&data[at + 1..])),
                    None => (&data[..], None),
                };
                if let Some(caps) = caps {
                    let caps = String::from_utf8_lossy(caps);
                    request.caps.extend(caps.split_whitespace().map(str::to_string));
                }
                let line = String::from_utf8_lossy(line.strip_suffix(b"\n").unwrap_or(line)).into_owned();
                if let Some(rest) = line.strip_prefix("shallow ") {
                    request.shallow.push(
                        ObjectId::from_hex(rest.trim().as_bytes())
                            .map_err(|_| Error::new(format!("not an object id: {rest}")))?,
                    );
                    continue;
                }
                let mut parts = line.splitn(3, ' ');
                let (Some(old), Some(new), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
                    return Err(Error::new(format!(
                        "protocol error: expected old/new/ref, got '{line}'"
                    )));
                };
                request.commands.push(Command {
                    old: id_or_none(old)?,
                    new: id_or_none(new)?,
                    name: name.to_string(),
                });
            }
            Some(other) => return Err(Error::new(format!("protocol error: unexpected {other:?} in a push"))),
        }
    }
    if request.commands.is_empty() {
        return Ok(None);
    }
    if request.has("push-options") {
        loop {
            match pktline::read(input).map_err(io_error)? {
                None | Some(Packet::Flush) => break,
                Some(packet) => request.options.push(packet.text().unwrap_or_default()),
            }
        }
    }
    Ok(Some(request))
}

/// Report what became of each command: `unpack ok` (or the error reading the pack), then `ok <ref>` or
/// `ng <ref> <reason>` each. Nothing for a client that did not ask for `report-status`.
pub(crate) fn report(
    out: &mut dyn Write,
    request: &PushRequest,
    unpack: Result<(), String>,
    results: &[Result<(), String>],
) -> Result<(), Error> {
    if !request.has("report-status") {
        if request.has("side-band-64k") {
            pktline::flush(out).map_err(io_error)?;
        }
        return out.flush().map_err(io_error);
    }
    let mut block = Vec::new();
    let one_line = |text: &str| text.replace(['\n', '\r'], " ");
    match &unpack {
        Ok(()) => pktline::text(&mut block, "unpack ok"),
        Err(err) => pktline::text(&mut block, &format!("unpack {}", one_line(err))),
    }
    .map_err(io_error)?;
    for (command, result) in request.commands.iter().zip(results) {
        match result {
            Ok(()) => pktline::text(&mut block, &format!("ok {}", command.name)),
            Err(reason) => pktline::text(&mut block, &format!("ng {} {}", command.name, one_line(reason))),
        }
        .map_err(io_error)?;
    }
    pktline::flush(&mut block).map_err(io_error)?;
    if request.has("side-band-64k") {
        pktline::band(out, pktline::BAND_DATA, &block, MAX_BAND_64K).map_err(io_error)?;
        pktline::flush(out).map_err(io_error)?;
    } else {
        out.write_all(&block).map_err(io_error)?;
    }
    out.flush().map_err(io_error)
}

/// A message for the client, shown as `remote: ...`: on band 2 when there is a side-band. As git sends its hooks'
/// output, `quiet` (which only stops progress) does not stop these.
pub(crate) fn message(out: &mut dyn Write, request: &PushRequest, text: &str) -> Result<(), Error> {
    if !request.has("side-band-64k") {
        return Ok(());
    }
    let mut text = text.trim_end().to_string();
    text.push('\n');
    pktline::band(out, pktline::BAND_PROGRESS, text.as_bytes(), MAX_BAND_64K).map_err(io_error)?;
    out.flush().map_err(io_error)
}
