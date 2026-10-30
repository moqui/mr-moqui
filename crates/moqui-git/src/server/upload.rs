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

//! git-upload-pack, the server side of a fetch or clone, in protocol v0/v1 (`gitprotocol-pack`: the ref
//! advertisement, then wants, shallow lines and rounds of haves answered with `multi_ack_detailed` acks) and v2
//! (`gitprotocol-v2`: the `ls-refs` and `fetch` commands). Over SSH a connection is one conversation (stateful); over
//! HTTP each request carries a whole round and is answered alone (stateless RPC), so a v0 request repeats the wants and
//! all earlier haves, and each response ends at the round's `NAK`. The negotiation follows git's `upload-pack.c`.

use std::collections::HashSet;
use std::io::{Read, Write};

use gix::ObjectId;

use super::pktline::{self, PackWriter, Packet, MAX_BAND_64K, MAX_BAND_SMALL};
use super::plan::{self, Deepen};
use super::{io_error, Advertised};
use crate::ops;
use crate::revwalk::peel_to_commit;
use crate::store::GitStore;
use moqui_core::context::Error;

/// The v0/v1 capabilities, after the first advertised ref.
fn v0_capabilities(advertised: &Advertised) -> String {
    let mut caps = String::from(
        "multi_ack_detailed no-done side-band side-band-64k ofs-delta thin-pack shallow deepen-since deepen-not \
         deepen-relative no-progress include-tag allow-tip-sha1-in-want allow-reachable-sha1-in-want",
    );
    caps.push_str(&format!(" symref=HEAD:{}", advertised.branch_ref));
    caps.push_str(" object-format=sha1 agent=");
    caps.push_str(&super::agent());
    caps
}

/// The v0/v1 ref advertisement: HEAD (when its branch has commits) and every ref, each annotated tag followed by
/// what it peels to; an empty repository advertises only its capabilities.
pub(crate) fn advertise_v0(out: &mut dyn Write, advertised: &Advertised) -> Result<(), Error> {
    let caps = v0_capabilities(advertised);
    let mut first = true;
    let mut line = |out: &mut dyn Write, id: &ObjectId, name: &str| -> std::io::Result<()> {
        if first {
            first = false;
            let mut bytes = format!("{id} {name}").into_bytes();
            bytes.push(0);
            bytes.extend_from_slice(caps.as_bytes());
            bytes.push(b'\n');
            pktline::data(out, &bytes)
        } else {
            pktline::text(out, &format!("{id} {name}"))
        }
    };
    if let Some(head) = &advertised.head {
        line(out, head, "HEAD").map_err(io_error)?;
    }
    for reference in &advertised.refs {
        line(out, &reference.id, &reference.name).map_err(io_error)?;
        if let Some(peeled) = &reference.peeled {
            pktline::text(out, &format!("{peeled} {}^{{}}", reference.name)).map_err(io_error)?;
        }
    }
    if first {
        let mut bytes = format!("{} capabilities^{{}}", ObjectId::null(gix::hash::Kind::Sha1)).into_bytes();
        bytes.push(0);
        bytes.extend_from_slice(caps.as_bytes());
        bytes.push(b'\n');
        pktline::data(out, &bytes).map_err(io_error)?;
    }
    pktline::flush(out).map_err(io_error)
}

/// The v2 capability advertisement.
pub(crate) fn advertise_v2(out: &mut dyn Write) -> Result<(), Error> {
    for line in [
        "version 2".to_string(),
        format!("agent={}", super::agent()),
        "ls-refs=unborn".to_string(),
        "fetch=shallow".to_string(),
        "object-format=sha1".to_string(),
    ] {
        pktline::text(out, &line).map_err(io_error)?;
    }
    pktline::flush(out).map_err(io_error)
}

fn parse_id(text: &str) -> Result<ObjectId, Error> {
    ObjectId::from_hex(text.trim().as_bytes()).map_err(|_| Error::new(format!("not an object id: {text}")))
}

