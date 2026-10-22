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

//! Git LFS at `/git/<repositoryId>.git/info/lfs/`: the batch API (`objects/batch`, the basic transfer), the objects
//! themselves (`PUT` and `GET objects/<oid>`, streamed, with a byte range), `verify`, and the locks API (`locks`,
//! `locks/verify`, `locks/<id>/unlock`). An object's content is the content blob of its SHA-256
//! (`crate::resource::blob`), so the same file in many repositories is stored once; a repository has an object
//! (`moqui.resource.GitLfsObject`) only once the content was uploaded to it, so knowing an oid does not reach another
//! repository's file. Over SSH, `git-lfs-authenticate` gives git-lfs the HTTPS endpoint and a token for it.
//!
//! The actions a batch answers carry a `MoquiLfs` token in their `Authorization` header: the user, repository,
//! operation and expiry, signed with `git_lfs_token_key` (or `git_lfs_token_key_previous`, while a key is rotated).
//! The token only says who the user is: each request is authorized again, as any other is. Guide:
//! `docs/Git-Hosting.md`, Large files.

use std::io::Read;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use moqui_lib::object::{basic_convert, Value};
use serde_json::{json, Value as Json};
use sha2::Sha256;

use super::{
    authenticate_with, may_push, open, property, provider, public_base_url, GitService, HttpResponder, Refusal,
};
use crate::context::Error;
use crate::entity::EntityValue;
use crate::resource::blob::{is_blob_id, BlobStore, Expected};
use crate::resource::git::RepositoryKind;
use crate::resource::source::READ_WINDOW;

pub const GIT_LFS_OBJECT: &str = "moqui.resource.GitLfsObject";
pub const GIT_LFS_LOCK: &str = "moqui.resource.GitLfsLock";

const LFS_TYPE: &str = "application/vnd.git-lfs+json";
/// A batch or locks request body.
const MAX_JSON: u64 = 1024 * 1024;
const MAX_BATCH: usize = 1000;
const BATCH_TOKEN_SECONDS: i64 = 15 * 60;
const SSH_TOKEN_SECONDS: i64 = 60 * 60;
const DEFAULT_MAX_OBJECT: u64 = 5 * 1024 * 1024 * 1024;

/// One request to the LFS endpoint of a repository.
#[derive(Clone, Debug, Default)]
pub struct LfsHttpRequest {
    pub repository: String,
    pub method: String,
    /// The path after `info/lfs/`.
    pub path: String,
    pub query: String,
    pub authorization: Option<String>,
    pub api_key: Option<String>,
    pub range: Option<String>,
    pub content_length: Option<u64>,
    /// Credentials may be taken: as for [`super::GitHttpRequest::secure`].
    pub secure: bool,
    pub client_ip: String,
}

/// The conf's `git-server` `lfs-max-object-size`: the largest object, 5 GiB.
pub fn max_object_size() -> u64 {
    crate::resource::git::setting("lfs-max-object-size")
        .parse::<u64>()
        .ok()
        .filter(|size| *size > 0)
        .unwrap_or(DEFAULT_MAX_OBJECT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Download,
    Upload,
}

impl Operation {
    fn code(self) -> &'static str {
        match self {
            Operation::Download => "d",
            Operation::Upload => "u",
        }
    }
}

