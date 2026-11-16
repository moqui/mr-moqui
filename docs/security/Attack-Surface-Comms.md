# Attack surface: calls, chat and messaging

The BasicComms app, chat, calls and their media, call links and guests, captions, recording, TURN, SIP, XMPP, voicemail, chat bridges and texts. The sockets and webhooks (`/notws`, `/callws`, `/channels`, `/xmpp-websocket`, `/xmpp-upload`) and the chat key transitions are rows in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). The mail client's screens are in [Attack-Surface-Mail.md](Attack-Surface-Mail.md), and the requests the server makes are in [Attack-Surface-Outbound.md](Attack-Surface-Outbound.md). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## The BasicComms app

BasicComms (`runtime/components/base/basic-comms`) is the framework's Comms app. Its `MoquiConf.xml` mounts `BasicComms` (menu title Comms) under `webroot/screen/webroot/apps.xml`, mounts `Voice` and `ChatBridges` under `component://tools/screen/System.xml`, and adds the token route `/call/<token>` ([Call links](#call-links)). It opens no port.

- **Who may open it.** The artifact group `BASIC_COMMS` holds `component://basic-comms/screen/BasicComms.xml` with `inheritAuthz="Y"` (`data/BasicCommsSetupData.xml`). `BASIC_COMMS_USERS` is allowed. `BASIC_COMMS_ADMIN` and `ADMIN` are always allowed. `ALL_USERS` has no grant. `BASIC_COMMS_ADMIN` is named only in that data file: no service or screen tests it. An app that mounts these screens under its own root authorizes them by that root ([Attack-Surface-Apps-BOSS.md](Attack-Surface-Apps-BOSS.md)).
- **Screens by family.** `Inbox` (`Mail`, `Compose`, `MessageDetail`) and `Settings/MailAccounts` are the mail client ([Attack-Surface-Mail.md](Attack-Surface-Mail.md#mail-client)). `Chat` and `Panel/Chat` are chat ([Chat](#chat)). `Calls` with `Call`, `History`, `Recordings`, `Voicemail`, `Dialer` and `Queues`, and `Panel/Dialer`, are calls and the phone ([Call services](#call-services), [Call recording](#call-recording), [Voicemail](#voicemail), [SIP](#sip)). `Settings` holds `ChatApps`, `Phone`, `PhoneDevices` and `CallHandling` (below), beside `Index`, `Calls`, `Chat`, `Devices`, `Identities`, `Rules`, `OutOfOffice` and `TextBlocks`. `Home` and `Panel` are the other two subscreens.
- **Transitions.** The transitions answer JSON. The `Chat` and `Calls` transitions that read are `read-only`, most with `method="get"`. The ones that change something (`send`, `upload`, `createRoom`, `addMembers`, `startCall`, `createCallLink` and the rest) declare no `method` ([CSRF](Attack-Surface-Web.md#csrf)).
- **Services.** The transitions call `org.moqui.comms.*` (the component's screen services), `org.moqui.chat.ChatServices`, `org.moqui.chat.ChatBridgeServices`, `org.moqui.chat.ChatKeyServices` and `org.moqui.call.*`. The phone screens and the Chat screen's bridge transitions call their service with authorization disabled, each in its own transaction, and the service decides by the signed-in user. No REST resource names those services: the one `*.rest.xml` file in `runtime/components/base` (`tools/service/moqui.rest.xml`) has no `org.moqui.chat`, `org.moqui.call` or `org.moqui.comms` service. They are reached only where a screen transition, a socket (`/notws`, `/callws`), an XMPP session, a job or server code names them. A service's `allow-remote` attribute is not the control ([services and `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)).
- **The people search.** `org.moqui.comms.CommsServices.search#Users` answers the user id and the full name (or username) of every enabled account whose username or name has the term. It reads `UserAccount` with `disable-authz="true"`, at most 50 a call, and an empty term matches every account. It leaves out usernames that start with `_`. It asks no provider and checks no organization, room or `EXTERNAL_USER`. It is behind the `searchUsers` transitions of `Chat`, `Calls`, `Calls/History` and `Settings/MailAccounts`, and of `template/VoiceTransitions.xml`.
- **The dialer.** `Calls/Dialer` and `Panel/Dialer` include `dialerData`, `dialSearch` and `dialNumber` (`template/DialerTransitions.xml`), which reach `CallHandlingServices.get#DialerData`, `search#Dialable` and `VoiceServices.place#PhoneCall` (its checks are under [SIP](#sip)). `search#Dialable` asks the `recipient-source` providers for people with phone numbers. When no provider answers, it searches `UserAccount` by username and full name (a contains match on a term of 2 to 100 characters) and returns each enabled user that one of the deployment's numbers rings, with those numbers, at most 25. That fallback checks no group or permission and does not test `EXTERNAL_USER`.
- **Phone screens.** `Calls/Voicemail` (`voicemail`, `audio`, `mark`, `delete`, `restore`, `retryTranscript`, `callBack`) reaches `VoicemailServices` by `voicemailId`: a message of a box the user does not read is `notFound`. `Settings/Phone` (`settings`, `saveSettings`, `setPin`, `uploadGreeting`, `deleteGreeting`, `greeting`) acts on the user's own box only. `Settings/PhoneDevices` (`devices`, `add`, `newPassword`, `remove`) reaches `SipDeviceServices`: a device that is not the user's is `notFound` unless the caller holds `VoiceAdmin`. `Settings/CallHandling` (`handling`, `change`) maps `op` to one of seven `CallHandlingServices` services through a fixed table; a rule, list or schedule of another user is `notFound`. `Calls/Queues` (`setStatus`) changes only the caller's own agent status.
- **Uploads and downloads.** Each download transition asks a service for a location by id, the service answers it only to a caller it allows, and `ec.web.sendResourceResponse` sends the file (`crates/moqui-core/src/web/mod.rs` `send_resource_response`). The content type comes from the stored file name. A user file is `private, no-store`. A file whose name has an active type (HTML, SVG, XML, JavaScript) is sent as an attachment even when inline is asked.
  - Chat files: `Chat/upload` and `Chat/attachment` ([Chat](#chat), its Files item).
  - Files sent in chunks: `Chat` and `Inbox/Compose` include `uploadChunk` (POST) and `uploadStatus` (GET) from `template/UploadTransitions.xml`, which call `org.moqui.edge.ClientSyncServices.upload#Chunk` and `get#UploadStatus`. An upload is found by the current user's id and `uploadId`, so a user reaches only their own. The limits are in [Chunked uploads](Attack-Surface-Files.md#chunked-uploads). An upload with no chunk for `client_upload_expire_hours` (24) is removed by `clean#ClientUploads`.
  - Recordings: `Calls/Recordings` `recordingFile` ([Call recording](#call-recording)).
  - Voicemail audio and greetings: `Calls/Voicemail` `audio`, `Settings/Phone` `greeting` and `uploadGreeting` ([Voicemail](#voicemail)).
  - Prompts, hold music and ring group greetings: System › Voice, below.
  - Mail attachments: [Attack-Surface-Mail.md](Attack-Surface-Mail.md#message-display-and-files).
- **Chat apps and app passwords.** `Settings/ChatApps` has the transitions `createXmppPassword` and `revokeAppPassword`, which call `org.moqui.impl.UserServices.create#UserAppPassword` and `revoke#UserAppPassword` for the session's user. `create#UserAppPassword` takes a `name` and makes 20 characters from a 31-symbol alphabet. It stores the SHA-256 login key hash and the SCRAM-SHA-256 salt, iteration count, stored keys and server keys (`UserAppPassword.scram*`, fields not encrypted). It asks for no recent sign-in and sets no expiry. The screen keeps the new password in the session attribute `newXmppPassword` until the screen is next rendered. The password is not bound to XMPP: the same row signs in to DAV, SSH and SFTP, git over HTTP, the LLM gateway and A2A (`login_app_password`; [login, lockout and passwords](Attack-Surface-Web.md#login-lockout-and-passwords)). mr-boss `Commons/Settings/Sync.xml` calls the same services.
- **System › Voice.** `System/Voice` has eleven subscreens: `Status`, `Trunks`, `Numbers`, `CallerIds`, `Fraud`, `Emergency`, `Phones`, `RingGroups`, `Schedules`, `IvrMenus` and `Queues`. The screens are authorized by the System app's group (`SYSTEM_APP`, `ADMIN` always; [Attack-Surface-Admin.md](Attack-Surface-Admin.md)). Every service behind them refuses a caller without the `VoiceAdmin` permission with `notAllowed`; `VoiceSetupData.xml` gives the permission to `ADMIN`. The families are `VoiceServices` (trunks, phone numbers, caller ID grants, emergency locations, the voice policy, user settings, auth blocks, the status), `IvrServices` (menus, options, prompts, call target options), `CallQueueServices` (queues, their agents and audio) and `CallHandlingServices` (ring groups, their members, the shared schedules). A trunk's password is written only: `find#VoiceTrunks` answers `hasPassword`. Three transitions take an audio upload: `IvrMenus/uploadPrompt`, `Queues/uploadAudio` and `RingGroups/uploadGreeting`. Each file is at most 10 MB, is checked as WAV or Ogg Opus by its first 96 bytes, and is stored under `dbresource://moqui/call/prompt/menu/<ivrMenuId>/`, `dbresource://moqui/call/prompt/queue/<callQueueId>/` or `dbresource://moqui/call/voicemail/greeting/<voicemailBoxId>/`. No screen transition serves a prompt or hold music file (no screen names `get#IvrPromptAudio`). `RingGroups/greeting` serves a ring group's greeting to a voice administrator. The shared transitions `searchUsers` and `searchTargets` (`template/VoiceTransitions.xml`) read `UserAccount` with authorization disabled and check no permission of their own.
- **System › ChatBridges.** `System/ChatBridges` is authorized by the System app's group too. It has two transitions, `setDisabled` (`ChatBridgeServices.store#Bridge`) and `retryDelivery`. The services check `ChatBridgeAdmin` themselves ([Chat bridges](#chat-bridges)).

## Chat

`org.moqui.chat.ChatServices` (`runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml`) holds rooms, members, messages, files, polls, read markers and presence. It is reached from the BasicComms `Chat` screen, from the root chat transitions, the `/ChatKeys` transitions and `/notws` ([Attack-Surface-Routes.md](Attack-Surface-Routes.md)), from XMPP sessions ([XMPP](#xmpp)) and from bridges ([Chat bridges](#chat-bridges)).

- **Who reaches a room.** Entity filters do not apply to `moqui.chat.*`, so each service that takes a `chatRoomId` asks `get#RoomAccess` for the current user. It answers yes for a current member (a direct room's members always), for any user when the room's visibility is `CrvUsers`, and for a record room when the theme's `STRT_RECORD_VISIBLE_SERVICE` answers yes for the user. A room the user cannot reach is "not found". An external user (`EXTERNAL_USER`) reaches no room bound to a record and no public room they are not a member of.
- **Roles.** Members post, react, pin, and edit or delete their own messages. Moderators and owners delete others' messages and change members. Owners change visibility and archive. The user who makes a room is its owner.
- **Members.** A member adds any enabled account to a room that is not direct (`add#Member`), and a user starts a direct room with any enabled account (`create#Room`). The account added is not asked, and no provider hook limits whom. Calls do ask the apps' `call-target` providers ([SIP](#sip)); chat has no such hook.
- **Acting as another user.** A parameter that names another user or actor (`get#RoomAccess.userId`, `send#Message.senderUserId` and `senderActorId`, `signal#Typing.userId`, `set#UserStatus.userId`, `get#UnreadCounts.userId`) is taken only when `ec.artifactExecution.authzDisabled()` is true. `create#Room` skips the record visibility check when authorization is disabled. The root transitions (`/setUserStatus`, `/chatTyping`, `/chatUnread`), the Chat screen's bridge transitions and `/notws` call chat services with authorization disabled; each builds its parameters from named fields and passes no `userId`.
- **Message bodies.** A body is `allow-html="safe"` and at most 65,536 characters; an encrypted room's ciphertext is at most 131,072. `clientRef` matches `^[A-Za-z0-9_.:-]{1,64}$`. The cleaner is ammonia's defaults plus three `span` attributes (`crates/moqui-lib/src/safe_html.rs`). It keeps an `img` element whose `src` names any host. The Chat screen shows the cleaned body with `v-html`, and the default `Content-Security-Policy` of a screen render has no `img-src`. In an encrypted room the client cleans the body with its own list (`WebrootChat.js`), which allows `img` with `src`.
- **Agent members.** `add#Member` and `set#MemberRole` take an `agentRef`, a service name, from a moderator or owner of the room. It is not compared with the agent's `AgentDefinition`. `deliver#AgentMessage` then calls the named service for each message of the room. The BasicComms `addMembers` and `setRole` transitions pass no `agentRef`.
- **Files.** A member who may post uploads a file with the Chat screen's `upload` transition (`ChatScreenServices.upload#ChatFiles`, then `ChatServices.upload#Attachment` for each file in its own transaction). The file is kept at `dbresource://chat/<chatRoomId>/<chatAttachmentId>/<name>` (`chat_attachment_root` names another root). The name is the last path segment with control characters removed, at most 200 characters. The content type is the one the client declared. The service sets no size limit and no quota; the request limit is `webapp_max_upload` (2 GiB by default). A file sent in chunks is at most 25 MB. A file never sent with a message is removed after 24 hours (`clean#PendingAttachments`). In an encrypted room the file is kept as `encrypted`, `application/octet-stream`. `send#Message` takes only files of that room that the sender uploaded and that no message has yet. The `attachment` transition (GET, `chatAttachmentId`) reads through `get#Attachment`: the room must be readable by the caller at the time of the read, and a file not yet sent is given only to its uploader. The file is sent as an attachment, or inline when `inline=true` and the stored type is png, jpeg, gif or webp. A file whose name has an active type is never inline.
- **Link previews.** After a message or its edit commits, `fetch#LinkPreviews` (`authenticate="anonymous-all"`, registered on commit by `queue#LinkPreviews`) makes the server fetch up to 3 links of the body, so any user who can post makes the server send requests. The request, its address guard and its limits are a row in [Attack-Surface-Outbound.md](Attack-Surface-Outbound.md#requests-the-server-makes). No picture is fetched. A preview is cached in `ChatLinkPreview` for 24 hours (1 hour when it failed), by the SHA-256 of the URL. Previews are off when `chat_link_previews` is `false`, when the sender's preference `CHAT_LINK_PREVIEWS` is `N`, in an encrypted room, and for a bridge's external actor. No limit other than the 3 links a message bounds how often a user makes the server fetch.
- **Presence.** `get#UserStatuses` (`authenticate="false"`, reached from `/userStatuses` with authorization disabled) answers, for up to 500 user ids of the caller's choice, each user's presence, status text, emoji, clear-at time, `inCall` and `lastActiveDate`. It checks no shared room. An external caller (`EXTERNAL_USER`) gets only their own. An id with no `UserStatus` row is answered as offline, the same as an id that is no account. For a user whose status is invisible the presence is `offline`, and the item still carries `lastActiveDate` and `inCall`.
- **Pushes.** Chat pushes are notifications that are not persisted, on the topics `chatRoom:<chatRoomId>` (to the room's member users), `chatUser:<userId>` and `userStatus` (to the user's room co-members). A `/notws` topic is only a filter: a notification is queued on the sockets of the users its `ROUTE_USER_IDS` name, so following another user's `chatRoom:<id>`, `chatUser:<userId>` or `rtc:<callSessionId>` topic delivers nothing.

## Call services

`org.moqui.call.CallServices` (logic in `runtime/components/base/framework/service/org/moqui/call/Calls.js`) are reached from the BasicComms `Calls` screens and from `/notws` and `/callws`.

- **Who sees a call.** A user sees a call they are a participant of and were not removed from, a meeting whose call link they own, and a group call of a room they are a current member of (not a direct or phone call). Any other call is "not found".
- **Binding a leg.** `bind#Connection` needs the leg to be the caller's, in that call, admitted and answered. The ICE servers go to an admitted participant with an answered leg.
- **Hosts and guests.** Only a call's host or the link's owner removes a participant. Someone with an answered leg in the call, or the link's owner, admits or denies a waiting guest.
- **Call links.** A call link belongs to its maker (`create#CallLink`; with a room, only one the user may post in). It lasts 1 hour to 90 days, a week by default. `find#CallLinks`, `revoke#CallLink` and `open#CallLink` act on the caller's own links.

## Call links

`/call/<token>` is a call link's guest page. Its token is 32 random characters; only its hash is stored. The token is
refused once the link is revoked or expired. The page's transitions are `joinGuest`, `guestState`, `iceServers`,
`recordingState`, `ackRecording`, `withdrawRecording` and `leaveGuest`, all
without the session token. `joinGuest` takes a name of up to 60 characters and gives a 40-character credential that
lasts 4 hours (hash kept). basic-comms declares a per-client rate limit for it (`call-guest`: `POST /call/*/joinGuest`,
10 a minute, burst 5), which applies only while per-client web protection is on (`webapp_trusted_proxies` set, or
`webapp_web_protection` `true`; [Web protection](Attack-Surface-Web.md#web-protection)). On the default conf `joinGuest`
has no per-client limit.

- A guest's credential reaches only that call's signaling, on `GET /callws`. The guest page never puts it in a URL.
  `guestState`, `iceServers` and `recordingState` are `read-only` with no `method`, so the server takes the credential
  from a query string as well as from a body ([CSRF](Attack-Surface-Web.md#csrf)). A wrong credential is not counted as
  a failed login.
- Each transition but `joinGuest` and `leaveGuest` checks the token again with `check#CallLinkToken` before it reads the
  credential.
- The route's context gives any holder of the token the link's name, its owner's name, the policy, whether it has
  video, whether it is encrypted, and whether a meeting going on is being recorded.
- `joinGuest` starts the link's meeting when none is going, on the first live media node, as a call started by the
  link's owner. Each join makes a `CallGuest`, a `CallParticipant` and a `CallLeg`, and pushes the guest's name to the
  people in the call and to the link's owner.
- A meeting takes at most 50 guests waiting or admitted (`callFull`). A guest who leaves, or whose credential runs out,
  keeps its admission status and still counts; only a removal or a denial changes it.
- `/callws` holds at most 64 sockets on a node that have not yet sent a credential, each for up to 10 seconds. The
  places are not per source. Another socket is answered 503.
- `guestState` shows a waiting guest nothing but their admission. Once admitted, a guest sees the names of those in the
  call. They never see the room, its members or its history.
- A guest leg can negotiate media only once admitted (`bind#GuestConnection`).
- Revoking the link, or removing or denying the guest, ends the credential and closes the guest's legs.
- A link made end-to-end encrypted (W8) takes no guest (`encryptedNoGuests`): a guest has no crypto device. The page says
  so from the token's route context (`encrypted`).

## Call captions

Call captions (W6, [WebRTC](../interfaces/WebRTC.md#captions)) send call audio out to a speech provider, so they are off
unless `call_captions_enabled` is `true`. `start#Captions` and `stop#Captions` (the Calls screen's `startCaptions`,
`stopCaptions`) need a signed-in user with an answered leg in the call; anyone else gets `notFound`. An end-to-end
encrypted call is refused. Every leg, a late joiner's and a guest's too, gets the notice naming the provider before its
audio is captioned. The provider is the `transcribe` model binding, reached through `moqui-llm`'s address guard (a
private address needs `allow-private-network`), counted against the quota of the user who turned captions on, and kept
as an `LlmCall`. No screen transition names `org.moqui.call.SpeechServices.transcribe#Audio` or `get#SpeechProvider`:
other services call them (captions, voicemail transcription, mr-mantle's process checks;
[services and `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)).
`transcribe#Audio` reads any `audioLocation` it is given, so its caller decides what may be sent out. Captions are not
stored unless `call_captions_store` is `true`.

## Call media

Off unless `rtc_media_port` is set, and only in a binary with the `rtc` feature (Maximal, Enterprise, Debug). One UDP socket on `rtc_media_host` (127.0.0.1 by default); browsers are told `rtc_public_address` (the bound address by default; a wildcard bind without it does not start). It carries ICE (STUN) and DTLS-SRTP for every browser leg (`crates/moqui-rtc`, on str0m, ICE-lite). A datagram goes to the peer whose ICE session (its ufrag and password from the SDP exchange over the authenticated socket) accepts it; anything else is dropped. A peer exists only after its leg's `join` was bound by the call services and reached the media node; leaving, a newer generation of the leg, the call's end (`send#MediaControl`) and ICE consent loss remove it. At most 200 rooms and 50 legs a room on a node; SDP over 64 KiB is refused. Media is hop by hop: the SFU decrypts and re-encrypts each stream, except the frames of an end-to-end encrypted call (below), which it forwards as they come. The node registers itself (`CallMediaNode`) every 20 seconds; a call whose node stops registering ends.

Cascading media nodes (`rtc_cascade_enabled`, off by default) add relay links between the media nodes of a call on the
same UDP sockets: one str0m peer a room on each side, DTLS-SRTP like a browser leg, negotiated only by `rtc-signal`
messages on the cluster's client-certificate HTTPS transport (kind `relay`), so only a cluster member opens one. The
origin takes at most 8 links a room and 64 in all, and refuses more (`tooManyRelays`); a message of an older link is
dropped, a room that ended takes no new link, and a link not connected within 10 seconds is dropped. A leg node forwards
to the origin only its own legs' tracks; the origin decides nothing a leg node asks beyond the layers it sends it. Any
cluster member may open a link for any room id: the cluster's members are trusted alike.

ICE-TCP (`rtc_media_tcp_port`, off unless set) takes TCP connections on the same address: at most 1,000, and 64 from one
source address. A connection that sends
nothing for 30 seconds is closed. Each carries
RFC 4571 frames of up to 2,048 bytes into the same SFU, which accepts only what a peer's ICE session takes.

A bound leg's `mute`, `hand`, `reaction` and `unpublish` call frames are sent to every other leg of the room as the
client wrote them, with `id` removed and `from` set to the sender's leg by the media node
(`crates/moqui-host/src/rtc/driver.rs`). An admitted guest's leg sends them too. A call frame is at most 128 KiB; a
larger one is refused (`tooLarge`). No rate limit applies to these frames. Any other `rtc` value is answered as an error.

End-to-end encrypted calls (W8, [WebRTC, Encrypted calls](../interfaces/WebRTC.md#encrypted-calls)) open nothing new.
Each browser SFrame-encrypts its frames (RFC 9605, AES-128-GCM on WebCrypto, in an `RTCRtpScriptTransform` worker) with
a key of its own per call and epoch. It sends that key to the other browsers' crypto devices as Olm to-device messages
over the `/ChatKeys` endpoints (C7 core), so the server holds and relays only Olm ciphertext; no chat Megolm session is
used. What the server and the media nodes still see: the RTP headers and header extensions (the audio level for the
active speaker), each frame's size and timing, the first 10 bytes of a VP8 keyframe and the first 3 of other VP8 frames
(kept in the clear so the SFU finds keyframes and sizes; authenticated as SFrame metadata), the SFrame header (the
sender's key index and epoch, the counter), and who is in the call with which device. The roster the browsers send keys
to is the server's (`get#Call`): a server that adds a device of a user in the call gets that leg's keys. A browser takes a
key only from the device the roster names for its leg (the Olm sender, which the server cannot forge), with that leg's key
index in its KID (refused while another leg has that index), and decrypts a track only with the keys of the leg it comes
from, each counter once (a replay window); a frame that does not decrypt is dropped, never played. A sender sends only
with keys it made, so a server-made roster cannot make two senders share a key and its nonces. A call page shows each
device as unverified (emoji and QR verification are in My devices, C7, but the call does not read their trust yet). The mode is set at the start (`requireE2ee`, a room's or a link's policy) and never lowered, and a
leg of such a call binds only with a crypto device of its user (`noCryptoDevice`); recording, captions, phone legs, desk
phones, forwarded numbers, guests and merges are refused. The browser's crypto store is encrypted at rest with a store
key the server gives only to its signed-in user (see Chat keys).

## Call recording

Off unless `call_recording_enabled` is true ([Call recording](../interfaces/Call-Recording.md)). It opens no port. Only a
host of the call starts, pauses, resumes or stops it (`org.moqui.call.CallRecordingServices`). The recorder runs on the
media node, takes frames from a bounded queue and spools under `runtime/tmp/call-recording/`. `store#RecordingFile`
(called by the recorder in the server; no screen transition names it) refuses a spool path outside that directory and a
location outside
`dbresource://moqui/call/recording/`. The files are read only through `get#RecordingFile` by segment id, from the
BasicComms Calls `Recordings` screen's `recordingFile` transition: the one who recorded it and the call's hosts get the
file, anyone else "not found" (an empty JSON answer) for the same id. No route serves those `dbresource://` locations
to users, and an app's record links (BOSS) go through the same services. The guest page's `ackRecording` and
`withdrawRecording` take the guest's credential and act on its own leg only. Every leg gets the notice, and each is
logged (`CallRecordingConsent`, with every earlier acknowledgement and withdrawal in `CallRecordingConsentHistory`).
The files count toward the owner's personal drive quota: a full drive refuses a start, and a recording that fills it
stops, so recording cannot fill the database past the quota by more than a segment. Retention deletes the files and
rows (`expire_Recordings_hourly`).

## TURN

The embedded TURN server (`crates/moqui-rtc` `turn/`, host `rtc/turn.rs`) runs on the media node. Each listener is off
unless set: `turn_port` (UDP and TCP), `turn_tls_port` (TLS), and 443 on `turn_tls443_host`. TLS uses the
`runtime/cert` certificates. Relays are UDP sockets in `turn_relay_ports`. The listeners bind `turn_host`, which is
`rtc_media_host` when empty. The URLs given to clients name `turn_public_host`, the media node's public address when
empty. Its rules:

- **Credentials** are short-lived TURN REST ones: `<expiry>:<legId>`, HMAC-SHA1 with `turn_auth_secret` (generated
  when unset; is-secret). They go only to an admitted leg (`get#IceServers`, `get#GuestIceServers`). An expired one
  is refused, and so is one of a leg that left (revoked by the media node, for 24 hours). On an edge node a local node's
  are `<expiry>:<legId>@<pairing>`, signed with that pairing's secret and taken only while it is active (E16). A credential
  lasts `turn_credential_hours` (4; held to 1..24). `turn_external_urls` are handed out with the same username and a
  credential under the same secret. A request with no credential is answered by `turn-server-proto` with a 401 and a
  nonce; a nonce lasts 600 seconds. An XMPP user's credential (`xmpp.<userId>`) is never revoked; it ends at its expiry.
- **Relay peers** are only the media nodes' media ports (and `turn_allowed_peers`, by address and port). A permission
  to any other address, or a channel to any other address or port, is dropped before the server sees it, and nothing
  is relayed to or from one, another port of a media node's address included. The exception is an XMPP user's own
  credential (`xmpp.<userId>`, from `XmppServices.get#IceServers`, XEP-0215), for calls between XMPP clients: it may
  relay to public unicast addresses too, never to loopback, private (RFC 1918, unique-local), link-local, shared
  (100.64/10), multicast or documentation ranges, nor an IPv6 address that carries an inside IPv4 one (6to4 of one,
  IPv4-compatible, NAT64, Teredo), nor another port of a media node's address (the `allowed_peers` addresses, which hold
  the media node's public
  address and its bind address when that is not a wildcard), nor the address a TURN listener is bound to
  (a relay there would come from the server itself, past a firewall that keeps the outside off those ports).
  A public address of the host that is neither (the address behind `turn_public_host` when it differs from the media
  address, another interface) is not refused.
- **Limits:** two allocations per credential, `turn_max_allocations` in all, and `turn_max_kbps` each way per
  allocation; excess data is dropped. A stream buffers at most 64 KiB without a whole message. No TCP relays to
  peers are offered.
  The two allocations are counted by username, and each request for ICE servers gives a username with its own expiry. A
  request whose username has a credential's form with an expiry still ahead is added to the server's user table before
  its MESSAGE-INTEGRITY is checked, and no code removes a name from the table. The TCP and TLS listeners take 512
  connections at once and 16 from one source (an IPv6 /64 is one). A connection has 10 seconds to send its first whole
  message (the TLS handshake included), and is closed after 10 minutes with nothing read or written, when 512 messages
  wait to be written to it, or when a write takes over 5 seconds. The UDP listener has no per-source limit; it reads
  datagrams into a 2,048 byte buffer.

coturn can take its place ([TURN](../interfaces/TURN.md)), with the same secret and a deny-all, allow-media-node
peer rule.

## SIP

The SIP engine (`crates/moqui-sip` on rsipstack, host `src/sip/`) runs beside a media node, only in a binary with the
`sip` feature (Maximal, Enterprise, Debug). Each listener is off unless set: `sip_port` (UDP and TCP on `sip_host`),
`sip_tls_port` (TLS on `sip_tls_host`, the `runtime/cert` certificates). Each phone leg has a UDP RTP socket in
`sip_rtp_ports` on `sip_rtp_host`. An exposed SIP listener is scanned within hours, so its rules are release gates
([SIP](../interfaces/SIP.md) "Fraud controls"):

- **Who is answered.** An INVITE from a trunk's host (each address of its SRV targets, for a trunk set up by name
  with no port, looked up again at each registration and ping) or listed signaling addresses is a trunk's call; any other
  INVITE or REGISTER is challenged with SIP Digest (SHA-256 and MD5, nonces that expire after 5 minutes and carry
  random bytes and a MAC under a per-process key, nonce counts that must go up). Only a PBX trunk's credentials
  (profile "PBX on a private network") answer it: a provider trunk's are the provider account's password, so a digest
  with them is refused and counted as a failure, and a provider's calls come only from its addresses. A listed address
  range is at most a /8 (IPv4) or a /32 (IPv6); the services refuse a wider one and the engine ignores it. Other requests from addresses that
  are not a trunk's are not answered. Without the registrar, REGISTER is refused (403) without a
  challenge, so it counts no failures. Over UDP a
  source address can be forged, so a trunk taken by its address is best on TLS (the default) or TCP.
  The `uri` inside a digest answer is not compared with the request's Request-URI. The UDP socket also answers a STUN
  Binding request (the RFC 5626 keepalive) from any source with a Binding success that carries the source's address (32
  bytes for IPv4, 44 for IPv6). That answer is sent before the SIP stack sees the datagram, so it is given with the
  registrar off, to a blocked source, and outside `sip_request_rate`. No other STUN message is answered.
- **The registrar (V4).** Off unless `sip_registrar_enabled` is true, even with the listeners on. With it, a REGISTER
  and a phone's INVITE answer the same challenge with a desk or soft phone's credentials: a generated username and a
  22-character random password that is shown to its user once and never stored. Only its digest verifiers are kept
  (`SipDeviceVerifier.ha1`, encrypted, per algorithm and realm): SHA-256, and MD5 only for a device of the legacy
  profile, so an MD5 answer for a standard device fails. A failed or replayed answer counts against its source as any
  failed digest; no device or account is locked. Each REGISTER or signed INVITE looks the username up once
  (`get#SipDeviceAuth`), within the per-source rate. Rotation deletes the old verifiers and revocation the device's,
  and either ends its bindings and its calls on every engine. A binding is the flow the phone opened (its transport
  and source address and port); calls to the phone go back over it only, never to the address its Contact names, and
  a stream connection that closes ends its bindings. A device has at most 10 bindings, and expiry is held to
  `sip_registrar_min_expires`..`sip_registrar_max_expires`. A phone's own call is placed as its user through the same
  checks as the browser's (`place#PhoneCall`: outbound permission, caller ID, international and premium, limits,
  emergency first), so a phone's credentials give no more than its user has. Plain RTP from a phone is taken from its
  SDP's address or the address it signals from (its NAT's), on any port until the first packet latches.
  `SipDeviceServices.create#SipDevice` gives any signed-in user a credential for their own account (another user's only
  with `VoiceAdmin`), at most 10 active devices a user, with the registrar on or off. It asks for no recent sign-in. The
  credential holds until `rotate#SipDeviceCredential` or `revoke#SipDevice`, until an administrator's
  `revoke#AccountAccess`, or while the account is disabled (`get#SipDeviceAuth` answers nothing for a disabled user); a
  password change does not end it. `find#SipDevices` returns each binding's source address, port and user agent to the
  device's owner and to a voice administrator, never a verifier.
- **Failed digests block the source.** `authFailureLimit` failures in `authFailureWindowSeconds` from one address
  block it for `authBlockSeconds` (5 in 10 minutes, for an hour, by default): its SIP datagrams and stream messages are
  dropped before a transaction is made, no SIP answer is sent (a STUN Binding request on the UDP port is still
  answered, and its TCP and TLS connections stay open within the connection caps), and the block is kept in
  `VoiceAuthFailure` across
  restarts (the engine counts failures in memory and writes only the block). An IPv6 source counts as its /64. The
  engine tracks at most 50,000 sources; when full, the ones not blocked go first, so rotating addresses does not lift
  the blocks. No account is locked or counted.
  A failure is counted against the request's source address whether or not the answer's nonce is one this server issued.
  A nonce is not bound to an address. The block has no exemption for a trunk's address.
- **What an incoming call reaches.** Only a number in `PhoneNumber` rings anyone, through the call services'
  ringing; the SDP must be audio with a codec in common, and SRTP when the trunk's profile requires it (488
  otherwise).
- **Outbound.** Calls go out from `send#SipControl` (the `rtc-signal` cluster topic, kind `sip`, to that node),
  which only the voice services send, after their checks (caller ID grant, outbound enabled, international and
  premium off by default, concurrency and daily limits, the emergency path). The only other source is an edge node's
  engine, which dials a number's replicated fallback
  forward itself while its local node is away ([Edge node](Attack-Surface-Cluster-Edge.md#edge-node)). With home country
  1, the area codes of
  the plan's other countries (the Caribbean's, Bermuda's) are international. The engine checks the number's class,
  the emergency trunk and the concurrency limit again before an INVITE goes out. A display name or extra header with
  a line break or a quote is cleaned or left out, so it cannot add headers (a `P-Asserted-Identity`) to the INVITE.
- **Forwards to outside numbers (V3).** A call that comes in can make the server dial out: a user's forwarding rules,
  their simultaneous ring to a mobile, and a ring group's fallback number. That is toll fraud's other door (a caller
  rings in to be bridged to a premium or foreign number), so a forward is a phone leg under the same checks as a call
  the user places (`VoiceServices` `forwardCheck` and `dialForward`): the user needs outbound calls and
  `VoiceUserSetting.forwardEnabled` (off by default, set only by a voice admin); international and premium need the
  deployment's and the user's permission; emergency numbers are never a target; the user's limits at once and per day
  count their forwards with the calls they place, and the deployment's calls in a day count every forward; and the
  engine checks the class and its calls at once again before the INVITE. A ring group's fallback number is set by a
  voice admin and checked against the deployment's policy. The caller ID of a forward is the number the call came in
  on, never one the caller supplied. A forward to one of this deployment's numbers is routed inside (no hairpin through
  the trunk), a forward back to the caller's number is skipped, and a call takes at most `call_forward_max_hops`
  forwards, with a user or group already in its plan skipped, so rules cannot loop a call or fan it out without bound.
  Caller lists and rules are the user's own (another user's are "not found"); the rule's target is checked when it is
  saved and again when a call uses it.
- **Transfers and adds to outside numbers (V5).** A user in a call can make the server dial out by transferring the
  call's outside party to a number, or adding a number to the call; a desk phone can ask for a transfer with REFER.
  Each is a call the user places, under the same checks (`CallTransfers.js`, `VoiceServices` `forwardCheck` with
  `transfer` and `dialForward`): outbound calls enabled for the user, international and premium only with the
  deployment's and the user's permission, emergency numbers refused (they are dialed directly), the user's limits at
  once and per day (which count every number they added or transferred to, `CallLeg.dialedByUserId`, so adding numbers
  one by one does not get past them), the deployment's calls in a day, and the engine's own check of the class and its calls at once
  before the INVITE. The limits are counted under one lock (`CallLimitLock`), so calls placed at once cannot pass them
  together. A REFER is taken only from a registered phone's answered leg in a call, as its user; its target is
  a username or a number under those checks, checked before the phone is answered (403 when refused, so it keeps the
  call); an attended REFER (Replaces) is refused. A person called, transferred to or added must pass the apps'
  `call-target` providers (IronStone: someone of the user's organizations), answered as a user that does not exist when
  refused. Only someone in the call (an
  answered leg of theirs) may transfer, add or merge; a call that still rings is not transferred; a stale revision or
  a second open transfer is refused. An external user (`EXTERNAL_USER`) neither transfers nor adds. An end-to-end encrypted call adds no phone legs. An IVR option, a menu's or a
  queue's fallback to a number is set by a voice admin and checked against the deployment's policy when it is saved
  and when a call uses it; a menu loop ends after 20 menus, and a queue's fallback chain after 10.
- **Credentials.** Trunk credentials are `VoiceTrunk.authJson`, encrypted; they answer only the trunk's realm and
  allowed algorithms. TLS to a trunk verifies its certificate against `sip_tls_ca_file` (the system bundle by default),
  for the trunk's host as set up, never an SRV target's name, so a changed SRV record cannot point a trunk at a server
  that does not hold the provider's certificate.
  `VoiceServices.get#EngineConfig` returns each enabled trunk's decrypted `password`, and
  `SipDeviceServices.get#SipDeviceAuth` returns a device's decrypted `ha1` verifiers. Both are `authenticate="false"`
  and are called by the SIP engine in process with authorization disabled; no screen transition names them ([services
  and `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)). `VoiceServices.send#SipControl`,
  `receive#PhoneCall`, `note#PhoneLeg`, `record#AuthFailure`, `register#SipNode`,
  `SipDeviceServices.store#SipRegistration`, `receive#DeviceCall`, `VoicemailServices.check#VoicemailPin`,
  `act#PhoneVoicemail`, `store#Voicemail` and `CallTransferServices.refer#Call` are declared and called the same way.
  `receive#DeviceCall` and `refer#Call` sign in as the device's user by name (`internalLoginUser`) before they place or
  transfer a call.
- **Media.** SRTP (AES_CM_128_HMAC_SHA1_80, SDES keys over TLS) unless the trunk's profile is a private-network PBX;
  a packet that fails its tag or replay window is dropped. RTP goes back where the leg's authenticated packets come
  from. Plain RTP is taken only from the SDP's address (for a registered phone, also the address it signals from; from any
  address when the SDP's address is unspecified, 0.0.0.0) and, once the first packet latched, only from that source;
  anything else is dropped, so a guessed port neither injects audio nor takes the call's audio. RTCP is dropped.
  Until a packet latches, a leg sends its RTP to the address and port the SDP names; no address range (loopback,
  private, link-local, the server's own) is refused there.
  rsipstack logs whole SIP messages (SDES keys, digest answers) at INFO, so the host keeps only its warnings.
- **Floods.** rsipstack has no caps of its own, so the engine's transport inspector holds new requests (outside a
  dialog) to `sip_request_rate` a second per source (`sip_request_burst` at once; 20 and 100 by default, an IPv6
  source as its /64), drops them while `sip_max_transactions` (2,000) run, and closes TCP and TLS connections past
  `sip_max_connections` (500), or past `sip_max_connections_per_source` (100) from one source, so one source cannot
  hold the connections the registrar's phones keep open. What is over is dropped unanswered; requests in a dialog and responses pass, so calls
  going on are not cut. Answers sent before an incoming call's decision expire after 30 seconds (1,000 at most).
  A TLS client has 10 seconds to finish its handshake; at most 128 handshakes run at once and 8 from one source (an IPv6
  /64 is one), and a connection past either is closed. A TCP or TLS connection that sends no byte for 300 seconds is
  closed (rsipstack's default; a CRLF keepalive counts as a byte). No deadline closes a connection that never sends a
  valid request. A SIP message over a stream is at most 65,535 bytes.
- **Limits.** A leg's jitter buffer holds 10 packets; a leg's consumer queue a second of frames; datagrams over 2 KiB
  are cut. The engine registers its node (`SipNode`) every 20 seconds.
- **Settings that name addresses and audio.** `sip_public_address` is the address put in Contact headers and SDP, and
  is required when a SIP host binds every address. `sip_domain` is the digest realm (`sip_public_address`, else `moqui`,
  when empty). `voicemail_prompt_location`, `call_hold_music_location`, `call_recording_prompt_audio` and
  `call_captions_prompt_audio` name resource locations (any scheme) whose files the engine reads and plays to callers.
  They are empty by default.

## XMPP

The XMPP server (`crates/moqui-xmpp`, host `src/xmpp/`, [XMPP](../interfaces/XMPP.md)) runs only in a binary with the
`xmpp` feature (Maximal, Enterprise, Debug) and only when `xmpp_domain` is set. Each listener is off unless set:
`xmpp_port` (STARTTLS, required before anything else) on `xmpp_host`, `xmpp_tls_port` (direct TLS) on
`xmpp_tls_host`, and `/xmpp-websocket` with `xmpp_websocket_enabled`. Local users only: no server-to-server
(`xmpp_s2s_port` is declared in the default conf and no code reads it). `xmpp_host` defaults to `0.0.0.0`, so `xmpp_port`
listens on every IPv4 address once it is set. `xmpp_tls_host` is empty by default and then takes `xmpp_host`; an empty
host is `0.0.0.0`. A direct TLS handshake and a STARTTLS handshake each have 10 seconds. A connection with nothing read
for 600 seconds is closed, and a write the client does not take for 60 seconds ends it.

- **Sign-in** takes app passwords only, never the account password (which would count toward the account's lockout
  and cannot carry a second factor): SCRAM-SHA-256 and SCRAM-SHA-256-PLUS against the verifiers stored when the
  app password was made (salted, 10,000 iterations), with tls-exporter channel binding where the connection has its
  own TLS session and a downgrade (`y` where the server binds) refused; PLAIN over TLS only; FAST tokens (XEP-0484)
  bound to the client's user-agent id, kept encrypted, expiring (`xmpp_fast_token_days`), and without channel binding
  refused unless their count grows; a token goes with the app password it was issued under. An unknown user gets a salt
  derived from the name and `xmpp_scram_decoy_key` (is-secret, generated when unset), so the exchange does not tell it
  apart, and its salt stays the same across restarts and nodes. A disabled account cannot sign in.
  A session checks every minute (`check#Credential`) that its app password is not revoked and that the
  account is not disabled with no `disabledDateTime`, and ends when not. An account past its `terminateDate`, or locked
  after failed logins, keeps its session; its service calls are refused by `internal_login_user`.
  A SCRAM or FAST sign-in is decided in `moqui-xmpp` from `get#Account` (`canLogin`: `disabled` is not `Y`, or
  `disabledDateTime` is past) and the stored verifiers or token. It writes no `UserLoginHistory` row and does not update
  `UserAppPassword.lastUsedDate`; the sign-in is a log line at INFO, a failure a log line at WARN with the address and
  no username. A PLAIN sign-in goes through `login_app_password`, which writes the history row and `lastUsedDate`. After
  the sign-in every service call logs the user in again with `internal_login_user`, which refuses a disabled or
  terminated account and one whose password must change. No client address is given to that login, so `ipAllowed` of the
  account and its groups is not checked for XMPP. The sign-in services `get#Account`, `get#ScramCredentials`,
  `check#PlainPassword`, `check#Credential` and the four FAST token services are `authenticate="false"` and run with
  authorization disabled (`Caller::Internal`). App passwords are made in BasicComms Settings, Chat apps ([The BasicComms
  app](#the-basiccomms-app)), and are not bound to XMPP.
- **Failures** are counted per source address (moqui-lib `source_guard.rs`, as the SIP engine counts failed digests):
  5 in 10 minutes block the address for an hour, a /64 for IPv6; the account is never locked. The counter is in memory
  in each node's process (`Server.guard`): it is not shared between nodes, not kept across a restart, and not the web sign-in's.
  Over WebSocket the
  address is the client a trusted proxy names. At most 20 connections from one address at once and 10,000 in all
  (TCP and WebSocket); a connection not signed in within a minute is closed at the next 30 second tick, so it lasts
  60 to 90 seconds; a stanza or stream header over 256 KiB,
  or nested over 32 deep, ends the stream; a session whose pushes back up (512) is closed. Stream management keeps at
  most 1,000 unacknowledged stanzas and 10 detached sessions a user; PEP 200 nodes a user and 256 items a node;
  uploads 60 slots a user an hour.
  A PEP item is at most 262,144 characters. `block#Jids` takes 500 JIDs a call with no total, and `enable#Push` adds a
  row for each `pushJid` and `node` with no total. There is no per-session stanza rate limit; a session's stanzas are
  worked one at a time.
- **Authorization** is the user's: most stanzas become a service call as the signed-in user, through
  `XmppClientServices` (granted ALWAYS to `ALL_USERS`): those services check the room (`get#RoomAccess`) or work on the
  user's own rows, except `get#Users` (any username, for a caller who is not external; 200 ids and 50 names a call) and
  `get#PepItems` (another user's node, by its access model). `ChatServices.disconnect#UserStatus` and `get#UserStatuses`
  are called as the user and are not in the group; both are `authenticate="false"`. `get#PushTargets` is called with
  authorization disabled. An external user (`EXTERNAL_USER`) learns only the usernames of the people of their own rooms.
- **Stanzas routed with no service call.** An `iq` to another local user's full JID, a `message` to a local user that
  carries no body, OMEMO payload, correction, retraction, reaction, displayed marker or chat state, and a directed
  `presence` to a local user are handed as they are to that user's sessions on the same node (`route_to`). The server
  sets `from` to the sender's full JID. No room or other relation between the two users is checked. The stanza enters
  the receiving session's queue (the 512 above); that session then drops it when the sender's bare JID is in its user's
  block list. A message with a body to a local user's JID opens or reuses the direct room with that user
  (`XmppServices.get#DirectRoom`, which calls `ChatServices.create#Room` with `CrtDirect`) and is stored by
  `send#Message`.
- **XML** is parsed by rxml, which refuses DTDs, entity declarations and processing instructions.
- **End-to-end encrypted Moqui rooms** (C7) are not served; OMEMO messages are kept as they came, unreadable to Moqui,
  never in a bridged room, with no text in snippets, search, notifications or agents. PEP items (OMEMO bundles hold
  public keys) are read by access model: `open` any local user who is not external (an `EXTERNAL_USER` reads it only
  when they share a room
  with the owner), `presence` the people the owner shares a room with,
  `whitelist` the owner.
- **TURN for XMPP calls** (XEP-0215): off unless `xmpp_turn_enabled` is true; while off, external services are not
  advertised and a request gets an empty list. On, a credential keyed to the user (`xmpp.<userId>`) may relay to public
  unicast addresses only, never loopback, private, link-local, shared or documentation ranges ([TURN](#turn)); it is a
  relay to the internet within the TURN quotas, as a public TURN service's is. An external user (`EXTERNAL_USER`) never
  gets one, and a user at most `xmpp_turn_per_hour` (30) an hour (counted on the distributed cache
  `moqui.xmpp.turn.issued`); a refusal is the same empty list.
- **Push** (XEP-0357) registrations are stored, with their app server's secret encrypted; a notification has the message
  count only, and goes nowhere until federation (C3b).
- **Upload files** (XEP-0363; the route's row is in
  [Attack-Surface-Routes.md](Attack-Surface-Routes.md#host-routes-and-servlets)). An upload is stored at
  `dbresource://xmpp-upload/<xmppUploadId>/<name>` (`xmpp_upload_root` names another root). The token is 32 random
  characters for a slot, and for a chat file the first 40 letters and digits of `singleUseCodeHash(xmpp_upload_secret +
  ':' + chatAttachmentId)`. The route takes any method: `OPTIONS` is 204, `PUT` writes, and every other method is
  answered as a read. A file that was put is served at its URL with no expiry: the read checks only the token's row,
  `completedDate` and that the location has bytes, and loads the whole file into memory on a request worker. The content
  type is the one the uploader gave the slot. A type that starts with `image/`, `audio/` or `video/` is served inline,
  except the exact text `image/svg+xml` (a type with a parameter or another case, such as `image/svg+xml;
  charset=utf-8`, is inline). Every answer has `nosniff` and `Content-Security-Policy: sandbox; default-src 'none'`. A
  user's slots are limited to 60 an hour and ten times `xmpp_upload_max_bytes` an hour; there is no total per user and
  no drive quota. `attach#Upload` makes a `ChatAttachment` of the room on the same stored file, with the uploader's file
  name and content type. It refuses an upload that is not the caller's own completed one, and the session then logs the
  whole URL, token included, at WARN. `clean_XmppUploads_daily` (`XmppServices.clean#Uploads`) removes only slots never
  put within an hour, and expired FAST tokens. `ChatServices.clear#MessageContent` deletes a stored file only under the
  chat root, so the file of a deleted message under the upload root stays, and its URL goes on serving it.

## Voicemail

Voicemail ([Voicemail](../interfaces/Voicemail.md)) opens no port: it runs on the SIP engine above, for calls to a
user's number that nobody answers, and on the access number (`PnrVoicemail`) when an administrator sets one.

- **The PIN over the phone.** A caller who presses `*` in a greeting, or calls the access number, can enter a box
  number and a PIN. `check#VoicemailPin` (internal) checks it against the box's argon2 hash and limits failures per
  box (5 in 15 minutes, then 15 minutes refused from any caller) and per source (the caller's number, 10 in an hour,
  then an hour). The source is a caller ID the caller can fake, so the box's limit is the one that holds: up to 480
  guesses a day (5 every 15 minutes), which tries every four-digit PIN in three weeks and every six-digit one in under
  six years, so a new PIN
  has at least `voicemail_pin_min_digits` (6 by default) digits. A box that does not exist, has no PIN, or is blocked is refused in
  the same words after the same hash work and is counted the same way, so the answers say nothing of which boxes
  exist. Three refusals end the call, and a refusal whose answer says `blocked`
  ends it at once. Failures for a number with a box are counted on the box, failures for digits with no box on those
  digits. The PIN is never logged, and weak ones (one digit repeated, a run) are refused
  when set. Accepted, the caller hears only that box's new messages, and its keys act only on them (`act#PhoneVoicemail`
  checks the box).
- **The audio route.** Messages and greetings are under `dbresource://moqui/call/voicemail/`, which no route serves.
  The audio is read through `get#VoicemailAudio` and `get#VoicemailGreeting`, from the Calls `Voicemail`,
  Settings `Phone` and System › Voice `RingGroups` screens' transitions: a user's box is its owner's, a ring group's
  box is its members' (messages) and a voice administrator's (settings and greeting); anyone else gets "not found" (an
  empty JSON answer),
  the same as for an id that does not exist. The email notice attaches the audio when the box's `attachAudio` is on.
  An edge node's engine reads greetings, prompts and, after an accepted PIN, a box's messages through `get#CallAudio`
  and `get#PhoneVoicemailAudio`. The engine writes only through `store#VoicemailFile` (internal), which
  takes a spool path only under `runtime/tmp/voicemail` and a location only under the voicemail root.
- **Uploads.** A greeting upload is at most 10 MB and is checked as WAV or Ogg Opus by its first 96 bytes. In a binary
  with the `sip` feature the server decodes and
  re-encodes it (`convert#GreetingAudio`, internal, which reads and writes only under the greetings' root or the
  prompts' root, both locations under the same one)
  and keeps a minute. In a binary without it the upload is stored as it came, under the same root, with its length
  unchecked.
- **Mail out.** The email notice carries only a link unless the owner turns on the audio or the transcript in it; the
  screen warns that mail cannot be recalled. Transcription sends the audio to the speech provider only when
  `voicemail_transcribe` is on.
- **Limits.** A message is at most `voicemail_max_seconds` (120; at most 600), spooled to disk as it is recorded; one
  under a second is dropped.
  `voicemail_prompt_location` names a folder (any resource location) of prompts the engine plays; it is empty by
  default.

## Chat bridges

The bridge layer ([Chat bridges](../interfaces/Chat-Bridges.md), C1) opens no port and routes no endpoint: a network
adapter (C4 on) brings its own transport, and C1 ships none.

- **Input.** No screen transition names `ChatBridgeServices.ingest#Event`: services and code in the server call it (an
  adapter, after its
  own checks of a webhook's signature or a gateway's session; [services and
  `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)). It authenticates the bridge by its inbound token
  (`check#BridgeToken`: the SHA-256 against `ChatBridge.inboundTokenHash`, in constant time; a bridge with no hash takes
  none) and refuses the whole batch, storing nothing, for an unknown or disabled bridge, a wrong token, a malformed
  transaction id, more than 200 items or 1 MB, or an item that names a local user. An item posts only as an external
  actor (no login, no `UserAccount`) into a room an administrator mapped to that bridge (`noMapping` otherwise), as a
  member a moderator has not taken out, edits and deletes only its own messages, and carries plain text only (escaped to
  HTML; files are refused). A puppet link to a local user is provenance, never authority. Its messages fetch no link
  previews, reach a room's agents only under the room's `agentsReadExternal`, and never start the framework's agents.
- **Output.** A room's messages go only to the networks an administrator mapped it to; a link to an external room grants
  no rights. Delivery rows keep no message text.
- **Administration.** `find#Bridges`, `store#Bridge`, `store#RoomBridge` and `end#RoomBridge` need `ChatBridgeAdmin`.
  `ChatBridge.configJson` is an encrypted field and never returned; the inbound token is kept only as its hash.
  `retry#Delivery`: the message's sender, a moderator of the room, or `ChatBridgeAdmin`; anyone else is told "not found"
  or "not allowed" with the same parameters.
  `ChatBridgeAdmin` is granted to `ADMIN` by `ChatBridgeSetupData.xml`. The Chat screen's `bridgeChoices`,
  `storeRoomBridge` and `endRoomBridge` transitions call `find#Bridges`, `store#RoomBridge` and `end#RoomBridge` with
  authorization disabled; each service checks the permission itself. System › ChatBridges has two transitions,
  `setDisabled` (`store#Bridge`) and `retryDelivery`; no screen that edits `configJson` was found. The SMS adapter
  (`crates/moqui-channels/src/telnyx.rs`) reads from the bridge's `configJson`: `apiKey` (sent as `Authorization:
  Bearer`), `apiBaseUrl` (`https://api.telnyx.com` by default; an `http://` address is accepted), `publicKey`,
  `webhookToleranceSeconds`, `maxInboundMediaBytes` (at most the adapter's 20 MiB cap) and `allowInternalAddresses`.
  Files a text names are fetched from public addresses only (3 redirects, each checked) unless `allowInternalAddresses`
  is set, which lifts the address guard for the API calls and the media fetch of that bridge ([the internal-address
  guard](Attack-Surface-Outbound.md#the-internal-address-guard)).
- **Internal services.** `send#Message`, `edit#Message`, `react#Message` and `redact#Message` take `senderActorId` only
  with authorization disabled. `get#LocalActors`, `get#ActorNames`, `get#RoomBridges`, `check#BridgeSupport`,
  `get#BridgeCapabilities`, `wake#BridgeDelivery`, `run#BridgeDeliveries` and the
  `ChatBridgeServices` internals are written for callers in the server. The attribute that says so is not enforced, and
  their callers were not traced.

## Conf-level shell providers

A component's MoquiConf.xml may register a provider of the hooks the framework asks for whoever acts (`screen-facade/shell-providers`; only `sms-consent`, `CONF_PROVIDER_HOOKS`; any other element is logged and left out). call#Providers asks it after the apps' providers for every caller, the system with no user included, with authorization disabled, since the deployment's conf names it: such a provider answers only what its hook asks; conf providers are always asked beside an app's provider of the same name, a "no" from any provider wins, and a provider that fails refuses the text (Mantle's check#SmsConsent reads consent and suppression rows and answers yes or no, nothing else). A promotional text counts against its number's day of promotional texts (VoicePolicy smsMaxPromotionalPerNumber, 1000), and one with no user not against the system's own budget, so a campaign cannot use up sign-in codes. Marketing's texts (the ledger's SMS channel, MdcSms: each attempt is handed to the framework's `org.moqui.SmsServices.send#SmsMessage`; the consent answer and STOP handling are `mantle.party.MessageSmsServices` check#SmsConsent, which mr-mantle's `MoquiConf.xml` names as the `sms-consent` provider, and note#SmsKeyword; no screen transition names either) are sent as the system from an SMS sending profile's number, which must be one of the organization's telecom numbers that is one of ours with a messaging bridge (MESSAGE_SENDING_CONFIGURE sets it); each text is checked for `PcpSms` consent on the send's topic and the SMS suppression before the call, and again by the framework's sms-consent hook (yes only for a party holding the number with PcpSms granted with an organization that owns the sending number and no suppression there); a STOP the framework keeps becomes a PcpSms withdrawal and a suppression (an EECA on SmsOptOut, in its own transaction), and a START ends neither; a lost answer is uncertain and never resent; no link, token or tracking in a text. PASS Email preferences may leave a text message topic, never join one.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- Chat bridges and SMS, Web Push and FCM, and call recording and captions were sampled only. `chat_bridge.rs` (the delivery worker), `chat_bridge_edge.rs`, `crates/moqui-channels/src/webhook.rs`, `fcm.rs`, `crates/moqui-rtc/src/sfu/`, `record/`, `audio/` and `tap.rs`, and `crates/moqui-host/src/rtc/mod.rs`, `media.rs`, `recorder.rs` and `captions.rs` were not read. What the sections above say of those areas was not rechecked line by line.
- BasicComms: the `Settings` screens `Index`, `Calls`, `Chat`, `Devices`, `Identities`, `Rules`, `OutOfOffice` and `TextBlocks`, the `Home` and `Panel` screens, the `*Tui.xml` screens, `template/FilePickerTransitions.xml`, `ComposeServices.xml`, `CommsHomeServices.xml` and `FilePickerServices.xml` were not read. `ChatScreenServices.xml` and the `Calls`, `Inbox` and `Settings` screens were read only for their transition names, authorization calls and file responses.
- `Chat.qvue`, `Call.qvue` and `CallGuest.qvue` were not read: what the clients do with relayed call frames, guest names and a forwarded message's body (`v-html`) was not checked.
- Whether an account that holds `EXTERNAL_USER` can also hold `BASIC_COMMS_USERS`, or reach an app's Comms root, was not traced.
- Whether an app narrows the people search, `create#Room` or `add#Member` to a user's own organizations some other way was not traced.
- Whether the `PublicOnly` resolver is used when the process has an HTTP proxy in its environment was not traced (`crates/moqui-lib/src/http.rs`; it matters for link previews, Web Push and inbound bridge media).
- Which user and authorization the asynchronous call of `deliver#AgentMessage` runs with was not traced.
- `ChatScreenServices.lookup#Records`, `createFrom` and `fileOnRecord` hand the request's `recordEntityName` and `recordId` to the apps' providers; the providers were not read.
- Whether the `dbresource://` store or the screen layer refuses a large chat file before `upload#Attachment` was not traced.
- Who reads a stored caption (`CallTranscript`, with `call_captions_store`) was not traced.
- Whether a guest who waits in the lobby longer than `call_ring_seconds` can still be admitted was not traced (`expire#Calls` closes an offered leg after that time).
- Whether `web-protection` matches `/call/*/joinGuest` against the token route's path as written was not read (the guard's matcher), and whether its `max-per-client` connection cap applies to `/callws` was not traced.
- `docs/interfaces/Chat-Keys.md`, `WebRTC.md` and `SMS.md` were not compared with the code statement by statement.
- TURN: the `turn-server-proto` crate was not read (whether it compares MESSAGE-INTEGRITY in constant time, limits its user table, or answers an unauthenticated request with a reply larger than the request). The rest of `turn_probe.rs` and the TURN tests were not read.
- TURN: which IPv6 ranges `is_public` leaves out, and whether a relay is ever bound to an IPv6 address, was not settled.
- XMPP: that `rxml` refuses DTDs, entity declarations and processing instructions in every position was taken from a comment and one test in `crates/moqui-xmpp/src/framing.rs`; the crate was not read. `jids.rs`, `features.rs`, `ns.rs` and `session/xml.rs` were not read.
- XMPP: whether the address given to a session over an edge tunnel (`serve_tunneled`, `crates/moqui-host/src/xmpp/mod.rs`) can be chosen by anyone but the paired edge node was not traced (`edge_link` was not read). How the client address of `/xmpp-websocket` and `/xmpp-upload` is computed, and the web protection of `/xmpp-upload/`, were not read.
- XMPP: whether a chat push can name a user who may no longer read the room was not traced; the session trusts the push and checks only its cached room.
- XMPP: `XmppServices.get#IceServers` counts the hour's credentials with a cache `get` then `put`. Whether two requests at once (several sessions of one user) both pass the limit was not checked.
- XMPP: whether the timing of the SCRAM challenge tells an account from an unknown name was not measured (an account takes one more service call).
- XMPP: `docs/interfaces/XMPP.md` says Chat apps shows whether each app password works with XMPP apps; the screen lists the name, made and last used only. Which is intended was not checked. What `check_plain_password` does for a row made before the SCRAM fields existed was not looked for.
- SIP: whether rsipstack sends a dialog's later requests (ACK, BYE, re-INVITE) to the Contact a phone or a trunk gave or back over the flow was not read (the dialog layer).
- SIP: `registrar.rs`, `sdp.rs` (parser limits, the `c=` address), `srtp.rs` (the replay window), `dial.rs` (the engine's class check), `resolve.rs` (SRV targets as trunk addresses), `race.rs`, `dtmf.rs`, and the host's `leg.rs`, `ivr.rs`, `edge.rs`, `owners.rs`, `trunk_lock.rs` and `prompt.rs` were not read; what this file says of them was not rechecked beyond the constants. Whether a leg sends RTP before it is answered was not read.
- SIP: whether rsipstack drops a datagram whose Via does not match its source before a digest failure is counted was not read.
- SIP: `secret_parameter` (`crates/moqui-core/src/context/identity.rs`) does not list `pin`, `ha1` or `response`. `setPin` posts `pin`. Where request or service parameters are recorded was not traced.
- SIP: `VoicemailServices.expire#VoicemailTrash` is `authenticate="anonymous-all"`, takes no input and deletes only trash past `voicemail_trash_days`. Which callers name it was not traced.
- SIP: `receive#DeviceCall` and `refer#Call` call `internalLoginUser` inside a request-pool job of the engine. Whether each job gets a fresh execution context was not traced.
- SIP: the Opus, G.722 and WAV decoders take untrusted input (RTP from phones and trunks, uploaded greetings and prompts). Whether `opus-rs` is Rust or a C binding, and its limits, were not read.
- SIP: `CallHandling.js` `evaluate` builds any user's call plan with no ownership check. No screen or script under `runtime/components/base`, `addon` or `dev`, or in `crates`, names its service by grep; other callers were not traced. The bodies of `CallHandling.js` (plans and routing), `CallQueues.js`, `CallTransfers.js` and `Ivr.js` beyond their guards were not read.
- Voicemail: a box's `notifyEmail` is any address the box's user types, with no confirmation; with `attachAudio` each message's audio is mailed there. Whether the `EmailMessage` rows made this way are rate-limited or audited was not read.
- Voicemail: whether the phone-access answers tell an existing box from a number with none after a block was not settled.
- SIP: every DTMF digit of a phone leg is a `note#PhoneLeg` call and a debug log line with the digit. Whether debug logging is on in production configurations was not checked.