/// What a fetch asks for, from either protocol version.
#[derive(Debug, Default)]
struct Request {
    wants: Vec<ObjectId>,
    caps: HashSet<String>,
    deepen: Deepen,
    /// deepen-not refs as named, resolved later.
    deepen_not_refs: Vec<String>,
}

impl Request {
    fn has(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }

    /// The common argument lines of both versions; false for a line that is not one.
    fn argument(&mut self, line: &str) -> Result<bool, Error> {
        if let Some(rest) = line.strip_prefix("want ") {
            let mut parts = rest.split(' ');
            self.wants.push(parse_id(parts.next().unwrap_or_default())?);
            self.caps
                .extend(parts.filter(|part| !part.is_empty()).map(str::to_string));
        } else if let Some(rest) = line.strip_prefix("shallow ") {
            self.deepen.client_shallow.push(parse_id(rest)?);
        } else if let Some(rest) = line.strip_prefix("deepen ") {
            let depth: u32 = rest
                .trim()
                .parse()
                .map_err(|_| Error::new(format!("bad deepen {rest}")))?;
            if depth == 0 {
                return Err(Error::new("deepen 0 is not a depth"));
            }
            self.deepen.depth = Some(depth);
        } else if line == "deepen-relative" {
            self.deepen.relative = true;
        } else if let Some(rest) = line.strip_prefix("deepen-since ") {
            self.deepen.since = Some(
                rest.trim()
                    .parse()
                    .map_err(|_| Error::new(format!("bad deepen-since {rest}")))?,
            );
        } else if let Some(rest) = line.strip_prefix("deepen-not ") {
            self.deepen_not_refs.push(rest.trim().to_string());
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Resolve `deepen-not` refs, check that the wants are objects here, and that depth is not combined with the
    /// others, as git refuses.
    fn check(&mut self, store: &dyn GitStore, advertised: &Advertised) -> Result<(), Error> {
        // v0 sends deepen-relative as a capability of the first want, v2 as an argument
        if self.has("deepen-relative") {
            self.deepen.relative = true;
        }
        if self.deepen.depth.is_some() && (self.deepen.since.is_some() || !self.deepen_not_refs.is_empty()) {
            return Err(Error::new(
                "--depth and --shallow-since (or --shallow-exclude) cannot be used together",
            ));
        }
        for name in &self.deepen_not_refs {
            let found = advertised
                .refs
                .iter()
                .find(|reference| reference.name == *name || super::short_ref(&reference.name) == name)
                .ok_or_else(|| Error::new(format!("git upload-pack: ambiguous deepen-not: {name}")))?;
            self.deepen.not.push(found.id);
        }
        for want in &self.wants {
            if store.object_size(want)?.is_none() {
                return Err(Error::new(format!("upload-pack: not our ref {want}")));
            }
        }
        Ok(())
    }
}

/// Which of the client's haves are common, and when the wants are all reachable from them.
struct Negotiation<'a> {
    store: &'a dyn GitStore,
    wants: Vec<ObjectId>,
    /// Common commits, in the order acknowledged.
    haves: Vec<ObjectId>,
    have_set: HashSet<ObjectId>,
    oldest_have: Option<i64>,
    /// Wants already known to reach a common commit.
    satisfied: HashSet<ObjectId>,
}

impl<'a> Negotiation<'a> {
    fn new(store: &'a dyn GitStore, wants: &[ObjectId]) -> Self {
        Self {
            store,
            wants: wants.to_vec(),
            haves: Vec::new(),
            have_set: HashSet::new(),
            oldest_have: None,
            satisfied: HashSet::new(),
        }
    }