/// What a token says.
struct Token {
    username: String,
    repository: String,
    operation: Operation,
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

/// A token key: set, and not the placeholder the conf ships.
/// The `git-server` attribute `name` (`lfs-token-key`), else the property `property`; None when empty or not yet set.
fn key(name: &str, property_name: &str) -> Option<String> {
    let configured = crate::resource::git::setting(name);
    let key = if configured.is_empty() {
        property(property_name)
    } else {
        configured
    };
    Some(key).filter(|key| !key.is_empty() && !key.ends_with("CHANGEME"))
}

fn tag(key: &str, payload: &str) -> Result<Vec<u8>, Error> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .map_err(|err| Error::new(format!("the LFS token key: {err}")))?;
    mac.update(payload.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

/// The `Authorization` header value of a token for `username` to `operation` on `repository`, for `seconds`.
fn issue(username: &str, repository: &str, operation: Operation, seconds: i64) -> Result<String, Error> {
    let key = key("lfs-token-key", "git_lfs_token_key").ok_or_else(|| Error::new("git_lfs_token_key is not set"))?;
    issue_with(&key, username, repository, operation, seconds)
}

fn issue_with(
    key: &str,
    username: &str,
    repository: &str,
    operation: Operation,
    seconds: i64,
) -> Result<String, Error> {
    let mut nonce = [0u8; 9];
    getrandom::fill(&mut nonce).map_err(|err| Error::new(format!("could not read random bytes: {err}")))?;
    let payload = json!({
        "u": username,
        "r": repository,
        "o": operation.code(),
        "e": now_seconds() + seconds,
        "n": URL_SAFE_NO_PAD.encode(nonce),
    })
    .to_string();
    let encoded = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(tag(key, &encoded)?);
    Ok(format!("MoquiLfs v1.{encoded}.{signature}"))
}

/// The token of a `MoquiLfs` header: `None` for another kind of header, an error for a token that is not good (a
/// wrong signature, expired).
fn read_token(header: &str) -> Option<Result<Token, Refusal>> {
    let keys: Vec<String> = [
        key("lfs-token-key", "git_lfs_token_key"),
        key("lfs-token-key-previous", "git_lfs_token_key_previous"),
    ]
    .into_iter()
    .flatten()
    .collect();
    read_token_with(&keys, header)
}

fn read_token_with(keys: &[String], header: &str) -> Option<Result<Token, Refusal>> {
    let token = header.trim().strip_prefix("MoquiLfs ")?.trim();
    let refused = |why: &str| {
        Some(Err(Refusal::Unauthenticated(format!(
            "the LFS token is not valid: {why}"
        ))))
    };
    let Some(rest) = token.strip_prefix("v1.") else {
        return refused("not a version 1 token");
    };
    let Some((encoded, signature)) = rest.split_once('.') else {
        return refused("no signature");
    };
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return refused("the signature is not base64");
    };
    let signed = keys.iter().any(|key| {
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).is_ok_and(|mut mac| {
            mac.update(encoded.as_bytes());
            mac.verify_slice(&signature).is_ok()
        })
    });
    if !signed {
        return refused("the signature does not match");
    }
    let Some(payload) = URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok())
    else {
        return refused("the payload is not JSON");
    };
    if payload["e"].as_i64().is_none_or(|expires| expires < now_seconds()) {
        return refused("it has expired");
    }
    let operation = match payload["o"].as_str() {
        Some("d") => Operation::Download,
        Some("u") => Operation::Upload,
        _ => return refused("no operation"),
    };
    Some(Ok(Token {
        username: payload["u"].as_str().unwrap_or_default().to_string(),
        repository: payload["r"].as_str().unwrap_or_default().to_string(),
        operation,
    }))
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        value => Some(moqui_lib::object::to_plain_string(value)).filter(|text| !text.is_empty()),
    }
}

/// A JSON response.
fn send(out: &mut dyn HttpResponder, status: u16, body: &Json) {
    let text = body.to_string();
    let headers = [
        ("Content-Type", LFS_TYPE.to_string()),
        ("Cache-Control", "no-store".to_string()),
        ("Content-Length", text.len().to_string()),
    ];
    if out.head(status, &headers).is_ok() {
        let writer = out.body();
        let _ = writer.write_all(text.as_bytes());
        let _ = writer.flush();
    }
}

fn message(status: u16, text: &str) -> (u16, Json) {
    (status, json!({ "message": text }))
}

fn refuse(out: &mut dyn HttpResponder, refusal: &Refusal) {
    let status = match refusal {
        Refusal::Unauthenticated(_) => 401,
        Refusal::Forbidden(_) => 403,
        Refusal::NotFound(_) => 404,
        Refusal::Unavailable(_) => 503,
    };
    let text = json!({ "message": refusal.message() }).to_string();
    let mut headers = vec![
        ("Content-Type", LFS_TYPE.to_string()),
        ("Content-Length", text.len().to_string()),
    ];
    if status == 401 {
        headers.push(("LFS-Authenticate", "Basic realm=\"Moqui Git\"".to_string()));
        headers.push(("WWW-Authenticate", "Basic realm=\"Moqui Git\"".to_string()));
    }
    if out.head(status, &headers).is_ok() {
        let writer = out.body();
        let _ = writer.write_all(text.as_bytes());
        let _ = writer.flush();
    }
}

