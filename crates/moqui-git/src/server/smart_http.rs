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

//! The client of git's smart HTTP protocol (protocol v0, stateless RPC) on `moqui_lib`'s HTTP client: a push to
//! another server (`GET info/refs?service=git-receive-pack`, then `POST git-receive-pack` with the commands and one
//! pack, and its `report-status`), for push mirrors, and a fetch of named refs (`git-upload-pack`: wants, haves, then
//! one pack with no side band), for `fetch#GitRefs` (docs/interfaces/Git-Hosting.md, "Push mirrors" and "Fetching
//! refs"). The credential goes in a basic `Authorization` header, never in a URL; every message names the URL without
//! user, password or query, and has the credential taken out of whatever the transport said. The outbound address
//! rules are the webhooks': no loopback, private or link-local address (and no plain http) unless `git-server`
//! `webhook-private-network` is true. No redirect is followed, so the credential goes to the host named only.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use gix::ObjectId;
use moqui_core::context::Error;
use moqui_lib::http::{Method, OpenRequest, OpenResponse, RestClient};

use super::push::{self, PushOutcome, RefMap, RemoteRefs};
use super::{io_error, pack_in, pktline, pktline::Packet};
use crate::store::GitStore;

/// The largest refs advertisement read: a repository with tens of thousands of refs is well within it.
const MAX_ADVERTISEMENT: u64 = 32 * 1024 * 1024;
/// The largest push report read.
const MAX_REPORT: u64 = 1024 * 1024;
/// The largest pack a fetch reads (each object is also held to `git-server` `max-object-size`).
const MAX_FETCH_PACK: u64 = 8 * 1024 * 1024 * 1024;
/// How long reading the refs may take.
const ADVERTISEMENT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a whole push or fetch may take, its pack included.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A remote repository over HTTP(S): its URL (`https://host/owner/repo.git`, no user or password in it) and the
/// credential sent with each request.
pub(crate) struct Remote<'a> {
    pub url: &'a str,
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    /// `git-server` `webhook-private-network`: internal addresses and plain http are allowed.
    pub private_network: bool,
}

/// Why `url` may not be a remote: not `https://` (or `http://` with private networks allowed), with a user or password
/// in it, or with no host.
pub(crate) fn check_url(url: &str, private_network: bool) -> Result<(), String> {
    let url = url.trim();
    let rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        if !private_network {
            return Err("an http URL is allowed only with git-server webhook-private-network; use https".into());
        }
        rest
    } else {
        return Err("not an https URL".into());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err("the URL has a user or password in it; give the credential apart".into());
    }
    if authority.is_empty() || url.chars().any(char::is_whitespace) {
        return Err("the URL has no host".into());
    }
    Ok(())
}

impl Remote<'_> {
    fn client(&self, timeout: Duration) -> RestClient {
        RestClient::new()
            .deny_internal_addresses(!self.private_network)
            .timeout(timeout)
            .redirects(0, false)
    }

    /// The URL as messages show it.
    pub(crate) fn shown(&self) -> String {
        moqui_lib::http::redacted(self.url)
    }

    /// `text` with the credential (and its basic header value) taken out.
    pub(crate) fn redact(&self, text: &str) -> String {
        redact(text, self.password)
    }

    fn endpoint(&self, tail: &str) -> String {
        format!("{}/{tail}", self.url.trim().trim_end_matches('/'))
    }

    fn headers(&self, service: &str, post: bool) -> Vec<(String, String)> {
        let mut headers = vec![
            ("User-Agent".to_string(), super::agent()),
            (
                "Accept".to_string(),
                format!(
                    "application/x-{service}-{}",
                    if post { "result" } else { "advertisement" }
                ),
            ),
        ];
        if post {
            headers.push(("Content-Type".to_string(), format!("application/x-{service}-request")));
        }
        if let Some(password) = self.password.filter(|password| !password.is_empty()) {
            let user = self.username.filter(|user| !user.is_empty()).unwrap_or("git");
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
            headers.push(("Authorization".to_string(), format!("Basic {token}")));
        }
        headers
    }

    fn failed(&self, what: &str, err: &moqui_lib::error::Error) -> Error {
        // the transport's own text with its cause, which never holds the header, and is redacted all the same
        Error::new(format!("{what} {}: {}", self.shown(), self.redact(&err.to_string())))
    }

    fn status(&self, what: &str, response: &OpenResponse) -> Result<(), Error> {
        match response.status {
            200 => Ok(()),
            401 | 403 => Err(Error::new(format!(
                "{what} {}: the server refused the credential (HTTP {})",
                self.shown(),
                response.status
            ))),
            404 => Err(Error::new(format!(
                "{what} {}: no repository there (HTTP 404)",
                self.shown()
            ))),
            status => Err(Error::new(format!("{what} {}: HTTP {status}", self.shown()))),
        }
    }
}

