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

//! What the server advertises, in one place: service discovery (XEP-0030) answers these lists and nothing else, and
//! `session::tests::every_advertised_feature_answers` sends a request for each one and checks it is served, so an
//! advertised feature cannot be left without its handler (docs/interfaces/XMPP.md "What is served").

use crate::ns;

/// The server's own JID (the domain).
pub const SERVER: &[&str] = &[
    ns::DISCO_INFO,
    ns::DISCO_ITEMS,
    ns::PING,
    ns::VERSION,
    ns::CARBONS,
    ns::BLOCKING,
    ns::EXTDISCO,
    ns::PUSH,
    ns::ROSTER,
    ns::VCARD,
];

/// A user's own bare JID: the archive (XEP-0313) and PEP (XEP-0163).
pub const ACCOUNT: &[&str] = &[
    ns::DISCO_INFO,
    ns::MAM,
    ns::MAM_EXTENDED,
    ns::SID,
    ns::PUBSUB,
    "http://jabber.org/protocol/pubsub#publish",
    "http://jabber.org/protocol/pubsub#auto-create",
    "http://jabber.org/protocol/pubsub#persistent-items",
    "http://jabber.org/protocol/pubsub#publish-options",
    "http://jabber.org/protocol/pubsub#retrieve-items",
    "http://jabber.org/protocol/pubsub#retract-items",
    "http://jabber.org/protocol/pubsub#delete-nodes",
    "http://jabber.org/protocol/pubsub#config-node",
    "http://jabber.org/protocol/pubsub#access-open",
    "http://jabber.org/protocol/pubsub#access-presence",
    "http://jabber.org/protocol/pubsub#access-whitelist",
    "http://jabber.org/protocol/pubsub#filtered-notifications",
];

/// The rooms service (`rooms.<domain>`).
pub const ROOMS: &[&str] = &[ns::DISCO_INFO, ns::DISCO_ITEMS, ns::MUC];

/// A room.
pub const ROOM: &[&str] = &[
    ns::DISCO_INFO,
    ns::MUC,
    "muc_persistent",
    "muc_nonanonymous",
    "muc_unmoderated",
    "muc_unsecured",
    ns::MAM,
    ns::SID,
    "http://jabber.org/protocol/muc#stable_id",
    "http://jabber.org/protocol/muc#self-ping-optimization",
];

/// The upload service (`upload.<domain>`).
pub const UPLOAD: &[&str] = &[ns::DISCO_INFO, ns::UPLOAD];

/// The features Bind 2 takes inline (XEP-0386).
pub const BIND2_INLINE: &[&str] = &[ns::CARBONS, ns::SM, ns::CSI];