/// The body of a JSON request, at most [`MAX_JSON`] bytes.
fn read_json(body: &mut dyn Read) -> Result<Json, (u16, Json)> {
    let mut bytes = Vec::new();
    <&mut dyn Read as Read>::take(body, MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| message(400, &format!("the request body could not be read: {err}")))?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(message(413, "the request body is too large"));
    }
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|err| message(422, &format!("the request body is not JSON: {err}")))
}

/// Serve one request to a repository's LFS endpoint, in an entered context (a job of the git [`super::pool`]).
pub fn serve_lfs(request: &LfsHttpRequest, body: &mut dyn Read, out: &mut dyn HttpResponder) {
    let started = (|| -> Result<Option<Operation>, Refusal> {
        let _provider = provider()?;
        let has_credentials = request.authorization.is_some() || request.api_key.is_some();
        if has_credentials && !request.secure {
            return Err(Refusal::Forbidden(
                "credentials over plain HTTP are refused; use HTTPS".into(),
            ));
        }
        match request.authorization.as_deref().and_then(read_token) {
            Some(token) => {
                let token = token?;
                if token.repository != request.repository {
                    return Err(Refusal::Unauthenticated(
                        "the LFS token is for another repository".into(),
                    ));
                }
                crate::user::internal_login_user(&token.username)
                    .map_err(|err| Refusal::Unauthenticated(err.message().to_string()))?;
                Ok(Some(token.operation))
            }
            None => {
                if !authenticate_with(request.authorization.as_deref(), request.api_key.as_deref())? {
                    return Err(Refusal::Unauthenticated(format!(
                        "log in to reach repository {}",
                        request.repository
                    )));
                }
                Ok(None)
            }
        }
    })();
    let token = match started {
        Ok(token) => token,
        Err(refusal) => {
            refuse(out, &refusal);
            return;
        }
    };
    let Ok(provider) = provider() else {
        return;
    };
    let repo = match open(&provider, &request.repository, GitService::UploadPack) {
        Ok(repo) => repo,
        Err(refusal) => {
            refuse(out, &refusal);
            return;
        }
    };
    let lfs = Lfs {
        repository: request.repository.clone(),
        mirror: repo.definition.kind == RepositoryKind::Remote,
        token,
    };
    let path = request.path.trim_matches('/');
    let outcome = match (request.method.as_str(), path) {
        ("POST", "objects/batch") => lfs.batch(body).map(|reply| send(out, reply.0, &reply.1)),
        ("GET", path) if path.starts_with("objects/") => {
            return lfs.download(&path["objects/".len()..], request, out);
        }
        ("PUT", path) if path.starts_with("objects/") => lfs
            .upload(&path["objects/".len()..], request, body)
            .map(|reply| send(out, reply.0, &reply.1)),
        ("POST", "verify") => lfs.verify(body).map(|reply| send(out, reply.0, &reply.1)),
        ("GET", "locks") => lfs.list_locks(&request.query).map(|reply| send(out, reply.0, &reply.1)),
        ("POST", "locks") => lfs.create_lock(body).map(|reply| send(out, reply.0, &reply.1)),
        ("POST", "locks/verify") => lfs.verify_locks(body).map(|reply| send(out, reply.0, &reply.1)),
        ("POST", path) if path.starts_with("locks/") && path.ends_with("/unlock") => lfs
            .unlock(&path["locks/".len()..path.len() - "/unlock".len()], body)
            .map(|reply| send(out, reply.0, &reply.1)),
        _ => Err(message(404, "no such LFS endpoint")),
    };
    if let Err((status, body)) = outcome {
        send(out, status, &body);
    }
}

/// A request's repository and how its user came.
struct Lfs {
    repository: String,
    mirror: bool,
    /// The operation of the token the user came with; `None` for credentials.
    token: Option<Operation>,
}

type Reply = Result<(u16, Json), (u16, Json)>;

impl Lfs {
    /// The user may change the repository's LFS objects and locks.
    fn may_write(&self) -> Result<(), (u16, Json)> {
        if self.token == Some(Operation::Download) {
            return Err(message(403, "this token is for downloads"));
        }
        if self.mirror {
            return Err(message(403, "the repository is a read-only mirror"));
        }
        if !may_push(&self.repository) {
            return Err(message(
                403,
                &format!("you may not push to repository {}", self.repository),
            ));
        }
        Ok(())
    }

