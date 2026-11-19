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

//! Uploads read as they arrive (a `multipart/form-data` body, a DAV `PUT`): their pace and how many arrive at once
//! (SEC-RTE-91), and whether the request names a user before its body is read (SEC-RTE-07).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{admission, BodyRefused, HttpState, ListenLimits};
use crate::guard::Event;

/// Bytes of an upload gathered before the reader takes them, on a blocking thread (it may write a spool file).
const UPLOAD_FEED: usize = 1024 * 1024;

/// A `multipart/form-data` body read as it arrives, up to `limit` bytes: its fields, and its files in memory or (past
/// 256 KiB) spooled to `runtime/tmp/upload`. Each chunk must come within `webapp-list/webapp[webroot]/web-protection/requests@upload-idle-timeout`.
/// `_slot` is the upload's place among those arriving ([`Uploads::take`]), held while it is read.
pub(crate) async fn read_multipart(
    state: &HttpState,
    body: axum::body::Body,
    limit: usize,
    content_type: &str,
    _slot: UploadSlot,
) -> Result<moqui_core::web::multipart::Parsed, BodyRefused> {
    use futures_util::StreamExt as _;
    use moqui_core::web::multipart::{MultipartError, MultipartReader, Spool};

    let refused = |err: MultipartError| match err {
        MultipartError::FieldTooLarge(_, field_limit) => BodyRefused::TooLarge(field_limit),
        MultipartError::Io(why) => BodyRefused::Failed(why),
        MultipartError::SpoolFull => BodyRefused::SpoolFull,
        other => BodyRefused::Malformed(other.to_string()),
    };
    let spool = Spool::under(state.factory.runtime_dir());
    let mut reader = MultipartReader::new(content_type, Some(spool)).map_err(refused)?;
    let mut stream = body.into_data_stream();
    let mut pending: Vec<u8> = Vec::with_capacity(UPLOAD_FEED);
    let mut total = 0usize;
    let started = Instant::now();
    loop {
        let next = match upload_wait(&state.limits, started.elapsed(), total)? {
            Some(wait) => tokio::time::timeout(wait, stream.next())
                .await
                .map_err(|_| BodyRefused::TimedOut)?,
            None => stream.next().await,
        };
        let done = match next {
            Some(Ok(chunk)) => {
                total += chunk.len();
                if total > limit {
                    return Err(BodyRefused::TooLarge(limit));
                }
                pending.extend_from_slice(&chunk);
                false
            }
            Some(Err(_)) => return Err(BodyRefused::Malformed("the upload was cut short".to_string())),
            None => true,
        };
        if pending.len() >= UPLOAD_FEED || done {
            let chunk = std::mem::replace(&mut pending, Vec::with_capacity(UPLOAD_FEED));
            let (back, fed) = tokio::task::spawn_blocking(move || {
                let fed = reader.feed(&chunk);
                (reader, fed)
            })
            .await
            .map_err(|err| BodyRefused::Failed(err.to_string()))?;
            reader = back;
            fed.map_err(refused)?;
        }
        if done {
            return reader.finish().map_err(refused);
        }
    }
}

