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

//! A server page's media in the background (`moquiDesktop.media`): a page that plays audio or video tells the app
//! what plays, and the app keeps it playing when the app goes to the background or the screen turns off.
//!
//! - `start({title, artist, artworkUrl, playing, position, duration, rate})`: the app starts its foreground service
//!   of type mediaPlayback (`MediaPlayback.kt`), with a media notification and a MediaSession;
//! - `update({...})`: the same fields, for the notification and the session (play or pause, the place);
//! - `stop()`: the service and the notification go.
//!
//! The notification's and the session's buttons (play, pause, seek, stop) go back to the page as
//! `moquiDesktop.media.on(fn)` events. While the service runs, the activity does not pause the webview.
//!
//! The checks here: the action is one of [`ACTIONS`]; the texts are clipped to [`TEXT_MAX`] characters with no
//! control characters; the artwork is an https URL with a host and no user or password, else none; the numbers are
//! finite and not negative. Only a server page (`moqui-app`) in the main webview may ask (`commands::media_session`).
//!
//! **The media relay** ([`RELAY_PATH`], `moquiDesktop.mediaRelay(url, hosts)`): some media hosts send an open-ended
//! range (`Range: bytes=N-`, what `<video>` and `<audio>` ask) at about the media's own bit rate, too slow for a
//! faster speed or a deep buffer, and a bounded one at full speed. A media element of the server page plays
//! `/__moqui-app/media?url=…&hosts=…` instead: the app asks the host for at most [`RELAY_CHUNK`] bytes of each range
//! and answers 206 with that part; the element asks again for the rest. The checks are the web request's
//! (src/web.rs): https on 443, a DNS host on the page's allow-list, each redirect checked again, no cookies either way.
//! Only the `Range` header goes to the host, and only the media headers come back.

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::http::{Response, StatusCode};
use tauri::Url;

/// What a page may ask.
pub(crate) const ACTIONS: &[&str] = &["start", "update", "stop"];
/// The most characters of a title or an artist.
pub(crate) const TEXT_MAX: usize = 200;
/// The most characters of the artwork's URL.
pub(crate) const URL_MAX: usize = 2048;

/// What the page tells of its media, as it gives it.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artwork_url: Option<String>,
    pub playing: Option<bool>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub rate: Option<f64>,
}

/// What the native code gets (`MediaPlayback.fromPage`), checked.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
    /// seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    /// seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
}

/// The action, if it is one of [`ACTIONS`].
pub(crate) fn action(given: &str) -> Result<&'static str, String> {
    ACTIONS
        .iter()
        .find(|known| **known == given)
        .copied()
        .ok_or_else(|| format!("Not a media action: {}", crate::commands::clip(given, 40)))
}

/// A text the notification shows: no control characters, at most [`TEXT_MAX`]; none when empty.
fn text(given: Option<&str>) -> Option<String> {
    given.map(|t| crate::commands::clip(t, TEXT_MAX)).filter(|t| !t.is_empty())
}

/// The artwork's URL, if it is https with a host and no user or password.
pub(crate) fn artwork(given: Option<&str>) -> Option<String> {
    let given = given?.trim();
    if given.is_empty() || given.len() > URL_MAX {
        return None;
    }
    let url = Url::parse(given).ok()?;
    if url.scheme() != "https" || url.host_str().is_none_or(str::is_empty) || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.to_string())
}

/// A number of seconds or a rate: finite and not negative, else none.
fn amount(given: Option<f64>, max: f64) -> Option<f64> {
    given.filter(|n| n.is_finite() && *n >= 0.0).map(|n| n.min(max))
}

/// The page's media info, checked ([`MediaState`]).
pub(crate) fn state(info: &MediaInfo) -> MediaState {
    MediaState {
        title: text(info.title.as_deref()),
        artist: text(info.artist.as_deref()),
        artwork_url: artwork(info.artwork_url.as_deref()),
        playing: info.playing,
        // at most ten days
        position: amount(info.position, 864_000.0),
        duration: amount(info.duration, 864_000.0),
        rate: amount(info.rate, 16.0).filter(|r| *r > 0.0),
    }
}

/// What a notification's or the session's button tells the page: the script for `moquiDesktop.media.on`'s listeners,
/// or none for an action that is not one. `position` is in seconds, for `seekto`.
pub(crate) fn action_script(what: &str, position: Option<f64>) -> Option<String> {
    const PAGE_ACTIONS: &[&str] = &["play", "pause", "stop", "seekto", "seekforward", "seekbackward"];
    if !PAGE_ACTIONS.contains(&what) {
        return None;
    }
    let position = position.filter(|n| n.is_finite() && *n >= 0.0);
    let event = serde_json::json!({ "action": what, "position": position });
    Some(format!("window.__moquiMediaAction && window.__moquiMediaAction({event})"))
}