    /// A `have`: true when this repository has the commit (it is common).
    fn have(&mut self, id: ObjectId) -> Result<bool, Error> {
        let Some(commit) = peel_to_commit(self.store, &id)? else {
            return Ok(false);
        };
        if self.have_set.insert(commit) {
            self.haves.push(commit);
            let seconds = ops::read_commit(self.store, &commit)?.seconds;
            self.oldest_have = Some(self.oldest_have.map_or(seconds, |oldest| oldest.min(seconds)));
        }
        Ok(true)
    }

    /// Whether every want reaches a common commit, looking no further back than the oldest one (git's
    /// `ok_to_give_up`).
    fn ok_to_give_up(&mut self) -> Result<bool, Error> {
        let Some(oldest) = self.oldest_have else {
            return Ok(false);
        };
        for want in self.wants.clone() {
            if self.satisfied.contains(&want) {
                continue;
            }
            let Some(commit) = peel_to_commit(self.store, &want)? else {
                // a tree or blob wanted directly has no history to negotiate
                self.satisfied.insert(want);
                continue;
            };
            let mut pending = vec![commit];
            let mut seen = HashSet::new();
            let mut reached = false;
            while let Some(id) = pending.pop() {
                if !seen.insert(id) {
                    continue;
                }
                if self.have_set.contains(&id) {
                    reached = true;
                    break;
                }
                let info = ops::read_commit(self.store, &id)?;
                if info.seconds < oldest {
                    continue;
                }
                pending.extend(info.parents);
            }
            if !reached {
                return Ok(false);
            }
            self.satisfied.insert(want);
        }
        Ok(true)
    }
}

/// The pack for a negotiated fetch: shallow lines first when `shallow_out` takes them, then the data.
struct Send<'a> {
    store: &'a dyn GitStore,
    advertised: &'a Advertised,
    threads: Option<usize>,
}

impl Send<'_> {
    /// Work out the shallow boundary of `request`, adding wants for unshallowed commits. The response's shallow
    /// lines and the grafts for the walk.
    fn shallow(&self, request: &mut Request) -> Result<(plan::ShallowInfo, HashSet<ObjectId>), Error> {
        let info = if request.deepen.deepens() {
            plan::shallow(self.store, &mut request.wants, &request.deepen)?
        } else {
            plan::ShallowInfo::default()
        };
        let grafts: HashSet<ObjectId> = request
            .deepen
            .client_shallow
            .iter()
            .chain(&info.shallow)
            .copied()
            .collect();
        Ok((info, grafts))
    }

    /// The pack, on the side-band when there is one; a failure there goes on band 3, the error band, which the
    /// client shows and stops at.
    fn pack(
        &self,
        request: &Request,
        haves: &[ObjectId],
        info: &plan::ShallowInfo,
        grafts: &HashSet<ObjectId>,
        out: &mut dyn Write,
        band: Option<usize>,
    ) -> Result<(), Error> {
        let Err(err) = self.pack_data(request, haves, info, grafts, out, band) else {
            return Ok(());
        };
        let Some(max) = band else {
            return Err(err);
        };
        tracing::warn!("git upload-pack failed while sending the pack: {err}");
        let text = format!("error: {}\n", err.message());
        pktline::band(out, pktline::BAND_ERROR, text.as_bytes(), max).map_err(io_error)?;
        pktline::flush(out).map_err(io_error)?;
        out.flush().map_err(io_error)
    }

    fn pack_data(
        &self,
        request: &Request,
        haves: &[ObjectId],
        info: &plan::ShallowInfo,
        grafts: &HashSet<ObjectId>,
        out: &mut dyn Write,
        band: Option<usize>,
    ) -> Result<(), Error> {
        let tags: Vec<ObjectId> = if request.has("include-tag") {
            self.advertised
                .refs
                .iter()
                .filter(|reference| reference.peeled.is_some())
                .map(|reference| reference.id)
                .collect()
        } else {
            Vec::new()
        };
        let planned = plan::plan(self.store, &request.wants, haves, &info.unshallow, grafts, &tags)?;
        let missing = super::pack_out::missing_objects(self.store, &planned.objects)?;
        if let Some(id) = missing.first() {
            return Err(Error::new(format!(
                "the repository is missing object {id} ({} in all), which the fetch needs",
                missing.len()
            )));
        }
        let mut writer = PackWriter::new(out, band);
        if !request.has("no-progress") {
            writer
                .progress(&format!(
                    "Sending {} objects ({} commits)\n",
                    planned.objects.len(),
                    planned.commits
                ))
                .map_err(io_error)?;
        }
        let deltas = super::pack_out::Deltas {
            ofs: request.has("ofs-delta"),
            thin: request.has("thin-pack").then_some(&planned.excluded),
        };
        super::pack_out::write_pack(self.store, &planned.objects, deltas, self.threads, &mut writer)?;
        writer.flush().map_err(io_error)?;
        drop(writer);
        if band.is_some() {
            pktline::flush(out).map_err(io_error)?;
        }
        out.flush().map_err(io_error)
    }
}