/// `text` with `secret`, and any basic authorization value (`Basic <base64>`, which carries it encoded), replaced by
/// `***`.
pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    let text = match secret.map(str::trim).filter(|secret| secret.len() >= 3) {
        Some(secret) => text.replace(secret, "***"),
        None => text.to_string(),
    };
    redact_basic(&text)
}

/// `text` with the token after each `Basic ` (any case) replaced by `***`.
fn redact_basic(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find("basic ") {
        let start = at + found + "basic ".len();
        let token = text[start..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_'))
            .count();
        out.push_str(&text[at..start]);
        if token > 0 {
            out.push_str("***");
        }
        at = start + token;
    }
    out.push_str(&text[at..]);
    out
}

/// The server's refs and capabilities for `service` (`git-receive-pack` or `git-upload-pack`).
pub(crate) fn advertisement(remote: &Remote<'_>, service: &str) -> Result<RemoteRefs, Error> {
    check_url(remote.url, remote.private_network).map_err(|why| Error::new(format!("{}: {why}", remote.shown())))?;
    let what = "reading the refs of";
    let response = remote
        .client(ADVERTISEMENT_TIMEOUT)
        .open(OpenRequest {
            method: Method::Get,
            url: remote.endpoint(&format!("info/refs?service={service}")),
            headers: remote.headers(service, false),
            body: None,
            timeout: None,
        })
        .map_err(|err| remote.failed(what, &err))?;
    remote.status(what, &response)?;
    let mut limited = response.reader.take(MAX_ADVERTISEMENT);
    let mut reader = BufReader::new(&mut limited);
    let first = pktline::read(&mut reader).map_err(io_error)?;
    let expected = format!("# service={service}");
    match first.and_then(|packet| packet.text()) {
        Some(line) if line == expected => {}
        _ => {
            return Err(Error::new(format!(
                "{what} {}: the server does not speak git's smart HTTP",
                remote.shown()
            )))
        }
    }
    // the flush after the service line (some servers leave it out)
    if reader.fill_buf().map_err(io_error)?.starts_with(b"0000") {
        pktline::read(&mut reader).map_err(io_error)?;
    }
    let refs = push::read_advertisement(&mut reader);
    drop(reader);
    if limited.limit() == 0 {
        return Err(Error::new(format!(
            "{what} {}: the refs are more than {MAX_ADVERTISEMENT} bytes",
            remote.shown()
        )));
    }
    refs.map_err(|err| Error::new(format!("{what} {}: {}", remote.shown(), remote.redact(err.message()))))
}

/// Push `refs` (remote name, local id), each forced to the local id, and with `delete_covered` delete the refs of the
/// server those maps cover that `refs` does not name. The request (commands and pack) is written to a file in
/// `scratch` first and sent from it, so a large pack is not held in memory.
pub(crate) fn push(
    store: &dyn GitStore,
    remote: &Remote<'_>,
    refs: &[(String, ObjectId)],
    delete_covered: Option<&[RefMap]>,
    scratch: &Path,
) -> Result<PushOutcome, Error> {
    let advertised = advertisement(remote, "git-receive-pack")?;
    let mut outcome = PushOutcome::default();
    let commands = push::commands(&advertised, refs, delete_covered, &mut outcome);
    if commands.is_empty() {
        return Ok(outcome);
    }
    std::fs::create_dir_all(scratch)
        .map_err(|err| Error::new(format!("cannot create {}: {err}", scratch.display())))?;
    let result = (|| {
        let path = scratch.join("receive-pack.request");
        let file = std::fs::File::create(&path)
            .map_err(|err| Error::new(format!("cannot create {}: {err}", path.display())))?;
        let mut output = std::io::BufWriter::with_capacity(64 * 1024, file);
        outcome.objects = push::write_request(store, &advertised, &commands, &mut output)?;
        output.flush().map_err(io_error)?;
        drop(output);
        let length = std::fs::metadata(&path).map_err(io_error)?.len();
        let body = std::fs::File::open(&path).map_err(io_error)?;
        let what = "pushing to";
        let response = remote
            .client(TRANSFER_TIMEOUT)
            .upload(
                OpenRequest {
                    method: Method::Post,
                    url: remote.endpoint("git-receive-pack"),
                    headers: remote.headers("git-receive-pack", true),
                    body: None,
                    timeout: None,
                },
                Box::new(body),
                Some(length),
            )
            .map_err(|err| remote.failed(what, &err))?;
        remote.status(what, &response)?;
        let mut report = response.reader.take(MAX_REPORT);
        push::read_report(&mut report, &mut outcome)
            .map_err(|err| Error::new(format!("{what} {}: {}", remote.shown(), remote.redact(err.message()))))
    })();
    if let Err(err) = std::fs::remove_dir_all(scratch) {
        tracing::warn!("cannot remove {}: {err}", scratch.display());
    }
    result.map(|()| outcome)
}

/// Fetch the objects of `wants` from the server that gave `advertised`, telling it the commits of `haves` this store
/// has, into the quarantine directory `dir`: one pack, completed from `store`, each object held to `max_object` bytes.
/// Nothing is asked for when `wants` is empty (`None`).
pub(crate) fn fetch_pack<'a>(
    store: &'a dyn GitStore,
    remote: &Remote<'_>,
    advertised: &RemoteRefs,
    wants: &[ObjectId],
    haves: &[ObjectId],
    dir: &Path,
    max_object: Option<usize>,
) -> Result<Option<pack_in::Quarantine<'a>>, Error> {
    if wants.is_empty() {
        return Ok(None);
    }
    let mut caps = Vec::new();
    if advertised.caps.contains("ofs-delta") {
        caps.push("ofs-delta".to_string());
    }
    if advertised.caps.iter().any(|cap| cap.starts_with("agent=")) {
        caps.push(format!("agent={}", super::agent()));
    }
    let mut body = Vec::new();
    let mut sent = HashSet::new();
    for want in wants {
        if !sent.insert(*want) {
            continue;
        }
        let line = if sent.len() == 1 {
            format!("want {want} {}\n", caps.join(" "))
        } else {
            format!("want {want}\n")
        };
        pktline::data(&mut body, line.as_bytes()).map_err(io_error)?;
    }
    pktline::flush(&mut body).map_err(io_error)?;
    // no flush between the haves and done: with neither multi_ack nor a flush the server answers one line, an ACK at
    // the first common commit or a NAK at done, and then the pack
    for have in haves {
        pktline::data(&mut body, format!("have {have}\n").as_bytes()).map_err(io_error)?;
    }
    pktline::data(&mut body, b"done\n").map_err(io_error)?;
    let what = "fetching from";
    let response = remote
        .client(TRANSFER_TIMEOUT)
        .open(OpenRequest {
            method: Method::Post,
            url: remote.endpoint("git-upload-pack"),
            headers: remote.headers("git-upload-pack", true),
            body: Some(body.into()),
            timeout: None,
        })
        .map_err(|err| remote.failed(what, &err))?;
    remote.status(what, &response)?;
    let mut reader = BufReader::with_capacity(64 * 1024, response.reader.take(MAX_FETCH_PACK));
    loop {
        let buffered = reader.fill_buf().map_err(io_error)?;
        if buffered.starts_with(b"PACK") {
            break;
        }
        if buffered.is_empty() {
            return Err(Error::new(format!(
                "{what} {}: the server sent no pack",
                remote.shown()
            )));
        }
        match pktline::read(&mut reader).map_err(io_error)? {
            None => {
                return Err(Error::new(format!(
                    "{what} {}: the server sent no pack",
                    remote.shown()
                )))
            }
            Some(Packet::Data(data)) => {
                let line = String::from_utf8_lossy(&data);
                if let Some(message) = line.strip_prefix("ERR ") {
                    return Err(Error::new(format!(
                        "{what} {}: the server refused: {}",
                        remote.shown(),
                        remote.redact(message.trim())
                    )));
                }
            }
            Some(_) => {}
        }
    }
    let received = pack_in::receive(store, &mut reader, dir, max_object, super::pack_threads())
        .map_err(|err| Error::new(format!("{what} {}: {}", remote.shown(), remote.redact(err.message()))))?;
    if reader.get_ref().limit() == 0 {
        return Err(Error::new(format!(
            "{what} {}: the pack is more than {MAX_FETCH_PACK} bytes",
            remote.shown()
        )));
    }
    Ok(Some(received))
}
