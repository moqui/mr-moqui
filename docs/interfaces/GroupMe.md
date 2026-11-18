# GroupMe

**Not yet tested with GroupMe.** The adapter is built from GroupMe's developer documentation (dev.groupme.com) and,
where that says nothing, from the community's (groupme-js.github.io/GroupMeCommunityDocs). It is checked against a
fake GroupMe only. Have it tested live before you rely on it. The facts this guide could not confirm are listed at the
end.

A GroupMe bridge brings one person's GroupMe account into Chat:
- each of their groups, each Topic and each direct conversation is a room;
- a like is a heart reaction, both ways;
- `@all` is `@room`, both ways;
- pictures go both ways.

GroupMe has no end-to-end encryption, and the bridge reads every message to pass it on. The room's label says so: "Not
end-to-end encrypted. GroupMe has no end-to-end encryption, and the bridge reads messages to pass them on."

The design is [plans/Comms-Messaging.md](../../plans/Comms-Messaging.md) §13.3 and §18.3 phase 14.

- **Rust:** `crates/moqui-channels/src/groupme/` (adapter `groupme`): `mod.rs` (the settings, sends, file fetches),
  `api.rs` (the API, the image service and the push server), `push.rs` (the node's coordinator, the leases and the
  push session), `events.rs` (what comes in, and the catch-up).
- **Entities** (`ChatEntities.xml`):
  - the bridge type `CbtGroupMe` (GroupMe);
  - `ChatBridge.ownerUserId` (the owner, kept plain) and `ChatBridge.retryDate` (when the connection tries again);
  - `ChatRoomBridge.syncCursor` (each room's catch-up mark).

  The lease of a bridge's connection is a `moqui.edge.EdgeSenderLock` row, `groupme:<chatBridgeId>`.
- **Services:** `org.moqui.chat.GroupMeServices` (`GroupMe.js`): `ensure#Room`, `receive#Likes` and `check#Bridge`
  are internal; `get#MyGroupMe`, `connect#MyGroupMe` and `disconnect#MyGroupMe` are the user's own bridge.
- **Screens:** Settings, Chat, GroupMe (the user's own), and System, Chat Bridges (an administrator's).

## Connect

**A user connects their own GroupMe** in Settings, Chat, GroupMe:
1. On dev.groupme.com, sign in with the GroupMe account and choose Access Token at the top of the page. Copy the token.
2. Paste it in Access token and choose Connect.

That makes the user's GroupMe bridge, owned by them (`ownerUserId`). Its rooms appear in Chat under Bridged as the
bridge syncs.
- A user has at most one GroupMe bridge. Connect again replaces its token; an empty token keeps the one kept.
- The token is write-only: it is kept encrypted in the bridge's `configJson` and never shown again.
- The section shows the bridge's status: Connected (with its room count, and the room limit when it is reached), Not
  connected (with the reason), or Disconnected. The adapter keeps its last error as a code and a text (`tokenRefused`,
  `connectFailed`, `noToken`, `roomLimit`, `disconnected`); the section says the code's text in the user's language.
- **Disconnect** stops the bridge's connection and forgets its token. Its rooms stay, with their history, and no longer
  sync. Connecting again needs a token.
- Only the owner, and an administrator with `ChatBridgeAdmin` (System, Chat Bridges), see or change the bridge.

The property `chat_groupme_self_service` (`true` by default) turns this off when it is `false`: the section then shows
only a bridge the user already has, without its token form, and Connect is refused (`selfServiceOff`).

**An administrator** can also make a GroupMe bridge in System, Chat Bridges:
- network GroupMe, adapter `groupme`;
- `configJson` with at least `accessToken` and `ownerUserId` (the local user whose account it is).

`store#Bridge` refuses one without both (`tokenRequired`, `noOwner`), a second GroupMe bridge of the same owner
(`oneBridge`), and a new owner for a bridge that has rooms (`ownerFixed`).

**The token.** It is the user's own GroupMe access token, the same the GroupMe apps use. It lets the server do anything
the user can do on GroupMe. GroupMe ends it when the user changes their password or signs out everywhere. The bridge then
shows "GroupMe refused the access token: get a new one" (below).

## Which groups

The bridge's settings (`configJson`; a user's own bridge has the defaults, an administrator can set the rest):