/// The side-band packet size a v0 client chose; `None` without a side-band.
fn band_of(request: &Request) -> Option<usize> {
    if request.has("side-band-64k") {
        Some(MAX_BAND_64K)
    } else if request.has("side-band") {
        Some(MAX_BAND_SMALL)
    } else {
        None
    }
}

/// How the client asked to be acknowledged.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MultiAck {
    None,
    Plain,
    Detailed,
}

fn read_line(input: &mut dyn Read) -> Result<Option<Packet>, Error> {
    pktline::read(input).map_err(io_error)
}

/// A v0/v1 upload-pack after the advertisement: wants, shallow lines, negotiation, then the pack. `stateless` for HTTP,
/// where each request is a whole round and is answered alone.
pub(crate) fn upload_v0(
    store: &dyn GitStore,
    advertised: &Advertised,
    input: &mut dyn Read,
    out: &mut dyn Write,
    stateless: bool,
    threads: Option<usize>,
) -> Result<(), Error> {
    let mut request = Request::default();
    loop {
        match read_line(input)? {
            None | Some(Packet::Flush) => break,
            Some(packet) => {
                let line = packet.text().unwrap_or_default();
                if !request.argument(&line)? {
                    return Err(Error::new(format!(
                        "git upload-pack: protocol error, expected a want, got '{line}'"
                    )));
                }
            }
        }
    }
    if request.wants.is_empty() {
        // ls-remote, or a fetch that is already up to date
        return Ok(());
    }
    request.check(store, advertised)?;
    let sender = Send {
        store,
        advertised,
        threads,
    };
    let (info, grafts) = sender.shallow(&mut request)?;
    if request.deepen.deepens() {
        for id in &info.shallow {
            pktline::text(out, &format!("shallow {id}")).map_err(io_error)?;
        }
        for id in &info.unshallow {
            pktline::text(out, &format!("unshallow {id}")).map_err(io_error)?;
        }
        pktline::flush(out).map_err(io_error)?;
        out.flush().map_err(io_error)?;
    }
    let multi_ack = if request.has("multi_ack_detailed") {
        MultiAck::Detailed
    } else if request.has("multi_ack") {
        MultiAck::Plain
    } else {
        MultiAck::None
    };
    let no_done = request.has("no-done") && multi_ack == MultiAck::Detailed;
    let mut negotiation = Negotiation::new(store, &request.wants);
    let (mut got_common, mut got_other, mut sent_ready) = (false, false, false);
    let mut last: Option<ObjectId> = None;
    loop {
        match read_line(input)? {
            // the end of a stateless request that only asked for the shallow list, or a client that went away: git's
            // upload-pack writes nothing more
            None => return Ok(()),
            Some(Packet::Flush) => {
                if multi_ack == MultiAck::Detailed && got_common && !got_other && negotiation.ok_to_give_up()? {
                    sent_ready = true;
                    if let Some(last) = last {
                        pktline::text(out, &format!("ACK {last} ready")).map_err(io_error)?;
                    }
                }
                if negotiation.haves.is_empty() || multi_ack != MultiAck::None {
                    pktline::text(out, "NAK").map_err(io_error)?;
                }
                if no_done && sent_ready {
                    if let Some(last) = last {
                        pktline::text(out, &format!("ACK {last}")).map_err(io_error)?;
                    }
                    break;
                }
                out.flush().map_err(io_error)?;
                if stateless {
                    return Ok(());
                }
                got_common = false;
                got_other = false;
            }
            Some(packet) => {
                let line = packet.text().unwrap_or_default();
                if let Some(rest) = line.strip_prefix("have ") {
                    let id = parse_id(rest)?;
                    if negotiation.have(id)? {
                        got_common = true;
                        last = Some(id);
                        match multi_ack {
                            MultiAck::Detailed => pktline::text(out, &format!("ACK {id} common")),
                            MultiAck::Plain => pktline::text(out, &format!("ACK {id} continue")),
                            MultiAck::None if negotiation.haves.len() == 1 => pktline::text(out, &format!("ACK {id}")),
                            MultiAck::None => Ok(()),
                        }
                        .map_err(io_error)?;
                    } else {
                        got_other = true;
                        if multi_ack != MultiAck::None && negotiation.ok_to_give_up()? {
                            if multi_ack == MultiAck::Detailed {
                                sent_ready = true;
                                pktline::text(out, &format!("ACK {id} ready")).map_err(io_error)?;
                            } else {
                                pktline::text(out, &format!("ACK {id} continue")).map_err(io_error)?;
                            }
                        }
                    }
                } else if line == "done" {
                    match (negotiation.haves.is_empty(), last) {
                        (false, Some(last)) if multi_ack != MultiAck::None => {
                            pktline::text(out, &format!("ACK {last}")).map_err(io_error)?;
                        }
                        (false, _) => {}
                        (true, _) => pktline::text(out, "NAK").map_err(io_error)?,
                    }
                    break;
                } else {
                    return Err(Error::new(format!("git upload-pack: expected SHA1 list, got '{line}'")));
                }
            }
        }
    }
    sender.pack(&request, &negotiation.haves, &info, &grafts, out, band_of(&request))
}

