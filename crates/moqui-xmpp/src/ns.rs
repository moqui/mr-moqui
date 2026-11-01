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

//! The XML namespaces the server reads and writes.

pub const STREAM: &str = "http://etherx.jabber.org/streams";
pub const CLIENT: &str = "jabber:client";
pub const FRAMING: &str = "urn:ietf:params:xml:ns:xmpp-framing";
pub const STREAMS_ERROR: &str = "urn:ietf:params:xml:ns:xmpp-streams";
pub const STANZAS: &str = "urn:ietf:params:xml:ns:xmpp-stanzas";
pub const TLS: &str = "urn:ietf:params:xml:ns:xmpp-tls";
pub const SASL: &str = "urn:ietf:params:xml:ns:xmpp-sasl";
pub const SASL2: &str = "urn:xmpp:sasl:2";
pub const SASL_CB: &str = "urn:xmpp:sasl-cb:0";
pub const BIND: &str = "urn:ietf:params:xml:ns:xmpp-bind";
pub const BIND2: &str = "urn:xmpp:bind:0";
pub const SESSION: &str = "urn:ietf:params:xml:ns:xmpp-session";
pub const FAST: &str = "urn:xmpp:fast:0";
pub const SM: &str = "urn:xmpp:sm:3";
pub const CSI: &str = "urn:xmpp:csi:0";
pub const ROSTER: &str = "jabber:iq:roster";
pub const DISCO_INFO: &str = "http://jabber.org/protocol/disco#info";
pub const DISCO_ITEMS: &str = "http://jabber.org/protocol/disco#items";
pub const CAPS: &str = "http://jabber.org/protocol/caps";
pub const PING: &str = "urn:xmpp:ping";
pub const VERSION: &str = "jabber:iq:version";
pub const CARBONS: &str = "urn:xmpp:carbons:2";
pub const FORWARD: &str = "urn:xmpp:forward:0";
pub const DELAY: &str = "urn:xmpp:delay";
pub const BLOCKING: &str = "urn:xmpp:blocking";
pub const BLOCKING_ERRORS: &str = "urn:xmpp:blocking:errors";
pub const MAM: &str = "urn:xmpp:mam:2";
pub const MAM_EXTENDED: &str = "urn:xmpp:mam:2#extended";
pub const RSM: &str = "http://jabber.org/protocol/rsm";
pub const DATA: &str = "jabber:x:data";
pub const MUC: &str = "http://jabber.org/protocol/muc";
pub const MUC_USER: &str = "http://jabber.org/protocol/muc#user";
pub const MUC_OWNER: &str = "http://jabber.org/protocol/muc#owner";
pub const CONFERENCE: &str = "jabber:x:conference";
pub const SID: &str = "urn:xmpp:sid:0";
pub const PUBSUB: &str = "http://jabber.org/protocol/pubsub";
pub const PUBSUB_EVENT: &str = "http://jabber.org/protocol/pubsub#event";
pub const PUBSUB_OWNER: &str = "http://jabber.org/protocol/pubsub#owner";
pub const PUBSUB_ERRORS: &str = "http://jabber.org/protocol/pubsub#errors";
pub const BOOKMARKS: &str = "urn:xmpp:bookmarks:1";
pub const AVATAR_DATA: &str = "urn:xmpp:avatar:data";
pub const AVATAR_METADATA: &str = "urn:xmpp:avatar:metadata";
pub const OMEMO_LEGACY: &str = "eu.siacs.conversations.axolotl";
pub const OMEMO_LEGACY_DEVICES: &str = "eu.siacs.conversations.axolotl.devicelist";
pub const OMEMO_LEGACY_BUNDLES: &str = "eu.siacs.conversations.axolotl.bundles:";
pub const OMEMO2: &str = "urn:xmpp:omemo:2";
pub const OMEMO2_DEVICES: &str = "urn:xmpp:omemo:2:devices";
pub const OMEMO2_BUNDLES: &str = "urn:xmpp:omemo:2:bundles";
pub const UPLOAD: &str = "urn:xmpp:http:upload:0";
pub const OOB: &str = "jabber:x:oob";
pub const EXTDISCO: &str = "urn:xmpp:extdisco:2";
pub const PUSH: &str = "urn:xmpp:push:0";
pub const CHATSTATES: &str = "http://jabber.org/protocol/chatstates";
pub const RECEIPTS: &str = "urn:xmpp:receipts";
pub const MARKERS: &str = "urn:xmpp:chat-markers:0";
pub const CORRECT: &str = "urn:xmpp:message-correct:0";
pub const RETRACT: &str = "urn:xmpp:message-retract:1";
pub const REACTIONS: &str = "urn:xmpp:reactions:0";
pub const REPLY: &str = "urn:xmpp:reply:0";
pub const FALLBACK: &str = "urn:xmpp:fallback:0";
pub const HINTS: &str = "urn:xmpp:hints";
pub const EME: &str = "urn:xmpp:eme:0";
pub const VCARD: &str = "vcard-temp";
pub const SASL_PLAIN: &str = "PLAIN";