| Key | Default | What |
| --- | --- | --- |
| `accessToken` | required | The token (kept encrypted, never returned) |
| `ownerUserId` | required | The local user whose account it is: the rooms' owner, and the only one whose messages go out |
| `groupIds` | every group | The groups to bring in, by GroupMe id (a list or comma-separated). Their Topics come with them |
| `skipGroupIds` | none | Groups to leave out |
| `topics` | `true` | Each Topic as a room of its own, "Group › Topic" |
| `directMessages` | `true` | Each direct conversation as a room. `false` keeps strangers from making rooms |
| `maxRooms` | 200 | The most rooms the bridge makes (groups, Topics and conversations together) |
| `maxDirectRooms` | 50 | The most direct conversations among them, so the groups the owner joins come before strangers' messages |
| `syncMinutes` | 60 | How often the catch-up runs while connected |
| `sendsPerMinute` | 20 | The sends a node makes a minute (the bridge's own `sendsPerMinute` wins) |
| `maxImageBytes` | 10 MB | The largest picture sent |
| `maxInboundMediaBytes` | 20 MiB | The largest picture or video fetched |
| `catchUpPageSize`, `catchUpPages` | 100, 5 | The messages a catch-up reads per page, and the pages a room per run |

A server-wide property, `chat_groupme_node_max` (50), is the most GroupMe bridges one node holds the connections of;
the others go to other nodes.

## How it works

- **Rooms.** Each group, each Topic and each direct conversation is a private room the owner owns, with the owner its
  one local member. The owner may add others to a room: they read it, and their messages are not sent
  (`notBridgeOwner`), since they would go from the owner's account.
  - A room's name is GroupMe's: the group's, "Group › Topic", or the person's. A new name on GroupMe renames the room
    unless the owner renamed it.
  - A room starts at GroupMe's newest message: earlier history is not brought in.
- **New messages: the push channel.** The server connects out to GroupMe's push server (Faye long-polling over HTTPS,
  `push.groupme.com/faye`), subscribed to the user's channel. GroupMe calls nothing on this server: there is no
  webhook, and an edge node takes no part.
  - **One node** holds each bridge's connection. Each node runs one coordinator thread for all GroupMe bridges. Every
    15 seconds it lists them in one query and claims or renews their leases in one pass (`moqui.edge.EdgeSenderLock`
    `groupme:<bridge>`, 120 seconds). It runs a session, a thread, only for each bridge it holds, up to
    `chat_groupme_node_max`.
    - When that node stops, its leases go, and another node takes the connections over at its next pass.
    - A node whose claims fail stops its sessions before the lease can lapse.
    - Each coordinator claims under a name of its own, so an old one never frees a new one's lease.
    - A bridge's configuration is read again only when its row changed.
  - After each connection, and every `syncMinutes`, a **catch-up** lists the groups and conversations, makes the rooms
    that are missing, and fetches each room's messages from its **mark** (`syncCursor`).
    - The mark moves past a page only once every message on it was taken.
    - A failed page, a failed message or the page cap stops the catch-up there; the next run goes on from the mark,
      within a minute.
    - A pushed message moves the mark only in a room that is caught up, so a gap is never skipped.
    - A message seen twice is kept once.
  - A wait of 30 seconds or more (a refused token, failures in a row) is kept as the bridge's `retryDate`, so a node that
    takes the connection over waits it out.
  - **Redirects.** A call that carries the token follows no redirect. A picture or video fetched from GroupMe follows at
    most three, each only to GroupMe's file hosts.
- **People.** A GroupMe person is an outside member of the room from their first message or like, with their GroupMe
  name and picture. The owner's own messages and likes from the GroupMe app show as the owner's GroupMe person.
- **Messages out.** The text (at most 1,000 characters, `textTooLong`) and its pictures. Each picture is uploaded to
  GroupMe's image service first.
  - Another file is refused (`fileNotCarried`): GroupMe takes only pictures from here.
  - A picture the image service does not take is tried again: nothing was posted yet.
  - Each message carries its delivery's key as GroupMe's `source_guid`. When an answer is lost, or GroupMe answers 500,
    502 or 503, the room's latest messages are looked up for it.
    - Found, the send is done.
    - Not found, it is tried again within GroupMe's minute of `source_guid` dedupe (the delivery's first three
      attempts), then it is marked uncertain, so it is never sent twice.
- **Mentions.** `@all` or `@everyone` from GroupMe mentions the room (`@room`, which still follows Chat's rule of who
  may mention a room). `@room` from here goes as `@all`, with every member of the group (a Topic's group) mentioned.
  A group of more than 100 others, or one whose members cannot be read, gets the text `@all` with no one mentioned by
  id.
- **Likes.** A heart (❤️) on a message is a like on GroupMe, and taking it back is an unlike. Any other reaction is
  refused (`reactionNotCarried`). GroupMe's likes come in as hearts and go when taken back.
- **Edits and deletes.** Edits and deletes made on GroupMe come in. One made here is not sent: Chat leaves Edit and
  Delete off for these messages.
- **Places and files from GroupMe.** A place is a line "Location: …" with an OpenStreetMap link. A file is the line "[A
  file on GroupMe: open GroupMe to get it]". Pictures and videos are fetched, only from GroupMe's file hosts.