/// One v2 command request: the command, its capabilities, its arguments. `None` at the end of the input or a lone
/// flush (the client ending the conversation).
fn read_command(input: &mut dyn Read) -> Result<Option<(String, Vec<String>)>, Error> {
    let mut command = None;
    let mut arguments = Vec::new();
    let mut in_arguments = false;
    loop {
        match read_line(input)? {
            None => return Ok(None),
            Some(Packet::Flush) if command.is_none() => return Ok(None),
            Some(Packet::Flush) => break,
            Some(Packet::Delim) => in_arguments = true,
            Some(Packet::ResponseEnd) => return Err(Error::new("a response end in a request")),
            Some(packet) => {
                let line = packet.text().unwrap_or_default();
                if in_arguments {
                    arguments.push(line);
                } else if let Some(name) = line.strip_prefix("command=") {
                    command = Some(name.to_string());
                }
                // the other capability lines (agent, object-format) need no answer
            }
        }
    }
    Ok(command.map(|command| (command, arguments)))
}

/// A protocol v2 conversation after the capability advertisement: each command answered in turn (one per request
/// over HTTP).
pub(crate) fn serve_v2(
    store: &dyn GitStore,
    advertised: &Advertised,
    input: &mut dyn Read,
    out: &mut dyn Write,
    stateless: bool,
    threads: Option<usize>,
) -> Result<(), Error> {
    while let Some((command, arguments)) = read_command(input)? {
        match command.as_str() {
            "ls-refs" => ls_refs(advertised, &arguments, out)?,
            "fetch" => fetch_v2(store, advertised, &arguments, out, threads)?,
            other => return Err(Error::new(format!("unknown command '{other}'"))),
        }
        out.flush().map_err(io_error)?;
        if stateless {
            break;
        }
    }
    Ok(())
}

