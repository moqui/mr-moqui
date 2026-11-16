# One App for All Messaging

Plan, 2026-11-16. Nothing built. Companion to [Chat-Messaging-App.md](Chat-Messaging-App.md) (the app itself) and
[RCS.md](RCS.md).

The goal: one MR Moqui app replaces GroupMe, iMessage, Google Messages (SMS, MMS and RCS), Signal and Cheogram. The user
keeps talking to the same people on the same networks. The people on the other side change nothing. The app also keeps
every protocol MR Moqui speaks today, so people on XMPP apps, SIP phones, mail and the web reach the user too.

Two parts:
1. **Reach**: each network's people appear as rooms in the app (§2, §3).
2. **Equivalents**: each feature of each app has a feature in MR Moqui that replaces it (§4).

## 1. Principles

- **One inbox.** Every conversation, from any network, is a room in one list. A person with several networks is one
  contact, and the user can pick the network per message.
- **Best path first.** To a person who has MR Moqui: Moqui chat (E2EE). Else the network they use. Else SMS.
- **Say what is private.** Every room shows its network and its E2EE state ([Chat-Messaging-App §6](Chat-Messaging-App.md)).
  A bridge always gets the label "Not end-to-end encrypted on this server. The bridge reads messages to pass them on."
- **Official ways first.** An official API before an unofficial one. Each unofficial bridge is an opt-in build feature,
  marked experimental, and runs as a separate process.
- **No lock-in.** History comes in from the old app (§5), and the user can export it again.

## 2. The protocols MR Moqui keeps