/// A body that is one file (a DAV `PUT`) read as it arrives, up to `limit` bytes, as an upload's file part is: in
/// memory up to 256 KiB, then spooled to `runtime/tmp/upload`, never held whole (SEC-FIL-04). Each chunk must come
/// within `webapp-list/webapp[webroot]/web-protection/requests@upload-idle-timeout`. A body refused or cut short leaves no file.
/// Its sender's credentials held before it is read, so it is an upload with a user of `client`'s.
pub(crate) async fn read_to_spool(
    state: &HttpState,
    body: axum::body::Body,
    limit: usize,
    file_name: &str,
    content_type: &str,
    client: IpAddr,
) -> Result<moqui_core::web::multipart::UploadedFile, BodyRefused> {
    use futures_util::StreamExt as _;
    use moqui_core::web::multipart::{BodySpool, MultipartError, Spool};

    let refused = |err: MultipartError| match err {
        MultipartError::SpoolFull => BodyRefused::SpoolFull,
        other => BodyRefused::Failed(other.to_string()),
    };
    let _slot = state.uploads.take(&state.limits, client, true)?;
    let mut spool = BodySpool::new(Spool::under(state.factory.runtime_dir()));
    let mut stream = body.into_data_stream();
    let mut pending: Vec<u8> = Vec::with_capacity(UPLOAD_FEED);
    let mut total = 0usize;
    let started = Instant::now();
    loop {
        let next = match upload_wait(&state.limits, started.elapsed(), total)? {
            Some(wait) => tokio::time::timeout(wait, stream.next())
                .await
                .map_err(|_| BodyRefused::TimedOut)?,
            None => stream.next().await,
        };
        let done = match next {
            Some(Ok(chunk)) => {
                total += chunk.len();
                if total > limit {
                    return Err(BodyRefused::TooLarge(limit));
                }
                pending.extend_from_slice(&chunk);
                false
            }
            Some(Err(_)) => return Err(BodyRefused::Malformed("the upload was cut short".to_string())),
            None => true,
        };
        if pending.len() >= UPLOAD_FEED || done {
            let chunk = std::mem::replace(&mut pending, Vec::with_capacity(UPLOAD_FEED));
            let (back, fed) = tokio::task::spawn_blocking(move || {
                let fed = spool.write(&chunk);
                (spool, fed)
            })
            .await
            .map_err(|err| BodyRefused::Failed(err.to_string()))?;
            spool = back;
            fed.map_err(refused)?;
        }
        if done {
            return spool.finish(file_name, content_type).map_err(refused);
        }
    }
}

/// The uploads arriving now on a listener set ([`HttpState::uploads`]): all of them, and by client, an IPv6 one by its
/// /64. Counted whether or not web protection's per-client parts are on (SEC-RTE-91).
#[derive(Default)]
pub(crate) struct Uploads {
    arriving: AtomicUsize,
    by_client: Mutex<HashMap<IpAddr, usize>>,
}

/// The share of `max-uploads` an upload with no user may take: the rest is kept for uploads by a user.
const ANONYMOUS_SHARE_PERCENT: usize = 75;

impl Uploads {
    /// The uploads arriving now.
    #[cfg(test)]
    pub(crate) fn arriving(&self) -> usize {
        self.arriving.load(Ordering::Acquire)
    }

    /// Whether an upload with no user would find room now: under the share of `max-uploads` uploads with no user take.
    pub(crate) fn anonymous_room(&self, limits: &ListenLimits) -> bool {
        self.arriving.load(Ordering::Acquire) < anonymous_cap(limits)
    }

    /// A slot for an upload of `client` (`has_user`: the request named a user before its body), or a 503 refusal:
    /// at most `max-uploads-per-client` of a client at once, and at most `max-uploads` in all, of which an upload with
    /// no user takes only a share, so neither one client nor anonymous uploads take every slot.
    pub(crate) fn take(
        self: &Arc<Self>,
        limits: &ListenLimits,
        client: IpAddr,
        has_user: bool,
    ) -> Result<UploadSlot, BodyRefused> {
        let client = upload_client(client);
        let cap = if has_user { limits.max_uploads } else { anonymous_cap(limits) };
        let taken = self
            .arriving
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| (count < cap).then_some(count + 1));
        if taken.is_err() {
            tracing::warn!(
                "an upload was refused: {cap} uploads are arriving at once (max-uploads{})",
                if has_user { "" } else { ", the share of uploads with no user" }
            );
            return Err(BodyRefused::SpoolFull);
        }
        let mut by_client = moqui_lib::sync::lock(&self.by_client);
        let count = by_client.entry(client).or_insert(0);
        if *count >= limits.max_uploads_per_client {
            drop(by_client);
            self.arriving.fetch_sub(1, Ordering::AcqRel);
            tracing::warn!(
                "an upload of {client} was refused: {} of its uploads are arriving at once (max-uploads-per-client)",
                limits.max_uploads_per_client
            );
            return Err(BodyRefused::SpoolFull);
        }
        *count += 1;
        Ok(UploadSlot {
            uploads: Arc::clone(self),
            client,
        })
    }
}