fn ls_refs(advertised: &Advertised, arguments: &[String], out: &mut dyn Write) -> Result<(), Error> {
    let symrefs = arguments.iter().any(|argument| argument == "symrefs");
    let peel = arguments.iter().any(|argument| argument == "peel");
    let unborn = arguments.iter().any(|argument| argument == "unborn");
    let prefixes: Vec<&str> = arguments
        .iter()
        .filter_map(|argument| argument.strip_prefix("ref-prefix "))
        .collect();
    let wanted = |name: &str| prefixes.is_empty() || prefixes.iter().any(|prefix| name.starts_with(prefix));
    if wanted("HEAD") {
        let target = if symrefs {
            format!(" symref-target:{}", advertised.branch_ref)
        } else {
            String::new()
        };
        match &advertised.head {
            Some(head) => pktline::text(out, &format!("{head} HEAD{target}")).map_err(io_error)?,
            None if unborn => {
                pktline::text(out, &format!("unborn HEAD symref-target:{}", advertised.branch_ref)).map_err(io_error)?
            }
            None => {}
        }
    }
    for reference in advertised.refs.iter().filter(|reference| wanted(&reference.name)) {
        let mut line = format!("{} {}", reference.id, reference.name);
        if let (true, Some(peeled)) = (peel, &reference.peeled) {
            line.push_str(&format!(" peeled:{peeled}"));
        }
        pktline::text(out, &line).map_err(io_error)?;
    }
    pktline::flush(out).map_err(io_error)
}

fn fetch_v2(
    store: &dyn GitStore,
    advertised: &Advertised,
    arguments: &[String],
    out: &mut dyn Write,
    threads: Option<usize>,
) -> Result<(), Error> {
    let mut request = Request::default();
    let mut haves = Vec::new();
    let mut done = false;
    for argument in arguments {
        if request.argument(argument)? {
            continue;
        }
        if let Some(rest) = argument.strip_prefix("have ") {
            haves.push(parse_id(rest)?);
        } else if argument == "done" {
            done = true;
        } else {
            // thin-pack, no-progress, include-tag, ofs-delta, and what this server does not offer
            request.caps.insert(argument.clone());
        }
    }
    request.check(store, advertised)?;
    let mut negotiation = Negotiation::new(store, &request.wants);
    let mut acks = Vec::new();
    for have in haves {
        if negotiation.have(have)? {
            acks.push(have);
        }
    }
    if !done {
        pktline::text(out, "acknowledgments").map_err(io_error)?;
        if acks.is_empty() {
            pktline::text(out, "NAK").map_err(io_error)?;
        }
        for id in &acks {
            pktline::text(out, &format!("ACK {id}")).map_err(io_error)?;
        }
        if !negotiation.ok_to_give_up()? {
            return pktline::flush(out).map_err(io_error);
        }
        pktline::text(out, "ready").map_err(io_error)?;
        pktline::delim(out).map_err(io_error)?;
    }
    let sender = Send {
        store,
        advertised,
        threads,
    };
    let (info, grafts) = sender.shallow(&mut request)?;
    if request.deepen.deepens() || !request.deepen.client_shallow.is_empty() {
        pktline::text(out, "shallow-info").map_err(io_error)?;
        for id in &info.shallow {
            pktline::text(out, &format!("shallow {id}")).map_err(io_error)?;
        }
        for id in &info.unshallow {
            pktline::text(out, &format!("unshallow {id}")).map_err(io_error)?;
        }
        pktline::delim(out).map_err(io_error)?;
    }
    pktline::text(out, "packfile").map_err(io_error)?;
    sender.pack(&request, &negotiation.haves, &info, &grafts, out, Some(MAX_BAND_64K))
}