    fn base(&self) -> String {
        format!("{}/git/{}.git/info/lfs", public_base_url(), self.repository)
    }

    fn entity(&self) -> Result<crate::entity::EntityFacade, (u16, Json)> {
        crate::context::ExecutionContext::with_current(|ec| ec.factory().entity())
            .flatten()
            .ok_or_else(|| message(503, "the database is not available"))
    }

    fn stored(&self, oid: &str) -> Result<Option<u64>, (u16, Json)> {
        let entity = self.entity()?;
        let _rows = crate::resource::git::rows_guard();
        let row = entity
            .find(GIT_LFS_OBJECT)
            .and_then(|find| {
                find.eq("repositoryId", Value::string(&self.repository))
                    .eq("oid", Value::string(oid))
                    .one()
            })
            .map_err(|err| message(500, err.message()))?;
        Ok(row.map(
            |row| match basic_convert(row.get("objectSize").unwrap_or(&Value::Null), "Long").value {
                Value::I64(size) => u64::try_from(size).unwrap_or(0),
                _ => 0,
            },
        ))
    }

    fn batch(&self, body: &mut dyn Read) -> Reply {
        let request = read_json(body)?;
        let operation = match request["operation"].as_str() {
            Some("download") => Operation::Download,
            Some("upload") => Operation::Upload,
            _ => return Err(message(422, "operation must be download or upload")),
        };
        if request["hash_algo"].as_str().is_some_and(|algo| algo != "sha256") {
            return Err(message(409, "only sha256 objects are kept"));
        }
        if operation == Operation::Upload {
            self.may_write()?;
        }
        let objects = request["objects"].as_array().cloned().unwrap_or_default();
        if objects.len() > MAX_BATCH {
            return Err(message(413, &format!("at most {MAX_BATCH} objects in a batch")));
        }
        let username = crate::user::current_username().ok().flatten().unwrap_or_default();
        let header = issue(&username, &self.repository, operation, BATCH_TOKEN_SECONDS)
            .map_err(|err| message(500, err.message()))?;
        let base = self.base();
        let max = max_object_size();
        let mut answered = Vec::with_capacity(objects.len());
        for object in objects {
            let oid = object["oid"].as_str().unwrap_or_default().to_string();
            let size = object["size"].as_u64();
            let (Some(size), true) = (size, is_blob_id(&oid)) else {
                answered.push(json!({ "oid": oid, "size": object["size"],
                    "error": { "code": 422, "message": "an object needs a SHA-256 oid and a size" } }));
                continue;
            };
            let action = |href: String| json!({ "href": href, "header": { "Authorization": header }, "expires_in": BATCH_TOKEN_SECONDS });
            let stored = self.stored(&oid)?;
            answered.push(match (operation, stored) {
                (Operation::Download, Some(_)) => json!({ "oid": oid, "size": size, "authenticated": true,
                    "actions": { "download": action(format!("{base}/objects/{oid}")) } }),
                (Operation::Download, None) => json!({ "oid": oid, "size": size,
                    "error": { "code": 404, "message": "the object is not in this repository" } }),
                (Operation::Upload, _) if size > max => json!({ "oid": oid, "size": size,
                    "error": { "code": 422, "message": format!("the object is over {max} bytes") } }),
                // there already: nothing to send
                (Operation::Upload, Some(_)) => json!({ "oid": oid, "size": size }),
                (Operation::Upload, None) => json!({ "oid": oid, "size": size, "authenticated": true,
                    "actions": { "upload": action(format!("{base}/objects/{oid}")),
                        "verify": action(format!("{base}/verify")) } }),
            });
        }
        Ok((
            200,
            json!({ "transfer": "basic", "objects": answered, "hash_algo": "sha256" }),
        ))
    }