These are built ([Future-Plans, Comms](Future-Plans.md#comms)). The app must keep them working and show them in the one
inbox.

| Protocol | Who it reaches | E2EE | Notes |
|---|---|---|---|
| Moqui chat (web, desktop, Android) | MR Moqui users | Yes in encrypted rooms (Megolm) | The home network |
| WebRTC calls | MR Moqui users and guests by link | Yes (SFrame) | [Chat-Messaging-App §8](Chat-Messaging-App.md) |
| XMPP (C3a) | People on XMPP apps (Conversations, Cheogram, Monal, Dino, Gajim) with an account on this server | OMEMO between XMPP apps | Federation to other XMPP servers is C3b, not built |
| SIP | Desk phones, soft phones, the phone network through a trunk | No | Calls, IVR, voicemail |
| SMS and MMS | Any phone number, through Telnyx | No | One room per outside number with its texts, calls and voicemail |
| Mail | Any mail address | No (TLS only in transit) | The Comms Inbox; a person's mail can show beside their chats |
| Matrix (C6, planned) | Matrix users | Megolm possible | Also the way to host Matrix bridges (§3.2) |

## 3. Reaching each network

### 3.1 Summary

| App | How the app reaches its people | Official? | E2EE through MR Moqui | Needs |
|---|---|---|---|---|
| Google Messages: SMS, MMS | The user's number, ported to Telnyx (decided) | Yes | No | Built; live checks ([Chat-Messaging-App §9](Chat-Messaging-App.md)) |
| Google Messages: RCS | Business RCS through Telnyx; personal RCS only through a Google Messages for Web bridge | Business: yes. Personal: no | No | [RCS.md](RCS.md); see §3.4 |
| iMessage | A Mac the user owns runs BlueBubbles Server; an adapter talks to its API | No | No | An always-on Mac signed in to the Apple ID |
| Signal | A linked device (like Signal Desktop) through `signal-cli` | No (Signal allows linked devices; the client is third party) | No, the bridge decrypts | A Signal account on a phone to link from |
| GroupMe | GroupMe's public API v3 with the user's access token | Yes | GroupMe has no E2EE | The user's token (`request_secret`) |
| Cheogram | Replaced: the number moves to Telnyx (decided). XMPP contacts reach the user on this server's XMPP | — | OMEMO only between XMPP apps | C3b federation for contacts on other XMPP servers |

### 3.2 How bridges plug in

All networks go through the chat bridge layer ([Chat-Bridges](../docs/interfaces/Chat-Bridges.md)): a
`ChatBridgeAdapter` in `crates/moqui-channels`, one room per outside conversation, the bridge chip and the E2EE label.

| Way | Use for | Why |
|---|---|---|
| A Rust adapter on a documented API | GroupMe (REST), BlueBubbles (REST and its event socket), Telnyx RCS | Small, no extra process, same as the SMS adapter |
| A Rust adapter on a sidecar's JSON-RPC | Signal (`signal-cli` daemon, GPL-3) | The Signal protocol stays in a maintained client. The sidecar is a separate process |
| The Matrix appservice API (C6c) with a mautrix bridge as sidecar | Personal RCS (`mautrix-gmessages`), later WhatsApp, Telegram, Discord | One API in MR Moqui opens many maintained bridges. Each bridge is AGPL and runs as a separate process |

Each sidecar is optional (a cargo feature and a System screen to start it). The sidecar's license applies to the
sidecar, not to MR Moqui; a legal check is still needed before shipping one.

### 3.3 Per network

**iMessage**
- Apple has no API. The only working way is a Mac that stays on, signed in to the user's Apple ID, running BlueBubbles
  Server. Messages, attachments, read state and group chats work. Tapbacks, edits, unsend and typing need its Private API,
  which needs System Integrity Protection turned off.
- Adapter `bluebubbles.rs`: send and receive through the server's REST API and event socket; tapbacks map to reactions.
- Risks: Apple can block the Apple ID for unusual volume. The Mac and the server both read all messages.
- Without a Mac: iPhone users get SMS and MMS from the user's number (green bubbles). No RCS from this app (RCS.md).

**Signal**
- `signal-cli` links as a secondary device, the same way Signal Desktop does. It sends and receives messages, groups,
  reactions, replies, attachments, edits, deletes and read receipts over JSON-RPC.
- Adapter `signal.rs`: map each Signal chat and group to a room. Calls: show "Signal call, open Signal" (signal-cli does
  not carry calls).
- Risks: unofficial client; Signal can change the protocol; the linked device must connect at least every few weeks or
  Signal unlinks it. The server stores plaintext.
- The UI says plainly: "Signal messages are end-to-end encrypted until they reach your server. Your server can read them."

**GroupMe**
- Public API v3 (`api.groupme.com/v3`): groups, direct messages, likes, attachments, members. A user token reads groups,
  members and Topics; the push channel gives new messages.
- Adapter `groupme.rs`: each group and Topic is a room; likes map to a heart reaction; `@all` maps to `@room`.
- Risk: low; the API is old but works. GroupMe has no E2EE, so nothing is lost.

**Google Messages and RCS** (details in [RCS.md](RCS.md))
- SMS and MMS: through the ported number on Telnyx. Group texts: [Chat-Messaging-App §9](Chat-Messaging-App.md).
- Business RCS (RBM) through Telnyx: verified brand sender, rich cards, replies in the thread. Not person to person.
- Personal RCS: only a bridge to Google Messages for Web (`mautrix-gmessages` behind C6c). It needs a phone with Google
  Messages as the default SMS app and the number on its SIM.

**Cheogram**
- Its phone number role moves to Telnyx (decided). Its XMPP role stays: the user's XMPP contacts on this server reach the
  user in the app; XMPP apps (including Cheogram itself) can still sign in to this server.
- C3b federation brings contacts on other XMPP servers.

### 3.4 The RCS conflict

Personal RCS needs the number on a SIM in a phone with Google Messages (RCS.md). The user's number is moving to Telnyx,
so that number cannot have personal RCS. The choices:

| Choice | Result |
|---|---|
| A. Number on Telnyx (decided) | SMS, MMS, calls and voicemail in the app. Others see SMS, not RCS. Business RCS possible for a brand sender |
| B. Number stays on a SIM, Google Messages plus the bridge | Personal RCS through the bridge; SMS and MMS through the bridge too; no Telnyx calls on that number |
| C. Both: Telnyx number for the user, a second SIM line with RCS | Two numbers; people must know which to use |

The plan builds A. B and C stay open (§7).

## 4. Equivalent features

Each feature of each app and what replaces it in MR Moqui. "Built" means it exists today. A § number points to the plan
that adds it. "New" means this plan adds it (§6).

### 4.1 Messaging basics (all five apps)

| Feature | Equivalent in MR Moqui | Status |
|---|---|---|
| Direct and group chats | Direct, private and public rooms | Built |
| Replies, quotes | Reply, threads | Built |
| Edit, unsend | Edit (24 h), redact | Built |
| Reactions, tapbacks, likes | Reactions; a tapback or a like maps to a reaction across bridges | Built; picker in Chat-Messaging-App §4 |
| Mentions, @all | Mentions, `@room` | Built |
| Typing, read receipts (with an off switch) | Typing, read receipts, `CHAT_READ_RECEIPTS` | Built |
| Photos, video, files, voice notes | Uploads; voice notes in Chat-Messaging-App §3.3 | Partly built |
| Search, media gallery | Server search; Details tabs | Built; tabs in Chat-Messaging-App §5 |
| Pin, mute, archive | Pin, mute; archive | Pin and mute built; archive Chat-Messaging-App §2 |
| Multi-device: phone, desktop, web | Android, desktop app, web | Built |
| Notifications with reply | Chat-Messaging-App §7 | Plan |
| Voice and video calls | Chat-Messaging-App §8 | Built; plan for seamless |
| Polls | Polls | Built |

### 4.2 Google Messages

| Feature | Equivalent | Status |
|---|---|---|
| SMS, MMS, group MMS | Telnyx SMS, MMS; group texts | Built; group texts planned |
| RCS chat | Business RCS; personal RCS by bridge (§3.4) | RCS.md |
| Default SMS app | `sim-sms` build | Chat-Messaging-App §10 |
| Scheduled send | Scheduled send: a send time on the outbox item, sent by the server | New |
| Spam protection, block, report | Block a person or number; spam filter for outside numbers (Telnyx number reputation and a local list) | New |
| One-time codes: copy, delete after 24 h | Detect codes in SMS; Copy button in the notification; optional delete | New |
| Message reminders | "Remind me" on a message: a notification later | New |
| Smart replies, writing help | Suggested replies from `moqui-llm`, plain rooms only (never encrypted rooms) | New |
| Photo quality choice | Send original or compressed | New |
| Messages for web | The web client and the desktop app | Built |

### 4.3 iMessage and FaceTime

| Feature | Equivalent | Status |
|---|---|---|
| Blue-bubble chat with iPhone users | BlueBubbles bridge (§3.3), else SMS | New |
| Tapbacks | Reactions | Built |
| Edit and unsend within minutes | Edit, redact | Built |
| Send later | Scheduled send | New |
| Message effects, stickers, GIFs | Sticker packs; GIF search through a provider the server proxies; effects: no | New (stickers, GIFs) |
| Share location, live location | A location message with a map (the Maps app), and live location for a set time | New |
| Check In (tell a contact you arrived) | Later, on live location | Later |
| FaceTime audio and video, group FaceTime | WebRTC calls | Built; Chat-Messaging-App §8 |
| Handoff of a call between devices | "Move to this device" | Chat-Messaging-App §8 |
| Shared photo albums in a chat | Media tab; save to a shared drive folder | Media tab planned |
| Contact names and photos | Phone book import | Chat-Messaging-App §9 |

### 4.4 Signal

| Feature | Equivalent | Status |
|---|---|---|
| E2EE by default | Encrypted direct and private rooms by default | Chat-Messaging-App §6.5 |
| Safety numbers | Emoji and QR verification | Built |
| Disappearing messages | A per-room timer from 30 s to 4 weeks. The clients delete at the time, the server deletes by an exact job. Today retention is whole days by a daily job | New |
| View-once media | Media that opens once, then the clients delete it and the server deletes the file | New |
| Note to self | A room with only the user | New (check if a one-person room works) |
| Usernames, phone number hidden | Usernames; a phone number is never shown to other MR Moqui users | Built (check) |
| Linked devices list | Apps and devices | Built |
| Registration lock, PIN | Device keys, password, recovery key for key backup | Built |
| Screen security (no screenshots) | Android `FLAG_SECURE` option, per app or per encrypted room | New |
| Group links with admin approval | Room invite links and QR codes, with optional approval | New |
| Stories | Later, if wanted | Later |
| Sealed sender | Not possible: the server sees who sends. The UI says so (§6 of Chat-Messaging-App) | Not planned |
| Proxy for censored networks | TURN on 443 and the web socket on 443 | Partly (Chat-Messaging-App §8) |

### 4.5 GroupMe

| Feature | Equivalent | Status |
|---|---|---|
| Large groups | Public and private rooms; test with 5,000 members | Built; load test new |
| Join by SMS without the app | SMS members of a room: a phone number added to a room gets each message as a text with the sender's name, and its replies post to the room | New |
| Group invite links and QR | Invite links | New |
| Likes | Heart reaction | Built |
| Topics (subgroups) | Threads, or linked sub-rooms | Built (threads) |
| Events with RSVP | An event message with Going / Maybe / No, and a calendar entry | New |
| Polls | Polls | Built |
| Gallery | Media tab | Chat-Messaging-App §5 |
| Mute, @all | Mute, `@room` | Built |
| Directory and join requests | Browse rooms; join request for private rooms | Browse built; requests new |

### 4.6 Cheogram (XMPP)

| Feature | Equivalent | Status |
|---|---|---|
| Phone number: SMS, MMS, calls, voicemail | Telnyx number | Built; Chat-Messaging-App §9 |
| Calls in the Android dialer | Android `PhoneAccount` | Chat-Messaging-App §9 |
| XMPP chat with OMEMO | This server's XMPP for XMPP contacts; Moqui chat with Megolm in the app | Built (C3a) |
| Contacts on other XMPP servers | C3b federation | Not built |
| Group chats (MUC) | Rooms are MUC rooms | Built |
| Ad-hoc commands (XEP-0050) | Not needed after the number moves | Not planned |
| Stickers, reactions, replies, threads | Stickers new; the rest built | Partly |
| Push without Google (UnifiedPush) | UnifiedPush option | Chat-Messaging-App §7.4 |

## 5. Moving history in

| From | How | Notes |
|---|---|---|
| Phone SMS and MMS | The `sim-sms` build reads the phone's messages once and uploads them to the number's rooms | Needs the default SMS role during import |
| Signal | `signal-cli` backfill when linking (Signal offers "transfer message history") | Only what Signal transfers |
| iMessage | BlueBubbles backfill (pages of 1,000) | From the Mac's history |
| GroupMe | The API's message history per group | Full history |
| Cheogram | The XMPP archive (MAM) of the old account, if its server keeps it; or an export | Best effort |
| Google Messages | Through the phone's SMS store (`sim-sms` import); RCS history only through the bridge | RCS history mostly lost |

Each import is idempotent (an outside message id per network), so it can run again.

## 6. New work from this plan

| # | Work | Section |
|---|---|---|
| 1 | Unified contacts: one person with many networks; pick the network per message | §1 |
| 2 | Block and report; spam filter for outside numbers | §4.2 |
| 3 | Scheduled send, message reminders | §4.2, §4.3 |
| 4 | Disappearing messages (exact timer) and view-once media | §4.4 |
| 5 | Invite links and QR, with approval; join requests | §4.4, §4.5 |
| 6 | SMS members of a room (GroupMe's SMS join) | §4.5 |
| 7 | Stickers, GIF search, location and live location | §4.3 |
| 8 | Event messages with RSVP | §4.5 |
| 9 | Screen security option; one-time code helper; send original photo | §4.2, §4.4 |
| 10 | GroupMe adapter (`groupme.rs`) | §3.3 |
| 11 | Signal adapter on `signal-cli` (`signal.rs`), optional build feature | §3.3 |
| 12 | iMessage adapter on BlueBubbles (`bluebubbles.rs`), optional build feature | §3.3 |
| 13 | Telnyx RCS adapter | RCS.md |
| 14 | Matrix appservice API (C6c), then `mautrix-gmessages` for personal RCS | §3.2 |
| 15 | History imports | §5 |

**Order** (after phases 1 to 3 of [Chat-Messaging-App](Chat-Messaging-App.md): phone layout, notifications, calls):
1. The number on Telnyx, block and spam (the user stops using Google Messages and Cheogram).
2. GroupMe adapter (official API, low risk).
3. Signal adapter.
4. iMessage adapter (needs a Mac).
5. Disappearing messages, invite links, scheduled send, SMS members.
6. RCS: Telnyx business adapter, then C6c and the personal RCS bridge if §3.4 picks B or C.
7. The rest of §6.

**Done when** for each network: the user sends and gets text, a photo and a reaction with a real person on that network,
and the room shows the right network and E2EE label.

## 7. Open questions

- §3.4: number on Telnyx only (A), or keep a SIM line for personal RCS (B or C)?
- iMessage: is there a Mac that can stay on? Without one, iPhone users get SMS.
- Signal: accept that the server can read bridged Signal messages? The alternative is to keep the Signal app for those
  chats.
- Sidecars (signal-cli, BlueBubbles, mautrix): on the same server, or on a separate machine?
- Stories: wanted?
