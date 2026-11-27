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

use serde::{Deserialize, Serialize};
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

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