    /// `GET objects/<oid>`: the content, or one byte range of it, streamed.
    fn download(&self, oid: &str, request: &LfsHttpRequest, out: &mut dyn HttpResponder) {
        let found = (|| {
            if !is_blob_id(oid) {
                return Err(message(404, "not an LFS object id"));
            }
            if self.stored(oid)?.is_none() {
                return Err(message(404, "the object is not in this repository"));
            }
            let blobs = BlobStore::new(self.entity()?);
            let _rows = crate::resource::git::rows_guard();
            let info = blobs
                .stat(oid)
                .map_err(|err| message(500, err.message()))?
                .ok_or_else(|| message(404, "the object's content is missing"))?;
            Ok(blobs.source(&info))
        })();
        let source = match found {
            Ok(source) => source,
            Err((status, body)) => return send(out, status, &body),
        };
        let total = source.len();
        let mut headers = vec![
            ("Content-Type", "application/octet-stream".to_string()),
            ("Accept-Ranges", "bytes".to_string()),
            ("ETag", format!("\"lfs-{oid}\"")),
            ("Cache-Control", "private, max-age=86400".to_string()),
        ];
        let (status, start, length) = match request
            .range
            .as_deref()
            .map(|range| crate::screen::static_files::byte_range(range, total))
        {
            None | Some(crate::screen::static_files::ByteRange::Whole) => (200, 0, total),
            Some(crate::screen::static_files::ByteRange::Unsatisfiable) => {
                headers.push(("Content-Range", format!("bytes */{total}")));
                let _ = out.head(416, &headers);
                return;
            }
            Some(crate::screen::static_files::ByteRange::Span(start, end)) => {
                headers.push(("Content-Range", format!("bytes {start}-{end}/{total}")));
                (206, start, end - start + 1)
            }
        };
        headers.push(("Content-Length", length.to_string()));
        if out.head(status, &headers).is_err() {
            return;
        }
        let writer = out.body();
        let mut sent = 0;
        while sent < length {
            let want = usize::try_from(length - sent).unwrap_or(usize::MAX).min(READ_WINDOW);
            let window = {
                let _rows = crate::resource::git::rows_guard();
                source.read_at(start + sent, want)
            };
            match window {
                Ok(window) if !window.is_empty() => {
                    if writer.write_all(&window).is_err() {
                        return;
                    }
                    sent += window.len() as u64;
                }
                Ok(_) => break,
                Err(err) => {
                    tracing::error!(repository = %self.repository, "LFS object {oid} could not be read: {}", err.message());
                    return;
                }
            }
        }
        let _ = writer.flush();
    }

