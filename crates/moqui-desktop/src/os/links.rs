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

//! The `mailto:`, `tel:` and `sip:` links the OS hands the app, as pages of the node (plans/Mobile-Desktop.md §4
//! configuration 1): a mail link opens the Comms app's Compose filled in, a call link its Dialer with the number. The
//! Comms app is the one under `comms_app_root` (`/qapps/BasicComms`, or `/qapps/Comms` with BOSS), as the node's own
//! links use.

use tauri::Url;

/// The schemes the app takes, as `plugins.deep-link.desktop.schemes` in tauri.conf.json registers them with the OS.
pub(crate) const SCHEMES: [&str; 3] = ["mailto", "tel", "sip"];

/// The Comms app when `comms_app_root` is not set, as in the node's own links (basic-comms/MoquiConf.xml).
pub(crate) const DEFAULT_COMMS_ROOT: &str = "/qapps/BasicComms";

/// The links among a launch's arguments: a Linux handler runs `MR-Moqui mailto:...` (the `.desktop` file's `%u`), on a
/// cold start as on a second launch. Other arguments (Chromium's switches) are not links of these schemes.
pub(crate) fn from_args(args: impl IntoIterator<Item = String>) -> Vec<Url> {
    args.into_iter()
        .filter_map(|arg| Url::parse(&arg).ok())
        .filter(|url| SCHEMES.contains(&url.scheme()))
        .collect()
}

/// The node's page for `link`, on the node at `node` (its origin is used), with the Comms app at `comms_root` (a path,
/// as `comms_app_root`). `None` for a link of another scheme, or one with nothing to open (a `tel:` with no number).
pub(crate) fn route(link: &Url, node: &Url, comms_root: &str) -> Option<Url> {
    let root = comms_root.trim().trim_end_matches('/');
    let root = if root.is_empty() { DEFAULT_COMMS_ROOT } else { root };
    let (path, params) = match link.scheme() {
        "mailto" => ("/Inbox/Compose", mail_params(link)),
        "tel" | "sip" => ("/Calls/Dialer", vec![("number".to_string(), call_number(link)?)]),
        _ => return None,
    };
    let mut url = node.clone();
    url.set_path(&format!("{root}{path}"));
    url.set_query(None);
    url.set_fragment(None);
    if !params.is_empty() {
        url.query_pairs_mut().extend_pairs(params);
    }
    Some(url)
}

/// Compose's parameters from a `mailto:` link (RFC 6068): the addresses of the path and of `to` headers as `to`, and
/// `cc`, `bcc`, `subject` and `body`. Other headers (`in-reply-to`, `attach` and the like) are dropped.
fn mail_params(link: &Url) -> Vec<(String, String)> {
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut bcc = Vec::new();
    let mut subject = None;
    let mut body = None;
    let path = percent_decode(link.path());
    if !path.trim().is_empty() {
        to.push(path.trim().to_string());
    }
    for pair in link.query().unwrap_or_default().split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        match percent_decode(name).to_ascii_lowercase().as_str() {
            "to" if !value.trim().is_empty() => to.push(value.trim().to_string()),
            "cc" if !value.trim().is_empty() => cc.push(value.trim().to_string()),
            "bcc" if !value.trim().is_empty() => bcc.push(value.trim().to_string()),
            "subject" if subject.is_none() => subject = Some(value),
            "body" if body.is_none() => body = Some(value),
            _ => {}
        }
    }
    let mut params = Vec::new();
    for (name, list) in [("to", to), ("cc", cc), ("bcc", bcc)] {
        if !list.is_empty() {
            params.push((name.to_string(), list.join(",")));
        }
    }
    for (name, value) in [("subject", subject), ("body", body)] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params.push((name.to_string(), value));
        }
    }
    params
}

/// The number or address to call: a `tel:` link's number without its parameters (`;ext=`, `;phone-context=`), a
/// `sip:` link's user when it is a number, else its `user@host`.
fn call_number(link: &Url) -> Option<String> {
    let text = percent_decode(link.path());
    let text = text.split(';').next().unwrap_or_default().trim();
    let text = text.split('?').next().unwrap_or_default().trim();
    let number = if link.scheme() == "sip" {
        match text.split_once('@') {
            Some((user, _)) if is_phone_number(user) => user,
            _ => text,
        }
    } else {
        text
    };
    (!number.is_empty()).then(|| number.to_string())
}

/// Whether `text` is a phone number as the Dialer reads one: digits, `+`, `*`, `#` and visual separators.
fn is_phone_number(text: &str) -> bool {
    text.chars().any(|ch| ch.is_ascii_digit()) && text.chars().all(|ch| ch.is_ascii_digit() || "+*#()-. ".contains(ch))
}

/// Decode `%XX` escapes as UTF-8 (lossy). A `+` stays a `+`: in a `mailto:` link it is not a space (RFC 6068 §5).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text
                .get(at + 1..at + 3)
                .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()));
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