/// The path on the app origin of the media relay, the host's own, never forwarded to the server.
pub(crate) const RELAY_PATH: &str = "/__moqui-app/media";
/// The most bytes of one relayed range.
pub(crate) const RELAY_CHUNK: u64 = 2 * 1024 * 1024;
const RELAY_TIMEOUT_SECONDS: u64 = 30;

/// The range to ask the host for, from the element's `Range` header: `bytes=A-B` or `bytes=A-`, at most
/// [`RELAY_CHUNK`] bytes from A; from 0 for no header, a suffix range or more than one range.
pub(crate) fn bounded_range(header: Option<&str>) -> (u64, u64) {
    let asked = header
        .and_then(|h| h.trim().strip_prefix("bytes="))
        .filter(|r| !r.contains(','))
        .and_then(|r| {
            let (from, to) = r.split_once('-')?;
            let from: u64 = from.trim().parse().ok()?;
            let to = match to.trim() {
                "" => None,
                to => Some(to.parse::<u64>().ok().filter(|to| *to >= from)?),
            };
            Some((from, to))
        });
    let (from, to) = asked.unwrap_or((0, None));
    let last = from.saturating_add(RELAY_CHUNK - 1);
    (from, to.map_or(last, |to| to.min(last)))
}

/// The relay's target from its query: `url` on a host of `hosts` (comma-separated), checked as a web request's; and
/// the checked hosts, for the redirects.
pub(crate) fn relay_target(query: &str) -> Result<(Url, Vec<String>), String> {
    let mut url = None;
    let mut hosts = Vec::new();
    for (name, value) in tauri::Url::parse(&format!("http://x/?{query}")).map_err(|err| err.to_string())?.query_pairs() {
        match name.as_ref() {
            "url" => url = Some(value.into_owned()),
            "hosts" => hosts.extend(value.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty())),
            _ => {}
        }
    }
    let allowed = crate::web::host_list(&hosts)?;
    let url = crate::web::allowed_url(&url.ok_or("No url")?, &allowed)?;
    Ok((url, allowed))
}

fn relay_plain(status: StatusCode, text: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(text.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// The relay's answer to a media element's request (blocking: the scheme answers off the main thread).
pub(crate) fn relay(request: &tauri::http::Request<Vec<u8>>) -> Response<Vec<u8>> {
    let (target, allowed) = match relay_target(request.uri().query().unwrap_or("")) {
        Ok(target) => target,
        Err(err) => return relay_plain(StatusCode::BAD_REQUEST, &err),
    };
    let range = request.headers().get("range").and_then(|v| v.to_str().ok());
    let (from, to) = bounded_range(range);
    match relay_fetch(target, &allowed, from, to) {
        Ok(response) => response,
        Err(err) => {
            log::debug!("Media relay: {err}");
            relay_plain(StatusCode::BAD_GATEWAY, &err)
        }
    }
}

fn relay_fetch(mut url: Url, allowed: &[String], from: u64, to: u64) -> Result<Response<Vec<u8>>, String> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(RELAY_TIMEOUT_SECONDS))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| format!("HTTP client: {err}"))?;
    for _ in 0..=crate::web::REDIRECTS_MAX {
        let response = client
            .get(url.clone())
            .header("range", format!("bytes={from}-{to}"))
            .send()
            .map_err(|err| err.without_url().to_string())?;
        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("A redirect with no location")?;
            let next = url.join(location).map_err(|err| format!("A redirect to no URL: {err}"))?;
            url = crate::web::allowed_url(next.as_str(), allowed)?;
            continue;
        }
        let header = |name: reqwest::header::HeaderName| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let content_type = header(reqwest::header::CONTENT_TYPE);
        let content_range = header(reqwest::header::CONTENT_RANGE);
        let limit = to - from + 1;
        let mut bytes = Vec::new();
        response
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| format!("The answer was not read: {err}"))?;
        // a host that sends the whole file for a range: too much to hand on as it is
        if status == reqwest::StatusCode::OK && bytes.len() as u64 > limit {
            return Err("The host does not answer ranges".to_string());
        }
        let mut answer = Response::builder()
            .status(status.as_u16())
            .header("Accept-Ranges", "bytes")
            .header("Cache-Control", "no-store")
            .header("Content-Length", bytes.len().to_string());
        if let Some(value) = content_type {
            answer = answer.header("Content-Type", value);
        }
        if let Some(value) = content_range {
            answer = answer.header("Content-Range", value);
        }
        return answer.body(bytes).map_err(|err| err.to_string());
    }
    Err(format!("More than {} redirects", crate::web::REDIRECTS_MAX))
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