    /// `PUT objects/<oid>`: the content, streamed into its blob and checked against its oid, then the object is the
    /// repository's.
    fn upload(&self, oid: &str, request: &LfsHttpRequest, body: &mut dyn Read) -> Reply {
        self.may_write()?;
        if !is_blob_id(oid) {
            return Err(message(422, "not an LFS object id"));
        }
        let max = max_object_size();
        if request.content_length.is_some_and(|length| length > max) {
            return Err(message(413, &format!("the object is over {max} bytes")));
        }
        let entity = self.entity()?;
        let blobs = BlobStore::new(entity.clone());
        let mut writer = blobs.begin().map_err(|err| message(500, err.message()))?.limit(max);
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let count = body
                .read(&mut buf)
                .map_err(|err| message(400, &format!("the upload was cut short: {err}")))?;
            if count == 0 {
                break;
            }
            writer.write(&buf[..count]).map_err(|err| message(413, err.message()))?;
        }
        let expected = Expected {
            id: Some(oid.to_string()),
            size: request.content_length,
        };
        let info = writer.finish(&expected).map_err(|err| message(422, err.message()))?;
        let _rows = crate::resource::git::rows_guard();
        if self.stored(oid)?.is_none() {
            let mut row = EntityValue::make(&entity, GIT_LFS_OBJECT).map_err(|err| message(500, err.message()))?;
            row.set("repositoryId", Value::string(&self.repository));
            row.set("oid", Value::string(oid));
            row.set("objectSize", Value::I64(i64::try_from(info.size).unwrap_or(i64::MAX)));
            if let Some(user) = crate::user::current_user_id().ok().flatten() {
                row.set("uploadedByUserId", Value::string(user));
            }
            row.set("uploadedDate", crate::user::now_effective_value());
            // another upload of the same object at the same time made it first: as good
            if let Err(err) = row.create(&entity) {
                if self.stored(oid)?.is_none() {
                    return Err(message(500, err.message()));
                }
            }
        }
        Ok((200, json!({})))
    }

    fn verify(&self, body: &mut dyn Read) -> Reply {
        self.may_write()?;
        let request = read_json(body)?;
        let oid = request["oid"].as_str().unwrap_or_default();
        match (self.stored(oid)?, request["size"].as_u64()) {
            (Some(stored), Some(size)) if stored == size => Ok((200, json!({ "message": "ok" }))),
            (Some(_), _) => Err(message(422, "the object has another size")),
            (None, _) => Err(message(404, "the object is not in this repository")),
        }
    }

    // ---- locks ----

    fn lock_json(&self, row: &EntityValue) -> Json {
        let owner = text(row.get("ownerUserId")).unwrap_or_default();
        let entity = self.entity().ok();
        let name = entity
            .and_then(|entity| {
                entity
                    .find("moqui.security.UserAccount")
                    .ok()?
                    .eq("userId", Value::string(&owner))
                    .one()
                    .ok()
                    .flatten()
            })
            .and_then(|account| text(account.get("username")))
            .unwrap_or(owner);
        let locked_at = match basic_convert(row.get("lockedDate").unwrap_or(&Value::Null), "Timestamp").value {
            Value::Timestamp(stamp) => stamp.to_string(),
            _ => String::new(),
        };
        json!({
            "id": text(row.get("lockId")).unwrap_or_default(),
            "path": text(row.get("lockPath")).unwrap_or_default(),
            "locked_at": locked_at,
            "owner": { "name": name },
        })
    }

    fn locks(&self) -> Result<Vec<EntityValue>, (u16, Json)> {
        let entity = self.entity()?;
        let _rows = crate::resource::git::rows_guard();
        Ok(entity
            .find(GIT_LFS_LOCK)
            .and_then(|find| {
                find.eq("repositoryId", Value::string(&self.repository))
                    .order_by("lockPath", false)
                    .list()
            })
            .map_err(|err| message(500, err.message()))?
            .into_values())
    }

    /// A page of `locks` from `cursor` (an offset), and the next cursor.
    fn page<'a>(
        locks: &'a [EntityValue],
        cursor: Option<&str>,
        limit: Option<u64>,
    ) -> (Vec<&'a EntityValue>, Option<String>) {
        let start = cursor.and_then(|cursor| cursor.parse::<usize>().ok()).unwrap_or(0);
        let limit = limit
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| *limit > 0)
            .unwrap_or(100);
        let page: Vec<&EntityValue> = locks.iter().skip(start).take(limit).collect();
        let next = (start + page.len() < locks.len()).then(|| (start + page.len()).to_string());
        (page, next)
    }

    fn list_locks(&self, query: &str) -> Reply {
        let pairs = crate::web::form_pairs(query);
        let param = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let mut locks = self.locks()?;
        if let Some(path) = param("path") {
            locks.retain(|row| text(row.get("lockPath")).as_deref() == Some(path));
        }
        if let Some(id) = param("id") {
            locks.retain(|row| text(row.get("lockId")).as_deref() == Some(id));
        }
        let (page, next) = Self::page(
            &locks,
            param("cursor"),
            param("limit").and_then(|limit| limit.parse().ok()),
        );
        let mut reply = json!({ "locks": page.iter().map(|row| self.lock_json(row)).collect::<Vec<_>>() });
        if let Some(next) = next {
            reply["next_cursor"] = json!(next);
        }
        Ok((200, reply))
    }

    fn create_lock(&self, body: &mut dyn Read) -> Reply {
        self.may_write()?;
        let request = read_json(body)?;
        let path = request["path"].as_str().unwrap_or_default().trim().to_string();
        if path.is_empty() {
            return Err(message(422, "a lock needs a path"));
        }
        let existing = |locks: &[EntityValue]| {
            locks
                .iter()
                .find(|row| text(row.get("lockPath")).as_deref() == Some(path.as_str()))
                .map(|row| self.lock_json(row))
        };
        if let Some(lock) = existing(&self.locks()?) {
            return Err((409, json!({ "lock": lock, "message": "the path is locked already" })));
        }
        let entity = self.entity()?;
        let _rows = crate::resource::git::rows_guard();
        let mut row = EntityValue::make(&entity, GIT_LFS_LOCK).map_err(|err| message(500, err.message()))?;
        row.set_sequenced_id_primary(&entity)
            .map_err(|err| message(500, err.message()))?;
        row.set("repositoryId", Value::string(&self.repository));
        row.set("lockPath", Value::string(&path));
        if let Some(name) = request["ref"]["name"].as_str() {
            row.set("refName", Value::string(name));
        }
        if let Some(user) = crate::user::current_user_id().ok().flatten() {
            row.set("ownerUserId", Value::string(user));
        }
        row.set("lockedDate", crate::user::now_effective_value());
        if let Err(err) = row.create(&entity) {
            // another lock of the path was made at the same time (its unique index)
            return match existing(&self.locks()?) {
                Some(lock) => Err((409, json!({ "lock": lock, "message": "the path is locked already" }))),
                None => Err(message(500, err.message())),
            };
        }
        Ok((201, json!({ "lock": self.lock_json(&row) })))
    }

    fn verify_locks(&self, body: &mut dyn Read) -> Reply {
        self.may_write()?;
        let request = read_json(body)?;
        let me = crate::user::current_user_id().ok().flatten().unwrap_or_default();
        let locks = self.locks()?;
        let (page, next) = Self::page(&locks, request["cursor"].as_str(), request["limit"].as_u64());
        let (ours, theirs): (Vec<&&EntityValue>, Vec<&&EntityValue>) = page
            .iter()
            .partition(|row| text(row.get("ownerUserId")).as_deref() == Some(me.as_str()));
        let mut reply = json!({
            "ours": ours.iter().map(|row| self.lock_json(row)).collect::<Vec<_>>(),
            "theirs": theirs.iter().map(|row| self.lock_json(row)).collect::<Vec<_>>(),
        });
        if let Some(next) = next {
            reply["next_cursor"] = json!(next);
        }
        Ok((200, reply))
    }

    fn unlock(&self, id: &str, body: &mut dyn Read) -> Reply {
        self.may_write()?;
        let request = read_json(body)?;
        let locks = self.locks()?;
        let row = locks
            .iter()
            .find(|row| text(row.get("lockId")).as_deref() == Some(id))
            .ok_or_else(|| message(404, "no such lock"))?;
        let me = crate::user::current_user_id().ok().flatten().unwrap_or_default();
        if text(row.get("ownerUserId")).as_deref() != Some(me.as_str()) {
            if !request["force"].as_bool().unwrap_or(false) {
                return Err(message(
                    403,
                    "the lock is another user's; force unlocking it needs the delete permission",
                ));
            }
            if !super::permitted(&self.repository, "delete") {
                return Err(message(
                    403,
                    "force unlocking another user's lock needs the delete permission",
                ));
            }
        }
        let lock = self.lock_json(row);
        let entity = self.entity()?;
        let _rows = crate::resource::git::rows_guard();
        row.delete(&entity).map_err(|err| message(500, err.message()))?;
        Ok((200, json!({ "lock": lock })))
    }
}

