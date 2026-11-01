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

//! Moqui names as JID parts (docs/interfaces/XMPP.md "Addresses"): a user is `username@domain`, with the characters a
//! localpart cannot hold escaped as XEP-0106 has them (an email address as a username is `jane\40example.com@domain`),
//! in lower case, as usernames are compared without case. A room is `room@rooms.domain`, its `chatRoomId` when that is
//! already a lower-case localpart, else `x-` and the id's bytes in hex.

/// The XEP-0106 escapes.
const ESCAPES: [(char, &str); 10] = [
    (' ', "\\20"),
    ('"', "\\22"),
    ('&', "\\26"),
    ('\'', "\\27"),
    ('/', "\\2f"),
    (':', "\\3a"),
    ('<', "\\3c"),
    ('>', "\\3e"),
    ('@', "\\40"),
    ('\\', "\\5c"),
];

/// The localpart of a username.
pub fn escape_node(username: &str) -> String {
    let mut out = String::with_capacity(username.len());
    for ch in username.trim().chars().flat_map(char::to_lowercase) {
        match ESCAPES.iter().find(|(c, _)| *c == ch) {
            Some((_, escaped)) => out.push_str(escaped),
            None => out.push(ch),
        }
    }
    out
}

/// The username of a localpart (XEP-0106 unescaped).
pub fn unescape_node(node: &str) -> String {
    let mut out = String::with_capacity(node.len());
    let mut rest = node;
    while let Some((before, after)) = rest.split_once('\\') {
        out.push_str(before);
        let found = after.get(..2).and_then(|code| {
            ESCAPES
                .iter()
                .find(|(_, escaped)| escaped[1..].eq_ignore_ascii_case(code))
        });
        match found {
            Some((ch, _)) => {
                out.push(*ch);
                rest = &after[2..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The localpart of a room.
pub fn room_node(chat_room_id: &str) -> String {
    let plain = !chat_room_id.is_empty()
        && !chat_room_id.starts_with("x-")
        && chat_room_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.'));
    if plain {
        return chat_room_id.to_string();
    }
    let mut out = String::from("x-");
    for byte in chat_room_id.bytes() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The `chatRoomId` of a room localpart; None for one that is not a room's.
pub fn room_id(node: &str) -> Option<String> {
    let Some(hex) = node.strip_prefix("x-") else {
        return (!node.is_empty()).then(|| node.to_string());
    };
    if hex.is_empty() || hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|at| hex.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
        .collect();
    String::from_utf8(bytes?).ok()
}

/// A JID split into its parts, without validation beyond the separators; [`parse`] prepares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub node: Option<String>,
    pub domain: String,
    pub resource: Option<String>,
}

impl Address {
    pub fn bare(&self) -> String {
        match &self.node {
            Some(node) => format!("{node}@{}", self.domain),
            None => self.domain.clone(),
        }
    }

    pub fn full(&self) -> String {
        match &self.resource {
            Some(resource) => format!("{}/{resource}", self.bare()),
            None => self.bare(),
        }
    }
}

/// A JID as the jid crate prepares it (stringprep: lower-case node and domain), or None when it is not one.
pub fn parse(text: &str) -> Option<Address> {
    let jid = jid::Jid::new(text.trim()).ok()?;
    Some(Address {
        node: jid.node().map(|node| node.as_str().to_string()),
        domain: jid.domain().as_str().to_string(),
        resource: jid.resource().map(|resource| resource.as_str().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_and_rooms_round_trip_as_localparts() {
        assert_eq!(escape_node("John.Doe"), "john.doe");
        assert_eq!(escape_node("jane@example.com"), "jane\\40example.com");
        assert_eq!(unescape_node("jane\\40example.com"), "jane@example.com");
        assert_eq!(unescape_node("a\\5cb\\zz"), "a\\b\\zz");
        assert_eq!(room_node("100050"), "100050");
        assert_eq!(room_node("DemoRoom"), "x-44656d6f526f6f6d");
        assert_eq!(room_id("x-44656d6f526f6f6d").as_deref(), Some("DemoRoom"));
        assert_eq!(room_id(&room_node("x-1")).as_deref(), Some("x-1"));
        assert_eq!(room_id("x-4"), None);
        let address = parse("Jane\\40Example.com@Moqui.Local/phone").expect("a JID");
        assert_eq!(address.node.as_deref(), Some("jane\\40example.com"));
        assert_eq!(address.full(), "jane\\40example.com@moqui.local/phone");
        assert!(parse("a@b@c").is_none());
    }
}