- **Left out:** typing, read receipts, polls, events, galleries, emoji PowerUps (they show as their placeholder),
  reactions other than the heart, replies and edits sent from here, videos and files sent from here, system messages,
  GroupMe history before the bridge, and bots.

## Status and limits

- **Connected** once the push channel is subscribed.
- **Not connected** with the reason:
  - "GroupMe refused the access token: get a new one (docs/interfaces/GroupMe.md)". The token was ended or is wrong.
    Paste a new one (Settings, Chat, GroupMe, or the bridge's `configJson`). The bridge tries again every 5 minutes, and
    within seconds when the token changes.
  - Another failure after three in a row (GroupMe down, the network). Each try waits twice as long, up to 5 minutes.
- **Disconnected**: the owner disconnected it, or an administrator disabled it.
- **The room limit.** Past `maxRooms`, or `maxDirectRooms` for a direct conversation, no room is made, and the status
  says "room limit reached" (`roomLimit`) while it lasts. A conversation refused is not asked again for an hour.
- Only the node that holds the connection writes the status, so it is always the latest real outcome; a status is
  written only when it changes.
- **Rate limits.** GroupMe answers 420 or 429 when asked too much. A send is tried again later, and the push channel
  waits at least a minute. GroupMe does not document its numbers. A 403 is a refusal (its update limits, or the owner
  is not in the group); the send fails with GroupMe's reason.
- A send that could not go stays visible in the room, with Send again.

## Not confirmed

Read on dev.groupme.com on 2026-11-17. Confirmed there:
- the base URL `https://api.groupme.com/v3`, the token in `X-Access-Token`, the `{response, meta}` envelope, and the
  status codes (420 for the rate limit);
- messages: listing them (`after_id`, `limit` up to 100, 304 when none) and sending one (`source_guid` deduped for a
  minute with 409, text up to 1,000 characters, `image` attachments only from `i.groupme.com`);
- the image service (`POST image.groupme.com/pictures`, `payload.url`);
- `like` and `unlike` at `/messages/:conversation_id/:message_id/`;
- `/chats` and `/users/me`;
- the push channel: the handshake, the subscription to `/user/<id>` with `ext.access_token` and `timestamp`, the
  long-poll connect, the `line.create` shape, and the hourly signature.

Not confirmed (from the community's docs, or not found):
- `GET` and `POST /direct_messages`: the current page lists Direct Messages but shows only Chats;
- Topics: `/groups/:id/subgroups`, a Topic's messages under its own id, `parent_id` on `line.create`;
- the push events `direct_message.create`, `favorite`, `message.update`, `message.deleted` and `membership.create`;
- the `mentions` (`user_ids`, `loci`) and `reply` attachments, and videos at `v.groupme.com`;
- the order of a direct conversation's chat id (`a+b`, the smaller id first, used only when GroupMe gave none);
- the rate limit's numbers, and the largest picture;
- whether GroupMe treats `@all` as anything but text.

## Tests

`crates/moqui-channels/src/groupme/tests.rs`, against a fake GroupMe on loopback (its API, image service and push
server), with the push session driven a step at a time:
- the push channel's subscription with the token, a message into the group's room once, the owner's echo and a system
  message left;
- a send with its key and the token, a text too long, and another member's message refused;
- a like both ways, another emoji refused, the owner's own like not doubled;
- `@all` and `@room` both ways;
- a picture out through the image service and one in, a PDF refused, a file on another host not fetched;
- a Topic and a direct message as rooms of their own;
- an expired token and a new one, rate limits and the push channel's back-off;
- the catch-up after a reconnect, past one page, capped and resumed, a failed page followed by a push, and its mark with
  more than 20 likes mapped;
- a like again after an unlike;
- a lost answer found by its `source_guid`, a picture's failure tried again, and a 502 tried again then uncertain;
- the token never following a redirect, and a file's redirects only to the file hosts;
- `@room` in a Topic from its group's members, with the members unread, and in a large group;
- direct messages' own cap and the room-limit status;
- the user's own bridge (one per user, the token never shown, disconnect, the property off) and an administrator's
  checks;
- the leases claimed in one pass, and the coordinator's run: one node holds, another takes over when it stops, a
  disabled bridge's session stops;
- the status left to the session, and the settings' `Debug` without the token.