/// The largest LFS pointer file: git-lfs writes them well under this.
pub(crate) const MAX_POINTER: u64 = 1024;

/// The oid and size an LFS pointer file names (`version https://git-lfs.github.com/spec/v1`, `oid sha256:<hex>`,
/// `size <bytes>`), when `data` is one.
pub(crate) fn pointer(data: &[u8]) -> Option<(String, u64)> {
    if data.len() as u64 > MAX_POINTER {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let mut lines = text.lines();
    if !lines.next()?.starts_with("version https://git-lfs.github.com/spec/") {
        return None;
    }
    let (mut oid, mut size) = (None, None);
    for line in lines {
        if let Some(hex) = line.strip_prefix("oid sha256:") {
            oid = Some(hex.trim().to_string()).filter(|hex| is_blob_id(hex));
        } else if let Some(bytes) = line.strip_prefix("size ") {
            size = bytes.trim().parse::<u64>().ok();
        }
    }
    Some((oid?, size?))
}

/// The text of the LFS pointer file of content `oid` of `size` bytes.
pub(crate) fn pointer_text(oid: &str, size: u64) -> String {
    format!("version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {size}\n")
}

/// `git-lfs-authenticate <repository> <download|upload>` (and `git-lfs-transfer`, which is refused, so git-lfs uses
/// HTTPS): the repository and operation.
pub fn parse_ssh_command(command: &str) -> Option<(String, String)> {
    let mut words = command.split_whitespace();
    let program = words.next()?;
    if program != "git-lfs-authenticate" && program != "git-lfs-transfer" {
        return None;
    }
    let repository = super::repository_of(words.next()?.trim_matches(['\'', '"']))?;
    let operation = words.next().unwrap_or("download").to_string();
    Some((repository, operation))
}

/// The JSON `git-lfs-authenticate` prints for `username`: the repository's HTTPS LFS endpoint and a token for it,
/// for an hour.
pub(crate) fn ssh_authenticate(username: &str, command: &str) -> Result<String, Error> {
    let (repository, operation) =
        parse_ssh_command(command).ok_or_else(|| Error::new(format!("not an LFS command: {command}")))?;
    if command.trim_start().starts_with("git-lfs-transfer") {
        return Err(Error::new(
            "git-lfs-transfer is not served; git-lfs uses the HTTPS endpoint that git-lfs-authenticate gives",
        ));
    }
    let operation = match operation.as_str() {
        "download" => Operation::Download,
        "upload" => Operation::Upload,
        other => return Err(Error::new(format!("unknown LFS operation {other}"))),
    };
    let provider = provider().map_err(|refusal| Error::new(refusal.message().to_string()))?;
    crate::user::internal_login_user(username)?;
    let service = match operation {
        Operation::Download => GitService::UploadPack,
        Operation::Upload => GitService::ReceivePack,
    };
    open(&provider, &repository, service).map_err(|refusal| Error::new(refusal.message().to_string()))?;
    let header = issue(username, &repository, operation, SSH_TOKEN_SECONDS)?;
    Ok(json!({
        "href": format!("{}/git/{repository}.git/info/lfs", public_base_url()),
        "header": { "Authorization": header },
        "expires_in": SSH_TOKEN_SECONDS,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_signed_and_expire() {
        let key = "a key for the test of LFS tokens".to_string();
        let header = issue_with(&key, "john.doe", "repo", Operation::Upload, 60).unwrap();
        let keys = [key.clone()];
        let token = read_token_with(&keys, &header).unwrap().ok().unwrap();
        assert_eq!(
            (token.username.as_str(), token.repository.as_str(), token.operation),
            ("john.doe", "repo", Operation::Upload)
        );
        assert!(read_token_with(&keys, "Basic abc").is_none(), "another kind of header");
        // a changed signature, and an expired token, are refused
        let (payload, signature) = header.rsplit_once('.').unwrap();
        let mut forged = signature.to_string().into_bytes();
        forged[0] = if forged[0] == b'A' { b'B' } else { b'A' };
        let forged = format!("{payload}.{}", String::from_utf8(forged).unwrap());
        assert!(read_token_with(&keys, &forged).unwrap().is_err());
        let expired = issue_with(&key, "john.doe", "repo", Operation::Download, -1).unwrap();
        assert!(read_token_with(&keys, &expired).unwrap().is_err());
        // a key rotated out still reads while it is the previous one
        let rotated = ["the new key".to_string(), key];
        assert!(read_token_with(&rotated, &header).unwrap().is_ok());
        assert!(read_token_with(&rotated[..1], &header).unwrap().is_err());
    }

    #[test]
    fn pointers_name_an_oid_and_a_size() {
        let oid = crate::resource::blob::blob_id(b"content");
        let text = pointer_text(&oid, 7);
        assert_eq!(pointer(text.as_bytes()), Some((oid.clone(), 7)));
        assert_eq!(pointer(b"not a pointer"), None);
        let no_size = format!("version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\n");
        assert_eq!(pointer(no_size.as_bytes()), None);
    }

    #[test]
    fn ssh_commands_name_a_repository_and_an_operation() {
        assert_eq!(
            parse_ssh_command("git-lfs-authenticate 'media.git' upload"),
            Some(("media".to_string(), "upload".to_string()))
        );
        assert_eq!(
            parse_ssh_command("git-lfs-transfer /media.git download"),
            Some(("media".to_string(), "download".to_string()))
        );
        assert_eq!(parse_ssh_command("git-upload-pack 'media.git'"), None);
    }
}