fn anonymous_cap(limits: &ListenLimits) -> usize {
    (limits.max_uploads * ANONYMOUS_SHARE_PERCENT / 100).max(1)
}

/// The key a client's uploads are counted under: its address, or an IPv6 address's /64.
fn upload_client(client: IpAddr) -> IpAddr {
    let client = client.to_canonical();
    if client.is_ipv6() {
        moqui_core::web::address::masked(client, 64)
    } else {
        client
    }
}

/// One upload arriving, counted in [`Uploads`] until it is dropped (SEC-RTE-91).
pub(crate) struct UploadSlot {
    uploads: Arc<Uploads>,
    client: IpAddr,
}

impl Drop for UploadSlot {
    fn drop(&mut self) {
        let mut by_client = moqui_lib::sync::lock(&self.uploads.by_client);
        if let Some(count) = by_client.get_mut(&self.client) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                by_client.remove(&self.client);
            }
        }
        drop(by_client);
        self.uploads.arriving.fetch_sub(1, Ordering::AcqRel);
    }
}

/// How long an upload that has had `total` bytes in `elapsed` may wait for its next chunk: `upload-idle-timeout`, cut
/// to when its average would fall under `upload-min-rate` once `body-timeout` (60 seconds when off) has passed; `None`
/// for no limit. A [`BodyRefused::TimedOut`] when it has fallen under already, so a client sending a byte now and then
/// does not hold the connection for as long as it likes (SEC-RTE-91).
pub(crate) fn upload_wait(limits: &ListenLimits, elapsed: Duration, total: usize) -> Result<Option<Duration>, BodyRefused> {
    if limits.upload_min_rate == 0 {
        return Ok(limits.upload_idle);
    }
    let grace = limits.body.unwrap_or(Duration::from_secs(60));
    let earned =
        Duration::from_millis(u64::try_from(total).unwrap_or(u64::MAX).saturating_mul(1000) / limits.upload_min_rate);
    let Some(left) = (grace + earned).checked_sub(elapsed).filter(|left| !left.is_zero()) else {
        return Err(BodyRefused::TimedOut);
    };
    Ok(Some(limits.upload_idle.map_or(left, |idle| idle.min(left))))
}

/// Whether the request names a user before its body is read: a session cookie of a signed-in session, or its own
/// credentials (`api_key`, `login_key`, Basic) that hold, checked without logging in
/// (`moqui_core::user::credential_user`). Asked of an upload that may be larger than an anonymous one (SEC-RTE-07);
/// a refused credential counts for web protection as a refused login does.
pub(crate) async fn upload_has_user(
    state: &HttpState,
    headers: &axum::http::HeaderMap,
    https: bool,
    client: IpAddr,
) -> bool {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    // the session id of a signed cookie this node made
    let session = crate::notws::session_cookie(headers, https)
        .and_then(|cookie| moqui_core::user::verify_session_cookie(&cookie).ok().flatten())
        .filter(|id| !id.is_empty());
    let (api_key, login_key, authorization) = (header("api_key"), header("login_key"), header("authorization"));
    if session.is_none() && api_key.is_none() && login_key.is_none() && authorization.is_none() {
        return false;
    }
    let factory = Arc::clone(&state.factory);
    let guard = Arc::clone(&state.guard);
    let found = admission::run(
        Arc::clone(&state.factory),
        Arc::clone(&state.guard),
        state.guard.conf().queue_timeout,
        move || {
            if let Some(session) = session {
                if matches!(crate::notws::user_of(&factory, &session), Ok(Some(_))) {
                    return true;
                }
            }
            let user = moqui_core::user::with_client_ip(&client.to_string(), || {
                moqui_core::user::credential_user(
                    api_key.as_deref(),
                    login_key.as_deref(),
                    authorization.as_deref(),
                    true,
                    // the route's own login asks its protocol's scope
                    None,
                )
            });
            if moqui_core::web::take_auth_failure() {
                guard.record(client, Event::AuthFailure);
            }
            matches!(user, Ok(Some(_)))
        },
    )
    .await;
    matches!(found, Ok(true))
}


#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
