# Attack surface: calls, chat and messaging

The BasicComms app, chat, calls and their media, call links and guests, captions, recording, TURN, SIP, XMPP, voicemail, chat bridges and texts. The sockets and webhooks (`/notws`, `/callws`, `/channels`, `/xmpp-websocket`, `/xmpp-upload`) and the chat key transitions are rows in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). The mail client's screens are in [Attack-Surface-Mail.md](Attack-Surface-Mail.md), and the requests the server makes are in [Attack-Surface-Outbound.md](Attack-Surface-Outbound.md). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## The BasicComms app

BasicComms (`runtime/components/base/basic-comms`) is the framework's Comms app. Its `MoquiConf.xml` mounts `BasicComms` (menu title Comms) under `webroot/screen/webroot/apps.xml`, mounts `Voice` and `ChatBridges` under `component://tools/screen/System.xml`, and adds the token route `/call/<token>` ([Call links](#call-links)). It opens no port.

- **Who may open it.** The artifact group `BASIC_COMMS` holds `component://basic-comms/screen/BasicComms.xml` with `inheritAuthz="Y"` (`data/BasicCommsSetupData.xml`). `BASIC_COMMS_USERS` is allowed. `BASIC_COMMS_ADMIN` and `ADMIN` are always allowed. `ALL_USERS` has no grant. `BASIC_COMMS_ADMIN` is named only in that data file: no service or screen tests it. An app that mounts these screens under its own root authorizes them by that root ([Attack-Surface-Apps-BOSS.md](Attack-Surface-Apps-BOSS.md)). Nothing stops an account from holding both `EXTERNAL_USER` and `BASIC_COMMS_USERS`: `EXTERNAL_USER` is a permission a group carries (`SecurityTypeData.xml`), and `BasicCommsSetupData.xml` has no deny for external users. mr-boss denies its own Comms root (`BOSS_COMMS`, `AUTHZT_DENY`) to its client group `BOSS_PROJECT_CLIENT`, which does not cover the framework's `/apps/BasicComms` mount. The bridge and text services never test `EXTERNAL_USER`: such a user texts only if a voice administrator granted them a number.
- **Screens by family.** `Inbox` (`Mail`, `Compose`, `MessageDetail`) and `Settings/MailAccounts` are the mail client ([Attack-Surface-Mail.md](Attack-Surface-Mail.md#mail-client)). `Chat` and `Panel/Chat` are chat ([Chat](#chat)). `Calls` with `Call`, `History`, `Recordings`, `Voicemail`, `Dialer` and `Queues`, and `Panel/Dialer`, are calls and the phone ([Call services](#call-services), [Call recording](#call-recording), [Voicemail](#voicemail), [SIP](#sip)). `Settings` holds `ChatApps`, `Phone`, `PhoneDevices` and `CallHandling` (below), beside `Index`, `Calls`, `Chat`, `Devices`, `Identities`, `Rules`, `OutOfOffice` and `TextBlocks`. `Home` and `Panel` are the other two subscreens.
- **Transitions.** The transitions answer JSON. The `Chat` and `Calls` transitions that read are `read-only`, most with `method="get"`. The ones that change something (`send`, `upload`, `createRoom`, `addMembers`, `startCall`, `createCallLink` and the rest) declare no `method` ([CSRF](Attack-Surface-Web.md#csrf)).
- **Services.** The transitions call `org.moqui.comms.*` (the component's screen services), `org.moqui.chat.ChatServices`, `org.moqui.chat.ChatBridgeServices`, `org.moqui.chat.ChatKeyServices` and `org.moqui.call.*`. The phone screens and the Chat screen's bridge transitions call their service with authorization disabled, each in its own transaction, and the service decides by the signed-in user. No REST resource names those services: the one `*.rest.xml` file in `runtime/components/base` (`tools/service/moqui.rest.xml`) has no `org.moqui.chat`, `org.moqui.call` or `org.moqui.comms` service. They are reached only where a screen transition, a socket (`/notws`, `/callws`), an XMPP session, a job or server code names them. A service's `allow-remote` attribute is not the control ([services and `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)).
- **The people search.** `org.moqui.comms.CommsServices.search#Users` answers the user id and the full name (or username) of every enabled account whose username or name has the term. It reads `UserAccount` with `disable-authz="true"`, at most 50 a call, and an empty term matches every account. It leaves out usernames that start with `_` and every `EXTERNAL_USER` account, and the users the apps' `chat-target` providers refuse (`ChatServices.get#RefusedUsers`, asked with `chat#Target`'s parameters; a provider that fails refuses them all); an external caller finds only the people of their own rooms. With `purpose` `call` (the Calls pickers `Calls.xml`, `Calls/History.xml`, and `VoiceTransitions.xml`) it asks the `call-target` providers instead (plans/Security-Review.md SEC-COM-01, fixed). A caller with authorization disabled is refused no one. mr-boss registers `ComposeServices.get#CallTarget` as its `chat-target`. It is behind the `searchUsers` transitions of `Chat`, `Calls`, `Calls/History` and `Settings/MailAccounts`, and of `template/VoiceTransitions.xml`. IronStone's own people search, `ironstone.shared.ChatterServices.search#Users` (Chatter mentions and followers, the share, member, reviewer and assignee drop-downs, the internal To), asks the same `chat-target` hook with the caller's authorization (it does find a project's client contact), and its `get#UserIdsParties` drops a refused id from the mentions and users a note, message, reply, edit or follower add names, as one that does not exist.
- **The dialer.** `Calls/Dialer` and `Panel/Dialer` include `dialerData`, `dialSearch` and `dialNumber` (`template/DialerTransitions.xml`), which reach `CallHandlingServices.get#DialerData`, `search#Dialable` and `VoiceServices.place#PhoneCall` (its checks are under [SIP](#sip)). `search#Dialable` asks the `recipient-source` providers for people with phone numbers. When no provider answers, it searches `UserAccount` by username and full name (a contains match on a term of 2 to 100 characters) and returns each enabled user that one of the deployment's numbers rings, with those numbers, at most 25. That fallback checks no group or permission and does not test `EXTERNAL_USER`.
- **Phone screens.** `Calls/Voicemail` (`voicemail`, `audio`, `mark`, `delete`, `restore`, `retryTranscript`, `callBack`) reaches `VoicemailServices` by `voicemailId`: a message of a box the user does not read is `notFound`. `Settings/Phone` (`settings`, `saveSettings`, `setPin`, `uploadGreeting`, `deleteGreeting`, `greeting`) acts on the user's own box only. `setPin` declares no `method`: the client posts the PIN in a body, and a GET with `pin` in the query works too. `secret_parameter` (`crates/moqui-core/src/context/identity.rs`), the one rule for what `ec.web.getRequestDetails`, the HTTP log's query and telemetry leave out, lists `password`, `secret`, `token`, `credential` and a few names, not `pin`, `ha1` or `response`, so a PIN sent in a query is kept in the HTTP log. `ha1` and `response` are parameters of in-server services only. `Settings/PhoneDevices` (`devices`, `add`, `newPassword`, `remove`) reaches `SipDeviceServices`: a device that is not the user's is `notFound` unless the caller holds `VoiceAdmin`. `Settings/CallHandling` (`handling`, `change`) maps `op` to one of seven `CallHandlingServices` services through a fixed table; a rule, list or schedule of another user is `notFound`. `Calls/Queues` (`setStatus`) changes only the caller's own agent status.
- **Uploads and downloads.** Each download transition asks a service for a location by id, the service answers it only to a caller it allows, and `ec.web.sendResourceResponse` sends the file (`crates/moqui-core/src/web/mod.rs` `send_resource_response`). The content type comes from the stored file name. A user file is `private, no-store`. A file whose name has an active type (HTML, SVG, XML, JavaScript) is sent as an attachment even when inline is asked.
  - Chat files: `Chat/upload` and `Chat/attachment` ([Chat](#chat), its Files item).
  - Files sent in chunks: `Chat` and `Inbox/Compose` include `uploadChunk` (POST) and `uploadStatus` (GET) from `template/UploadTransitions.xml`, which call `org.moqui.edge.ClientSyncServices.upload#Chunk` and `get#UploadStatus`. An upload is found by the current user's id and `uploadId`, so a user reaches only their own. The limits are in [Chunked uploads](Attack-Surface-Files.md#chunked-uploads). An upload with no chunk for `client-sync@upload-expire-hours` (24) is removed by `clean#ClientUploads`.
  - Recordings: `Calls/Recordings` `recordingFile` ([Call recording](#call-recording)).
  - Voicemail audio and greetings: `Calls/Voicemail` `audio`, `Settings/Phone` `greeting` and `uploadGreeting` ([Voicemail](#voicemail)).
  - Prompts, hold music and ring group greetings: System › Voice, below.
  - Mail attachments: [Attack-Surface-Mail.md](Attack-Surface-Mail.md#message-display-and-files).
- **My devices' names.** `Settings/Devices` `peerNames` (Comms-Messaging phase 3) calls `ChatKeyServices.get#PeerNames`
  with up to 50 user ids: it answers the full name (or username) of those the caller's devices reach (the caller, and
  those sharing a room or a live call, ChatKeys.js `reachable`, the same reach as key queries and to-device sends), and
  leaves the others out with no error, so it names no one a verification could not come from.
- **The encryption default.** `Chat` `encryptionDefault` (Comms-Messaging phase 12) calls
  `ChatKeyServices.get#EncryptionDefault` with up to 50 user ids: one bit, whether a new room of the caller and them is
  end-to-end encrypted by default (every one has a crypto device in use). It names no device, count or date; a user an
  app's chat-target provider (or an external caller's reach) refuses, unknown or disabled counts as one with no device,
  after the same device query, so the time of the answer says no more; past 600 people an hour a user it answers false
  with `rateLimited`, counted atomically (a cache slot per person with `putIfAbsent`; per node unless a distributed cache
  factory is configured). A device Chat made by itself counts only once used on a later day or verified. `createDirect`
  and `createRoom` apply the same rule through `ChatScreenServices.create#ChatRoom` (`check#DevicesReady`, internal) when the caller gives no `encrypted`; the
  room made carries its label, so a default the server picked shows.
- **Chat apps and app passwords.** `Settings/ChatApps` has the transitions `createXmppPassword` and `revokeAppPassword`, which call `org.moqui.impl.UserServices.create#UserAppPassword` and `revoke#UserAppPassword` for the session's user. `create#UserAppPassword` takes a `name` and a `scope` (`xmpp` here) and makes 20 characters from a 31-symbol alphabet. It stores the SHA-256 login key hash and the SCRAM-SHA-256 salt, iteration count, stored keys and server keys (`UserAppPassword.scram*`, fields not encrypted). It needs a sign-in in the last five minutes (`require#RecentAuth`; a refused form asks the user to confirm and sends again) and sets no expiry. The screen keeps the new password in the session attribute `newXmppPassword` until the screen is next rendered. The password is bound to XMPP: each protocol's login takes only a row made for it, or one made before scopes, which each takes (`login_app_password`, SEC-COM-21; [login, lockout and passwords](Attack-Surface-Web.md#login-lockout-and-passwords)). mr-boss `Commons/Settings/Sync.xml` calls the same services.
- **System › Voice.** `System/Voice` has eleven subscreens: `Status`, `Trunks`, `Numbers`, `CallerIds`, `Fraud`, `Emergency`, `Phones`, `RingGroups`, `Schedules`, `IvrMenus` and `Queues`. The screens are authorized by the System app's group (`SYSTEM_APP`, `ADMIN` always; [Attack-Surface-Admin.md](Attack-Surface-Admin.md)). Every service behind them refuses a caller without the `VoiceAdmin` permission with `notAllowed`; `VoiceSetupData.xml` gives the permission to `ADMIN`. The families are `VoiceServices` (trunks, phone numbers, caller ID grants, emergency locations, the voice policy, user settings, auth blocks, the status), `IvrServices` (menus, options, prompts, call target options), `CallQueueServices` (queues, their agents and audio) and `CallHandlingServices` (ring groups, their members, the shared schedules). A trunk's password is written only: `find#VoiceTrunks` answers `hasPassword`. Three transitions take an audio upload: `IvrMenus/uploadPrompt`, `Queues/uploadAudio` and `RingGroups/uploadGreeting`. Each file is at most 10 MB, is checked as WAV or Ogg Opus by its first 96 bytes, and is stored under `dbresource://moqui/call/prompt/menu/<ivrMenuId>/`, `dbresource://moqui/call/prompt/queue/<callQueueId>/` or `dbresource://moqui/call/voicemail/greeting/<voicemailBoxId>/`. No screen transition serves a prompt or hold music file (no screen names `get#IvrPromptAudio`). `RingGroups/greeting` serves a ring group's greeting to a voice administrator. The shared transitions `searchUsers` and `searchTargets` (`template/VoiceTransitions.xml`) read `UserAccount` with authorization disabled and check no permission of their own.
- **System › ChatBridges.** `System/ChatBridges` is authorized by the System app's group too. It has three transitions, `setDisabled` (`ChatBridgeServices.store#Bridge`, with `chatBridgeId` and `disabled` only), `retryDelivery` and `replayRefused` (`replay#RefusedWebhooks`). The services check `ChatBridgeAdmin` themselves ([Chat bridges](#chat-bridges)).

## Chat

`org.moqui.chat.ChatServices` (`runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml`) holds rooms, members, messages, files, polls, read markers and presence. It is reached from the BasicComms `Chat` screen, from the root chat transitions, the `/ChatKeys` transitions and `/notws` ([Attack-Surface-Routes.md](Attack-Surface-Routes.md)), from XMPP sessions ([XMPP](#xmpp)) and from bridges ([Chat bridges](#chat-bridges)).

- **Who reaches a room.** Entity filters do not apply to `moqui.chat.*`, so each service that takes a `chatRoomId` asks `get#RoomAccess` for the current user. It answers yes for a current member (a direct room's members always), for any user when the room's visibility is `CrvUsers`, and for a record room when the theme's `STRT_RECORD_VISIBLE_SERVICE` answers yes for the user. A room the user cannot reach is "not found". An external user (`EXTERNAL_USER`) reaches no room bound to a record and no public room they are not a member of.
- **Roles.** Members post, react, pin, and edit or delete their own messages. Moderators and owners delete others' messages and change members. Owners change visibility and archive. The user who makes a room is its owner.
- **Members.** A member adds an enabled account to a room that is not direct (`add#Member`), and a user starts a direct room with one (`create#Room`), only when the apps' `chat-target` providers do not refuse it; a refused user is answered "User X not found", as one that does not exist, and an external user reaches only the people of their own rooms. A public room's mention of a non-member asks the same hook. A provider runs as its app by the user's own grant of the app, with that grant's entity filters, never by an inheritable grant of its caller: `add#Member` and `send#Message` are XMPP client services that `ALL_USERS` may always run (`XmppSecurityData.xml`), and under that grant mr-boss's `get#CallTarget` read every organization's people and answered no one as refused (DC-UI-043, fixed in `screen/shell.rs` `run_in_app`). The account added is not asked. Calls ask the apps' `call-target` providers ([SIP](#sip)).
- **Acting as another user.** A parameter that names another user or actor (`get#RoomAccess.userId`, `send#Message.senderUserId` and `senderActorId`, `signal#Typing.userId`, `set#UserStatus.userId`, `get#UnreadCounts.userId`) is taken only when `ec.artifactExecution.authzDisabled()` is true. `create#Room` skips the record visibility check when authorization is disabled. The root transitions (`/setUserStatus`, `/chatTyping`, `/chatUnread`), the Chat screen's bridge transitions and `/notws` call chat services with authorization disabled; each builds its parameters from named fields and passes no `userId`.
- **Message bodies.** A body is `allow-html="safe"` and at most 65,536 characters; an encrypted room's ciphertext is at most 131,072. `clientRef` matches `^[A-Za-z0-9_.:-]{1,64}$`. The cleaner is ammonia's defaults plus three `span` attributes (`crates/moqui-lib/src/safe_html.rs`). `send#Message`, `edit#Message` and a forward's copy clean the body with `moqui_lib::safe_html::clean_chat` (`StringUtilities.cleanChatHtml`): an `img` keeps its `src` only when it is `/richTextImage/<id>`, any other image loses it, and an image with no `src` is no body. The client matches: in an encrypted room `moqui.chat.clean` (a DOMPurify hook), and for a plain room's stored body and a forward `moqui.chat.ownImages`, drop any other image source, so a message stored before shows none (SEC-COM-02, fixed). The Chat screen shows the cleaned body with `v-html`; the default `Content-Security-Policy` of a screen render has no `img-src`.
- **Agent members.** `add#Member` and `set#MemberRole` take an `agentRef`, a service name, from a moderator or owner of the room, only when it is one of `chat@agent-entry-services` (default `org.moqui.agent.AgentServices.handle#ChatMessage`). `deliver#AgentMessage` then calls the named service for each message of the room. The BasicComms `addMembers` and `setRole` transitions pass no `agentRef`. `send#Message` queues `deliver#AgentMessage` with `ec.service.async()`: the worker makes a fresh context, signs in the user that was current when it was queued (`service/pool.rs` `internal_login_queued_user`) and runs the call with authorization disabled for its whole length. So the agent's service runs as the message's sender (no user at all for a bridge's inbound message), with `authzDisabled()` true, and the chat services it calls take `senderUserId` and `userId` from it.
- **Record rooms.** `ChatScreenServices.lookup#Records` passes the request's `recordEntityName` and `recordId` untouched to the apps' `record-lookup` providers and reads back what they answer; the framework does not use them as entity names. `find#RecordRoom` uses the provider's `entity` when the record is visible, else the request's own string, as the room's key, and refuses a record a provider says is not visible; with no provider answer it calls `create#Room` with the caller's string. mr-boss's provider hands the name to `ironstone.shared.ChatterServices.get#RecordInfo`, which calls `mantle.party.CommunicationServices.get#RecordLinkInfo`.
- **Files.** A member who may post uploads a file with the Chat screen's `upload` transition (`ChatScreenServices.upload#ChatFiles`, then `ChatServices.upload#Attachment` for each file in its own transaction). The file is kept at `dbresource://chat/<chatRoomId>/<chatAttachmentId>/<name>` (`chat@attachment-root` names another root). The name is the last path segment with control characters removed, at most 200 characters. The content type is the one the client declared. The service refuses a file over `chat@attachment-max-bytes` (25 MB) and a user's uploads over `chat@attachment-daily-bytes` (1 GB) in 24 hours. A file sent in chunks is at most 25 MB. A file never sent with a message is removed after 24 hours (`clean#PendingAttachments`). In an encrypted room the file is kept as `encrypted`, `application/octet-stream`. `send#Message` takes only files of that room that the sender uploaded and that no message has yet. The `attachment` transition (GET, `chatAttachmentId`) reads through `get#Attachment`: the room must be readable by the caller at the time of the read, and a file not yet sent is given only to its uploader. The file is sent as an attachment, or inline when `inline=true` and the stored type is png, jpeg, gif or webp. A file whose name has an active type is never inline.
- **Link previews.** After a message or its edit commits, `fetch#LinkPreviews` (`authenticate="anonymous-all"`, registered on commit by `queue#LinkPreviews`) makes the server fetch up to 3 links of the body, so any user who can post makes the server send requests. The request, its address guard and its limits are a row in [Attack-Surface-Outbound.md](Attack-Surface-Outbound.md#requests-the-server-makes). No picture is fetched. A preview is cached in `ChatLinkPreview` for 24 hours (1 hour when it failed), by the SHA-256 of the URL. Previews are off when `chat@link-previews` is `false`, when the sender's preference `CHAT_LINK_PREVIEWS` is `N`, in an encrypted room, and for a bridge's external actor. No limit other than the 3 links a message bounds how often a user makes the server fetch.
- **Presence.** `get#UserStatuses` (`authenticate="false"`, reached from `/userStatuses` with authorization disabled) answers, for up to 500 user ids of the caller's choice, each user's presence, status text, emoji, clear-at time, `inCall` and `lastActiveDate`. It checks no shared room. An external caller (`EXTERNAL_USER`) gets only their own. An id with no `UserStatus` row is answered as offline, the same as an id that is no account. Another user who is offline, or invisible, is answered with no `lastActiveDate` and `inCall` false, and the `userStatus` push to co-members carries `inCall` false for them (SEC-COM-06). The status text and emoji are still answered.
- **Pushes.** Chat pushes are notifications that are not persisted, on the topics `chatRoom:<chatRoomId>` (to the room's member users), `chatUser:<userId>` and `userStatus` (to the user's room co-members). A `/notws` topic is only a filter: a notification is queued on the sockets of the users its `ROUTE_USER_IDS` name, so following another user's `chatRoom:<id>`, `chatUser:<userId>` or `rtc:<callSessionId>` topic delivers nothing.
- **Device pushes** (plans/Comms-Messaging.md §7; `service/chat_push.rs`, `ChatPushServices.xml`). A message pushes to the devices of the room's members, and of a user it mentions who may read the room, by the room's level; no service takes a recipient from the caller (`append#Event` writes the job, `run#ChatPush`, `queue#ReadClear` and `filter#RingUsers` are `allow-remote="false"`). Each push is encrypted to its device's keys (RFC 8291) for every provider; the push service sees the device's token or endpoint, the time, a collapse key or Topic (an HMAC, keyed by `push/web@vapid-key`, of the registration, the room and the kind: no room id, and no link between two users' pushes of a room) and, for FCM, the registration id. A device that moves to another user drops what waits for it, and a send checks the device's user. An encrypted room's push has the room's name and the sender's name, never text; a user whose `CHAT_NOTIFY_PREVIEW` is N gets no snippet. An FCM device that gave no keys gets `{kind, pushId}` only for a chat push; a notification to it keeps its kind, topic and tag only, and a call's ring goes to Google as it was, with `callSessionId`, `chatRoomId` and `caller` readable (`fcm.rs`). By default a plain room's push carries a snippet of the body (140 characters), the room name and the sender, which a locked device may show; only `CHAT_NOTIFY_PREVIEW` `N` removes the snippet. The payload is written to `PushOutbox.payloadJson` (not an encrypted field) when the job is worked and kept until 7 days after the push is done. A deleted message is left out only when the payload is written: at the send the worker checks the time to live (4 weeks), the read marker, that the device is enabled and still the user's, and a native device's key, not that the event is not redacted, that the user is still a member or that the account may sign in, so a push held by do not disturb or waiting on its push service goes out as written. Redaction, retention and member removal do not name `PushOutbox`. One thread a node (`moqui-chat-push`) sends all chat pushes one after another, up to 500 a round, each a blocking POST with a 3 second timeout for the whole request (a Web Push or UnifiedPush one), internal addresses refused and no redirects; a 2xx answer is delivered, and an endpoint that never answers is tried 12 times. A Web Push or UnifiedPush answer taken after 2 seconds counts as a failure of its endpoint though delivered, so one that answers late is disabled after 5 in a row as one that fails is (`PushTransport::slow_after`); storing the endpoint again keeps its failures, and one disabled for them stays disabled (`keep_failures`), so the browser ends its subscription and subscribes again for a new endpoint (`WebrootPush.js` `subscribeAgain`); and each recipient's pushes take at most 6 seconds of a round: the rest of that user's are put off to the next round and the pushes due are read again, up to 4 times a round, so a user with many old pushes does not hold everyone else's (`PushTransport::user_round_budget`, `UserBudget`, `put_off`) (plans/Security-Review-Medium.md SEC-COM-81). `store#WebPushSubscription` (`POST /WebPush/store`, any signed-in user) makes a row for each new endpoint (https with a host and no user part) and keeps at most `push/web@max-subscriptions` (10) of a user's active, disabling the longest unregistered past it (`cap_subscriptions`, SEC-COM-81), so a user's stored endpoints still decide where the server posts for each message of their rooms, at most ten times a message; a browser subscription is removed at logout and by nothing else that was found. `register#PushDevice` (`/qapps/registerClientPush`, a signed-in app holding a live device key of the user) holds a device key to one row; with provider `unifiedpush` its `token` is an https endpoint the server posts to in the same way. A VAPID token goes to whatever host an endpoint names, for UnifiedPush too when the key is set. `send#TestPush` (`/qapps/testClientPush`) queues a push for the caller's own device or the endpoint the caller names, one a user every 10 seconds whichever device asks (SEC-COM-81). `ack#Push` (`/qapps/ackClientPush`) counts only the caller's own push or device. `find#PushHealth` (System, Push Health) needs `PushHealthAdmin` (ADMIN) and returns no endpoint or key. A user's do not disturb allowed list (`store#DndAllowed`) is the user's own preference; it lets the people on it ring the user's phones through do not disturb.

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
10 a minute, burst 5), which applies only while per-client web protection is on (`webapp-list/webapp[webroot]@trusted-proxies` set, or
`webapp-list/webapp[webroot]/web-protection@enabled` `true`; [Web protection](Attack-Surface-Web.md#web-protection)). On the default conf `joinGuest`
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
- A meeting takes at most 50 guests with an open guest leg, waiting or in the call (`callFull`). A guest who leaves,
  whose credential runs out or whose offered leg times out no longer counts (SEC-COM-05). A link takes at most
  `calls@guest-joins-per-minute` (20) joins a minute whatever the client (`tooManyJoins`).
- `/callws` holds at most 64 sockets on a node that have not yet sent a credential, 4 of them from one client address,
  each for up to 5 seconds (SEC-COM-04). Another socket is answered 503. A socket refused for its first frame (none,
  not a credential, or one that does not work) counts as a failed sign-in of its client for web protection.
- `guestState` shows a waiting guest nothing but their admission. Once admitted, a guest sees the names of those in the
  call. They never see the room, its members or its history.
- A guest leg can negotiate media only once admitted (`bind#GuestConnection`).
- Revoking the link, or removing or denying the guest, ends the credential and closes the guest's legs.
- A link made end-to-end encrypted (W8) takes no guest (`encryptedNoGuests`): a guest has no crypto device. The page says
  so from the token's route context (`encrypted`).

## Call captions

Call captions (W6, [WebRTC](../interfaces/WebRTC.md#captions)) send call audio out to a speech provider, so they are off
unless `calls/captions@enabled` is `true`. `start#Captions` and `stop#Captions` (the Calls screen's `startCaptions`,
`stopCaptions`) need a signed-in user with an answered leg in the call; anyone else gets `notFound`. An end-to-end
encrypted call is refused. Every leg, a late joiner's and a guest's too, gets the notice naming the provider before its
audio is captioned. The provider is the `transcribe` model binding, reached through `moqui-llm`'s address guard (a
private address needs `allow-private-network`), counted against the quota of the user who turned captions on, and kept
as an `LlmCall`. No screen transition names `org.moqui.call.SpeechServices.transcribe#Audio` or `get#SpeechProvider`:
other services call them (captions, voicemail transcription, mr-mantle's process checks;
[services and `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)).
`transcribe#Audio` reads any `audioLocation` it is given, so its caller decides what may be sent out. Captions are not
stored unless `calls/captions@store` is `true`.
`transcribe#CaptionAudio` takes only a participant of the call as the quota's user, caps a chunk at 4 MiB, and as
`_edge_` takes only a call whose media is on the edge node. Each caption goes to every leg of the room, a guest's too,
as `{rtc: 'caption', leg, participant, name, seq, text}`; the text is the provider's answer, trimmed only
(`crates/moqui-host/src/rtc/captions.rs`). Caption jobs run on the service pool, at most half its workers for all calls
of the node. The captions thread decodes every browser's Opus and has no `catch_unwind`. A stored caption is a
`CallTranscript` row with `callSessionId` and `callLegId` and no `callRecordingId` (`store#CallTranscript`, which takes
only a leg of that call). No service, screen or job reads those rows, and none deletes them: `CallTranscript` rows are
deleted only by `callRecordingId` (a recording's) and by `voicemailId`. A reader of the entity (the database, entity
REST for `ADMIN`) reaches them. `stop#Captions` is refused with `captionsOff` once `calls/captions@enabled` is turned
off, so captions already on then stop only with the call.

## Call media

Off unless `media-server@port` is set, and only in a binary with the `rtc` feature (Maximal, Enterprise, Debug). One UDP socket on `media-server@host` (127.0.0.1 by default); browsers are told `media-server@public-address` (the bound address by default; a wildcard bind without it does not start). It carries ICE (STUN) and DTLS-SRTP for every browser leg (`crates/moqui-rtc`, on str0m, ICE-lite). A datagram goes to the peer whose ICE session (its ufrag and password from the SDP exchange over the authenticated socket) accepts it; anything else is dropped. A peer exists only after its leg's `join` was bound by the call services and reached the media node; leaving, a newer generation of the leg, the call's end (`send#MediaControl`) and ICE consent loss remove it. At most 200 rooms and 50 legs a room on a node; SDP over 64 KiB is refused. Media is hop by hop: the SFU decrypts and re-encrypts each stream, except the frames of an end-to-end encrypted call (below), which it forwards as they come. The node registers itself (`CallMediaNode`) every 20 seconds; a call whose node stops registering ends.

Cascading media nodes (`media-server@cascade-enabled`, off by default) add relay links between the media nodes of a call on the
same UDP sockets: one str0m peer a room on each side, DTLS-SRTP like a browser leg, negotiated only by `rtc-signal`
messages on the cluster's client-certificate HTTPS transport (kind `relay`), so only a cluster member opens one. The
origin takes at most 8 links a room and 64 in all, and refuses more (`tooManyRelays`); a message of an older link is
dropped, a room that ended takes no new link, and a link not connected within 10 seconds is dropped. A leg node forwards
to the origin only its own legs' tracks; the origin decides nothing a leg node asks beyond the layers it sends it. Any
cluster member may open a link for any room id: the cluster's members are trusted alike.

ICE-TCP (`media-server@tcp-port`, off unless set) takes TCP connections on the same address: at most 1,000, and 64 from one
source address. A connection that sends
nothing for 30 seconds is closed. Each carries
RFC 4571 frames of up to 2,048 bytes into the same SFU, which accepts only what a peer's ICE session takes.

A bound leg's `mute`, `hand`, `reaction` and `unpublish` call frames are sent to every other leg of the room as a new
frame of only that frame's own fields (booleans, an emoji of at most 64 bytes, a short `mid` or `source`), with `from`
and `callLegId` the sender's, made by the media node (`crates/moqui-host/src/rtc/driver.rs` `relayed_frame`). An
admitted guest's leg sends them too. A call frame is at most 128 KiB; a larger one is refused (`tooLarge`). A leg
relays at most 20 of these at once and 5 a second after (`RelayBucket`); the rest are answered `rateLimited`. Any other `rtc` value is answered as an error.

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

Off unless `calls/recording@enabled` is true ([Call recording](../interfaces/Call-Recording.md)). It opens no port. An
admitted host of the call, or the owner of its call link (in the meeting or not), starts, pauses, resumes or stops it
(`org.moqui.call.CallRecordingServices`, `CallRecordings.js` `isHost`); the recording's owner who is no longer a host
also pauses, resumes and stops it. The call must be active, with a media node, and not end-to-end encrypted. Only the
owner deletes a recording, once it stopped. The recorder runs on the
media node, takes frames from a bounded queue and spools under `runtime/tmp/call-recording/`. `store#RecordingFile`
(called by the recorder in the server; no screen transition names it) refuses a spool path outside that directory and a
location outside
`dbresource://moqui/call/recording/`. The files are read only through `get#RecordingFile` by segment id, from the
BasicComms Calls `Recordings` screen's `recordingFile` transition: the one who recorded it and the call's hosts get the
file, anyone else "not found" (an empty JSON answer) for the same id. No route serves those `dbresource://` locations
to users, and an app's record links (BOSS) go through the same services. The guest page's `ackRecording` and
`withdrawRecording` take the guest's credential and act on its own leg only. Every leg gets the notice, and each is
logged (`CallRecordingConsent`, with every earlier acknowledgement and withdrawal in `CallRecordingConsentHistory`).
The files count toward the owner's personal drive quota only when the owner has a personal drive with `quotaBytes`
(`CallRecordings.js:162`): then a full drive refuses a start, and a recording that fills it stops, so recording cannot
fill the database past the quota by more than a segment. With no personal drive or no `quotaBytes` the bounds are
`call_recording_max_mb` (2048) a recording and `call_recording_retention_days` (90); nothing bounds how many recordings
a host makes. Retention deletes the files and
rows (`expire_Recordings_hourly`).
A recording has one OS thread and one store thread, with no cap of their own; it needs a room, so the room cap bounds
them. A full tap queue drops frames and never blocks the media loop; past 100 dropped the recording fails. The spool
file is `runtime/tmp/call-recording/<callRecordingId>/<period>-<seq>-<leg>-<source>.<ogg|webm>` (the key and the leg
cut to `[A-Za-z0-9_-]`), the stored one
`dbresource://moqui/call/recording/<callRecordingId>/<callRecordingSegmentId>.<ogg|webm>`, and the download name
`call-recording-<id>-<kind>[-<source>]-<period>-<seq>.<ext>`: every part is made by the server. `recordingFile` (GET)
sends the file inline only for an audio or video type. An edge node's queued segment (`_edge_` only) cannot name a
spool path or a recording of a call whose media is not on it.

## TURN

The embedded TURN server (`crates/moqui-rtc` `turn/`, host `rtc/turn.rs`) runs on the media node. Each listener is off
unless set: `media-server/turn-server@port` (UDP and TCP), `media-server/turn-server@tls-port` (TLS), and 443 on `media-server/turn-server@tls443-host`. TLS uses the
`runtime/cert` certificates. Relays are UDP sockets in `media-server/turn-server@relay-ports`. The listeners bind `media-server/turn-server@host`, which is
`media-server@host` when empty. The URLs given to clients name `media-server/turn-server@public-host`, the media node's public address when
empty. Its rules:

- **Credentials** are short-lived TURN REST ones: `<expiry>:<legId>`, HMAC-SHA1 with `media-server/turn-server@auth-secret` (generated
  when unset; is-secret). They go only to an admitted leg (`get#IceServers`, `get#GuestIceServers`). An expired one
  is refused, and so is one of a leg that left (revoked by the media node, for 24 hours). On an edge node a local node's
  are `<expiry>:<legId>@<pairing>`, signed with that pairing's secret and taken only while it is active (E16). A credential
  lasts `media-server/turn-server@credential-hours` (4; held to 1..24). `media-server/turn-server@external-urls` are handed out with the same username and a
  credential under the same secret. A request with no credential is answered by `turn-server-proto` with a 401 and a
  nonce; a nonce lasts 600 seconds. An XMPP user's credential (`xmpp.<userId>`) is never revoked; it ends at its expiry.
- **Relay peers** are only the media nodes' media ports (and `media-server/turn-server@allowed-peers`, by address and port). A permission
  to any other address, or a channel to any other address or port, is dropped before the server sees it, and nothing
  is relayed to or from one, another port of a media node's address included. The exception is an XMPP user's own
  credential (`xmpp.<userId>`, from `XmppServices.get#IceServers`, XEP-0215), for calls between XMPP clients: it may
  relay to public unicast addresses too, never to loopback, private (RFC 1918, unique-local), link-local, shared
  (100.64/10), multicast or documentation ranges, nor an IPv6 address that carries an inside IPv4 one (6to4 of one,
  IPv4-compatible, NAT64, Teredo), nor another port of a media node's address (the `allowed_peers` addresses, which hold
  the media node's public
  address and its bind address when that is not a wildcard), nor the address a TURN listener is bound to
  (a relay there would come from the server itself, past a firewall that keeps the outside off those ports).
  A public address of the host that is neither (the address behind `media-server/turn-server@public-host` when it differs from the media
  address, another interface) is not refused.
- **Limits:** two allocations per credential, `media-server/turn-server@max-allocations` in all, and `media-server/turn-server@max-kbps` each way per
  allocation; excess data is dropped. A stream buffers at most 64 KiB without a whole message. No TCP relays to
  peers are offered.
  The two allocations are counted by the request's leg, over all of its usernames. A username is given to the server's
  user table only when the request's MESSAGE-INTEGRITY holds for that name's password (`admit_message`,
  `Message::validate_integrity`); any other request gets the server's 401, as an unknown name does (plans/Security-Review.md
  SEC-COM-13, fixed). turn-server-proto has no removal, so names that proved their password stay for the life of the
  process, bounded by the credentials the services issue. The TCP and TLS listeners take 512
  connections at once and 16 from one source (an IPv6 /64 is one). A connection has 10 seconds to send its first whole
  message (the TLS handshake included), and is closed after 10 minutes with nothing read or written, when 512 messages
  wait to be written to it, or when a write takes over 5 seconds. The UDP listener has no per-source limit; it reads
  datagrams into a 2,048 byte buffer.

coturn can take its place ([TURN](../interfaces/TURN.md)), with the same secret and a deny-all, allow-media-node
peer rule.

## SIP

The SIP engine (`crates/moqui-sip` on rsipstack, host `src/sip/`) runs beside a media node, only in a binary with the
`sip` feature (Maximal, Enterprise, Debug). Each listener is off unless set: `sip-server@port` (UDP and TCP on `sip-server@host`),
`sip-server@tls-port` (TLS on `sip-server@tls-host`, the `runtime/cert` certificates). Each phone leg has a UDP RTP socket in
`sip-server@rtp-ports` on `sip-server@rtp-host`. An exposed SIP listener is scanned within hours, so its rules are release gates
([SIP](../interfaces/SIP.md) "Fraud controls"):

- **Who is answered.** An INVITE from a trunk's host (each address of its SRV targets, for a trunk set up by name
  with no port, looked up again at each registration and ping) or listed signaling addresses is a trunk's call; any other
  INVITE or REGISTER is challenged with SIP Digest (SHA-256 and MD5, nonces that expire after 5 minutes and carry
  random bytes and a MAC under a per-process key, nonce counts that must go up). Only a PBX trunk's credentials
  (profile "PBX on a private network") answer it: a provider trunk's are the provider account's password, so a digest
  with them is refused and counted as a failure, and a provider's calls come only from its addresses. A listed address
  range is at most a /8 (IPv4) or a /32 (IPv6); the services refuse a wider one and the engine ignores it. An OPTIONS from a trunk's address is answered 200 and
  any other method outside a dialog from it 481. Other requests from addresses that
  are not a trunk's are not answered. A trunk's addresses are what DNS answers: each SRV target's addresses replace
  the trunk's at each registration and ping (`resolve.rs`, `ua.rs`), from the system resolver with no DNSSEC
  requirement, so whoever can answer that lookup names the addresses that place calls as the trunk with no digest.
  A request that matches a live dialog is handed to it before the source, trunk and digest checks (`ua.rs` `incoming`,
  `match_dialog`): rsipstack matches by Call-ID and the two tags and checks that the CSeq does not go back, and no code
  compares the source with the dialog's peer. So a BYE (ends the leg), a re-INVITE (answered from the leg's local
  media; the leg's RTP then goes to the new address, with new SRTP keys when the offer has them) or a REFER (passed to
  the host as the leg's transfer request) that names a dialog's ids is handled whatever address it came from. This
  side's Call-ID is 20 random characters; the ids are seen by a network peer on the path of UDP or TCP signaling, not
  over TLS. The source of a request is the connection's peer, else the top Via as rsipstack rewrote it from the
  datagram's real address and port (a client's own `received` and `rport` are stripped); rsipstack drops nothing for a
  Via that does not match its source. Without the registrar, REGISTER is refused (403) without a
  challenge, so it counts no failures. Over UDP a
  source address can be forged, so a trunk taken by its address is best on TLS (the default) or TCP.
  A digest answer whose `uri` is not the Request-URI (`uri_matches`) is refused with 403 and not counted. The UDP socket also answers a STUN
  Binding request (the RFC 5626 keepalive) from any source with a Binding success that carries the source's address (32
  bytes for IPv4, 44 for IPv6). That answer is sent before the SIP stack sees the datagram, so it is given with the
  registrar off, to a blocked source, and outside `sip-server/limits@request-rate`. No other STUN message is answered.
- **The registrar (V4).** Off unless `sip-server/registrar@enabled` is true, even with the listeners on. With it, a REGISTER
  and a phone's INVITE answer the same challenge with a desk or soft phone's credentials: a generated username and a
  22-character random password that is shown to its user once and never stored. Only its digest verifiers are kept
  (`SipDeviceVerifier.ha1`, encrypted, per algorithm and realm): SHA-256, and MD5 only for a device of the legacy
  profile, so an MD5 answer for a standard device fails. A failed or replayed answer counts against its source as any
  failed digest; no device or account is locked. Each REGISTER or signed INVITE looks the username up once
  (`get#SipDeviceAuth`), within the per-source rate. Rotation deletes the old verifiers and revocation the device's,
  and either ends its bindings and its calls on every engine. A binding is the flow the phone opened (its transport
  and source address and port, the transport the connection's, not the Via's claim); the INVITE of a call to the phone
  goes back over it only, never to the address its Contact names, and
  a stream connection that closes ends its bindings. A dialog whose first request came or went over TCP or TLS keeps
  that connection for every later request. A UDP dialog, and one whose connection closed, is routed by rsipstack
  (0.8.5, `dialog/dialog.rs`, `transaction/transaction.rs`) by the remote target and the route set, the Contact and
  Record-Route the peer gave: the ACK of a 200 and every later request (BYE, re-INVITE) go there, a name is resolved,
  and a `;transport=tcp` target with no open connection is dialed. The endpoint sets no target locator, whitelist or
  outbound proxy, and no code checks the address of a Contact; `media_target_allowed` guards RTP only. The registrar
  keeps the Contact URI whole as text (bounded only by the 65,535 byte message) and uses it only as the INVITE's
  Request-URI; a REGISTER's `User-Agent` is cut to 200 characters without control characters. A REGISTER whose CSeq
  does not go up is refused. A device has at most 10 bindings, and expiry is held to
  `sip-server/registrar@min-expires`..`sip-server/registrar@max-expires`. A phone's own call is placed as its user through the same
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
  A nonce's MAC covers the address the challenge was sent to (`NonceBook::nonce`, `verify`; an IPv4-mapped IPv6
  address is the same address): an answer with a nonce not issued to its own source is challenged again and not
  counted, so only a source that received the challenge counts failures against itself. A failed answer from a trunk's
  target or listed inbound address is refused (403) and never counted (`authenticate`, `trunk_by_address`; SEC-COM-25,
  fixed); a block an operator set or one reloaded from `VoiceAuthFailure` still applies to it.
- **What an incoming call reaches.** Only a number in `PhoneNumber` rings anyone, through the call services'
  ringing; the SDP must be audio with a codec in common, and SRTP when the trunk's profile requires it (488
  otherwise).
- **Outbound.** Calls go out from `send#SipControl` (the `rtc-signal` cluster topic, kind `sip`, to that node),
  which only the voice services send, after their checks (caller ID grant, outbound enabled, international and
  premium off by default, concurrency and daily limits, the emergency path). The only other source is an edge node's
  engine, which dials a number's replicated fallback
  forward itself while its local node is away ([Edge node](Attack-Surface-Cluster-Edge.md#edge-node)), through the
  engine's `dial` (the class check) and within the number's limits; a number of another trunk is refused. Those forward
  counts are in memory (`crates/moqui-host/src/sip/fallback.rs` `ForwardCount`), so a restart of the edge node's engine
  starts each number's forwards-a-day count again. With home country
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
  the trunk), a forward back to the caller's number is skipped, and a call takes at most `calls@forward-max-hops`
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
  allowed algorithms. `store#VoiceTrunk` (`Voice.js` `storeTrunk`) clears the stored password when the host, port,
  transport, username, authUsername or realm changes and the password is not given again (`passwordCleared`), and the
  trunk is written only by that service, never a generic tool, REST or request load (`SERVICE_WRITTEN_ENTITY_KEYS`). TLS to a trunk verifies its certificate against `sip-server@tls-ca-file` (the system bundle by default),
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
  Until a packet latches, a leg sends its RTP to the address and port the SDP names, but never to an address no phone
  can be (`moqui-sip` `media::media_target_allowed`): an unspecified, multicast or broadcast address, port 0, the leg
  socket's own address, and loopback or link-local unless the socket is bound in that scope. A private address stays
  allowed (a phone behind NAT names one); the first refusal per target is logged.
  rsipstack logs whole SIP messages (SDES keys, digest answers) at INFO, so the host keeps only its warnings.
  The SDP parser has no limit of its own (the message's 65,535 bytes bound it); the `c=` address is any IP literal,
  an unspecified one means hold, and only formats the `m=` line lists are used. SRTP has one context for every SSRC
  and a 64-packet replay window, checked before the tag and updated after; a key with an MKI is refused, and SRTP and
  plain RTP are never mixed or downgraded.
- **Codecs.** G.711 and G.722 are decoded in house, in safe Rust: tables are indexed only with masked values and sums
  saturate. Opus is opus-rs 0.1.42, a Rust port of libopus with no C code but with unchecked indexing
  (`get_unchecked`), raw pointers and `transmute` throughout. The wrapper (`crates/moqui-rtc/src/audio/opus.rs`) sets
  the output size (5,760 samples) and no input limit, so frames go to `decode` and `decode_fec` as they came: from a
  phone or trunk that negotiated Opus, and from any leg that publishes audio, a guest's included, while the call has a
  mixed recording, captions or a phone leg (each decodes every browser's Opus). The recorder catches a panic and fails
  the recording; the captions thread and the phone leg's worker (`crates/moqui-host/src/sip/leg.rs`) do not. A jitter
  buffer holds 10 frames a stream, and a stream idle 5 seconds is dropped.
- **DTMF.** A telephone-event packet with a timestamp other than the last one is a new digit (`dtmf.rs`). The leg's
  worker sends each digit to the engine over an unbounded channel, and the engine spawns a task and calls
  `note#PhoneLeg` for each; there is no minimum gap between digits and no count a leg. The engine logs each digit of a
  call at debug (`crates/moqui-host/src/sip/mod.rs`); the default level is info (`logging@level`), and digits of a
  voicemail or IVR session return before that line, so a voicemail PIN typed on a phone is not logged even at debug.
- **Floods.** rsipstack has no caps of its own, so the engine's transport inspector holds new requests to
  `sip-server/limits@request-rate` a second per source (`sip-server/limits@request-burst` at once; 20 and 100 by default, an IPv6
  source as its /64), drops them while `sip-server/limits@max-transactions` (2,000) run, and closes TCP and TLS connections past
  `sip-server/limits@max-connections` (500), or past `sip-server/limits@max-connections-per-source` (100) from one source, so one source cannot
  hold the connections the registrar's phones keep open. What is over is dropped unanswered; requests in a dialog and responses pass, so calls
  going on are not cut. The inspector does not ask whether a dialog exists: a request is new when its To header does
  not hold the text `;tag=` (`ua.rs` `is_new_request`), so any request whose To carries a tag passes the rate and the
  transaction cap. rsipstack makes a server transaction of it, and `incoming` treats an INVITE or REGISTER with no
  matching dialog as a new one (challenged, or taken as a trunk's call from a trunk's address). A blocked source is
  still dropped first. Answers sent before an incoming call's decision expire after 30 seconds (1,000 at most).
  A TLS client has 10 seconds to finish its handshake; at most 128 handshakes run at once and 8 from one source (an IPv6
  /64 is one), and a connection past either is closed. A TCP or TLS connection that sends no byte for 300 seconds is
  closed (rsipstack's default; a CRLF keepalive counts as a byte). No deadline closes a connection that never sends a
  valid request. A SIP message over a stream is at most 65,535 bytes.
- **Limits.** A leg's jitter buffer holds 10 packets; a leg's consumer queue a second of frames; datagrams over 2 KiB
  are cut. A leg's RTP socket is read until it is empty each tick, with no count. The engine registers its node (`SipNode`) every 20 seconds.
- **Settings that name addresses and audio.** `sip-server@public-address` is the address put in Contact headers and SDP, and
  is required when a SIP host binds every address. `sip-server@domain` is the digest realm (`sip-server@public-address`, else `moqui`,
  when empty). `voicemail@prompt-location`, `calls@hold-music-location`, `calls/recording@prompt-audio` and
  `calls/captions@prompt-audio` name resource locations (any scheme) whose files the engine reads and plays to callers.
  They are empty by default.

## XMPP

The XMPP server (`crates/moqui-xmpp`, host `src/xmpp/`, [XMPP](../interfaces/XMPP.md)) runs only in a binary with the
`xmpp` feature (Maximal, Enterprise, Debug) and only when `xmpp-server@domain` is set. Each listener is off unless set:
`xmpp-server@port` (STARTTLS, required before anything else) on `xmpp-server@host`, `xmpp-server@tls-port` (direct TLS) on
`xmpp-server@tls-host`, and `/xmpp-websocket` with `xmpp-server@websocket-enabled`. Local users only: no server-to-server
(`xmpp-server@s2s-port` is declared in the default conf and no code reads it). `xmpp-server@host` defaults to `0.0.0.0`, so `xmpp-server@port`
listens on every IPv4 address once it is set. `xmpp-server@tls-host` is empty by default and then takes `xmpp-server@host`; an empty
host is `0.0.0.0`. A direct TLS handshake and a STARTTLS handshake each have 10 seconds. A connection with nothing read
for 600 seconds is closed, and a write the client does not take for 60 seconds ends it.

- **Sign-in** takes app passwords only, never the account password (which would count toward the account's lockout
  and cannot carry a second factor): SCRAM-SHA-256 and SCRAM-SHA-256-PLUS against the verifiers stored when the
  app password was made (salted, 10,000 iterations), with tls-exporter channel binding where the connection has its
  own TLS session and a downgrade (`y` where the server binds) refused; PLAIN over TLS only; FAST tokens (XEP-0484)
  bound to the client's user-agent id, kept encrypted, expiring (`xmpp-server@fast-token-days`), and without channel binding
  refused unless their count grows; a token goes with the app password it was issued under. An unknown user gets a salt
  derived from the name and `xmpp-server@scram-decoy-key` (is-secret, generated when unset), so the exchange does not tell it
  apart, and its salt stays the same across restarts and nodes. A disabled account cannot sign in.
  A session checks every minute (`check#Credential`) that its app password is not revoked, that the
  account is not disabled with no `disabledDateTime` and not past its `terminateDate`, and ends when not (SEC-COM-15).
  An account locked after failed logins keeps its session; its service calls are refused by `internal_login_user`,
  which counts no failure, so the lock ends after `disable-minutes` (SEC-COM-12).
  A SCRAM or FAST sign-in is decided in `moqui-xmpp` from `get#Account` (`canLogin`: `disabled` is not `Y`, or
  `disabledDateTime` is past, and not past `terminateDate`) and the stored verifiers or token, and then asks the host
  (`Backend::sign_in`, `login_verified_app_password`): the key login's gate, `ipAllowed`, a `UserLoginHistory` row and
  `UserAppPassword.lastUsedDate` (at most once a minute); a refusal fails the exchange with not-authorized. A failed
  attempt is a log line at WARN with the address and no username. A PLAIN sign-in goes through `login_app_password`,
  which writes the history row and `lastUsedDate`. The host runs every call of the session, the sign-in lookups
  included, inside `with_client_ip(peer)`, and after the sign-in every service call logs the user in again with
  `internal_login_user`, which refuses a disabled or terminated account and one whose password must change, and checks
  `ipAllowed` of the account and its groups against the session's peer (plans/Security-Review.md SEC-COM-11, SEC-COM-22,
  fixed). The sign-in services `get#Account`, `get#ScramCredentials`,
  `check#PlainPassword`, `check#Credential` and the four FAST token services are `authenticate="false"` and run with
  authorization disabled (`Caller::Internal`). App passwords are made in BasicComms Settings, Chat apps ([The BasicComms
  app](#the-basiccomms-app)), with the scope `xmpp`; SCRAM (`get_scram_credentials`) and PLAIN
  (`login_app_password(.., "xmpp")`) take only a row made for `xmpp` or one made before scopes. Chat apps lists exactly
  those rows, as `docs/interfaces/XMPP.md` says. A row made before the SCRAM fields existed is left out of SCRAM, so it
  signs in by PLAIN over TLS only; the screen lists it with no note of that.
  Only a TCP connection without TLS must STARTTLS (`session/auth.rs`). A WebSocket whose request was not HTTPS in this
  process is taken: it is offered SCRAM-SHA-256 without PLAIN, and its stanzas go in the clear unless a proxy in front
  ends TLS. PLAIN's authzid must be the authcid. `issue#FastToken` stores the mechanism the client asked for, and
  `get#FastToken` and `use#FastToken` never read it, so a token is taken by either mechanism; for `HT-SHA-256-NONE`
  the proof is `HMAC(token, "Initiator")`, the same for every sign-in, with `count` an attribute beside it (the
  mechanism as XEP-0484 has it). For a name that is no account, `get#Account` runs the exact find and then the
  case-insensitive find and `credentials()` returns; for an account it then calls `get#ScramCredentials` (an account
  find and an app password find), so the lookups before the challenge differ by a service call while the messages are
  the same in form; the time was not measured. A session's own address is the lower-case, trimmed username
  (`jids.rs` `escape_node`), and `get#Account` finds the exact name first, then any name equal without case: two
  accounts whose usernames differ only by case have one JID.
- **Failures** are counted per source address (moqui-lib `source_guard.rs`, as the SIP engine counts failed digests):
  5 in 10 minutes block the address for an hour, a /64 for IPv6; the account is never locked. The counter is in memory
  in each node's process (`Server.guard`): it is not shared between nodes, not kept across a restart, and not the web sign-in's.
  Over WebSocket the
  address is the HTTP listener's `ClientAddr` (the client a trusted proxy names), the value web protection uses. Over
  an edge tunnel on a local node it is the `x-moqui-edge-client` header of the tunnel's CONNECT on the down connection
  (`edge_link/down.rs`), which the edge node writes from the TCP peer it accepted: whoever holds the edge node's end of
  the link names any client address, and with it which address is blocked, counted and checked against `ipAllowed`. On
  an all-in-one node it is the accepted socket's. At most 20 connections from one address at once and 10,000 in all
  (TCP and WebSocket); a connection that has not signed in and bound a resource within a minute is closed at the next
  30 second tick, so it lasts
  60 to 90 seconds, and one that signed in and bound nothing runs no credential check until then; a stanza or stream header over 256 KiB,
  or nested over 32 deep, ends the stream; a session whose pushes back up (512) is closed. The 256 KiB count
  (`framing.rs` `feed`) leaves out only whitespace between stanzas: a call that ends in `NeedMoreData` at depth 1 is
  counted while a start tag is under way (minidom's builder stays at depth 1 until a top-level stanza's `>`), so a start
  tag sent in pieces that each end in the middle of an attribute counts whole and ends the stream at the limit
  (SEC-COM-82). rxml does not refuse an element for its number of attributes (a token is at most 8,192 bytes); the
  count bounds them. Over `/xmpp-websocket` a frame is at most `max_stanza_bytes`.
  Stream management keeps at
  most 1,000 unacknowledged stanzas, with no byte bound, and 10 detached sessions a user; PEP 200 nodes a user and 256 items a node;
  uploads 60 slots a user an hour. A detached session is resumed only by a sign-in of the same user. It runs no
  credential check (`conn.rs` `wait_detached` calls no `tick`), so for up to `xmpp-server@resume-seconds` (300, at most 3600)
  a session of a revoked app password or a disabled account goes on taking pushes into its unacknowledged queue.
  One MAM query returns up to 250 messages, each with up to two more service calls; a MUC join asks up to 100.
  A PEP item is at most 262,144 characters, and `publish#PepItem` keeps one only while the user's PEP items stay within
  `xmpp-server@pep-max-bytes` (20 MiB) of UTF-8 (`XmppPepItem.payloadBytes`, the replaced item not counted). `block#Jids` takes
  500 JIDs a call and blocks none past `xmpp-server@block-max-jids` (1,000); `enable#Push` adds a row for each `pushJid` and
  `node` and keeps none past `xmpp-server@push-max-registrations` (50, a replacement not counted). Past a cap the IQ is refused
  with `resource-constraint` (type wait); 0 is no cap. There is no per-session stanza rate limit; a session's stanzas are
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
- **Chat pushes to a session.** The users a `chatRoom:` push names are read from the member rows when each event is
  pushed (`ChatServices.xml`: current members, a direct room's always, not filtered and not blocking), so a user who
  left is not named and their session gets nothing, whatever its cache holds. The session's cached room decides only
  how the stanza looks (`direct`, `encrypted`, joined, names), and a `chatUser:` `room` notice clears it. A member who
  can no longer read by another rule but still has a member row is named, as on the web socket.
- **XML** is parsed by rxml 0.14.0 with `RawParser::new()`, the defaults (`framing.rs`). After `<!` its lexer takes
  only a comment or CDATA start, so `<!DOCTYPE`, `<!ENTITY` and `<!ELEMENT` do not parse; a processing instruction and
  an XML declaration after the start are refused; comments are refused by default; an entity reference is one of the
  five named ones or a character reference, so nothing expands. The raw parser does not check duplicate attributes or
  the number of attributes. JIDs are parsed by the `jid` crate, attributes are set through `rxml::NcName`, and nothing
  writes XML by string but the two stream headers, which escape.
- **End-to-end encrypted Moqui rooms** (C7) are not served; OMEMO messages are kept as they came, unreadable to Moqui,
  never in a bridged room, with no text in snippets, search, notifications or agents. PEP items (OMEMO bundles hold
  public keys) are read by access model: `open` any local user who is not external (an `EXTERNAL_USER` reads it only
  when they share a room
  with the owner), `presence` the people the owner shares a room with,
  `whitelist` the owner.
- **TURN for XMPP calls** (XEP-0215): off unless `xmpp-server@turn-enabled` is true; while off, external services are not
  advertised and a request gets an empty list. On, a credential keyed to the user (`xmpp.<userId>`) may relay to public
  unicast addresses only, never loopback, private, link-local, shared or documentation ranges ([TURN](#turn)); it is a
  relay to the internet within the TURN quotas, as a public TURN service's is. An external user (`EXTERNAL_USER`) never
  gets one, and a user at most `xmpp-server@turn-per-hour` (30) an hour (counted on the distributed cache
  `moqui.xmpp.turn.issued`); a refusal is the same empty list. The count is read and then written (a cache `get`,
  then `put`), and a user's sessions run on separate workers, so two that read the same count both pass: a user with n
  sessions can get up to about n credentials for each count, bounded by the user's connections (20 an address).
- **Push** (XEP-0357) registrations are stored, with their app server's secret encrypted; a notification has the message
  count only, and goes nowhere until federation (C3b).
- **Upload files** (XEP-0363; the route's row is in
  [Attack-Surface-Routes.md](Attack-Surface-Routes.md#host-routes-and-servlets)). An upload is stored at
  `dbresource://xmpp-upload/<xmppUploadId>/<name>` (`xmpp-server/upload@root` names another root). The token is 32 random
  characters for a slot, and for a chat file the first 40 letters and digits of `singleUseCodeHash(xmpp-server/upload@secret +
  ':' + chatAttachmentId)`. The route takes any method: `OPTIONS` is 204, `PUT` writes, and every other method is
  answered as a read. A file that was put is served at its URL with no expiry: the read checks only the token's row,
  `completedDate` and that the location has bytes, and loads the whole file into memory on a request worker. The content
  type is the one the uploader gave the slot. A type that starts with `image/`, `audio/` or `video/` is served inline,
  except the exact text `image/svg+xml` (a type with a parameter or another case, such as `image/svg+xml;
  charset=utf-8`, is inline). Every answer has `nosniff` and `Content-Security-Policy: sandbox; default-src 'none'`. A
  user's slots are limited to 60 an hour and ten times `xmpp-server/upload@max-bytes` an hour, and uploads count toward no
  drive quota. `attach#Upload` makes a `ChatAttachment` of the room on the same stored file, with the uploader's file
  name and content type. It refuses an upload that is not the caller's own completed one, and the session then logs the
  upload by its file name at WARN, never the URL whose token reads it. `clean_XmppUploads_daily` (`XmppServices.clean#Uploads`) removes slots never
  put within an hour, a completed upload no chat file has `xmpp-server/upload@unattached-hours` (24) after it was put (so a
  file shared only inside an OMEMO message goes then), and expired FAST tokens. `ChatServices.clear#MessageContent` and `clean#PendingAttachments` remove each chat
  file through `ChatServices.remove#AttachmentFile`: a file under the upload root goes, with its `XmppUpload` rows, when
  no other `ChatAttachment` has its location, so a file put over XMPP goes with the last message that has it (deletion or
  retention); and the `XmppUpload` rows made to serve a chat file to XMPP clients (`get#AttachmentUrls`) always go with
  the chat file (SEC-COM-14, fixed).

## Voicemail

Voicemail ([Voicemail](../interfaces/Voicemail.md)) opens no port: it runs on the SIP engine above, for calls to a
user's number that nobody answers, and on the access number (`PnrVoicemail`) when an administrator sets one.

- **The PIN over the phone.** A caller who presses `*` in a greeting, or calls the access number, can enter a box
  number and a PIN. `check#VoicemailPin` (internal) checks it against the box's argon2 hash and limits failures per
  box (5 in 15 minutes, then 15 minutes refused from any caller) and per source (the caller's number, 10 in an hour,
  then an hour). The source is a caller ID the caller can fake, so the box's limit is the one that holds: up to 480
  guesses a day (5 every 15 minutes), which tries every four-digit PIN in three weeks and every six-digit one in under
  six years, so a new PIN
  has at least `voicemail@pin-min-digits` (6 by default) digits. A box that does not exist, has no PIN, or is blocked is refused in
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
  `voicemail@transcribe` is on.
- **Limits.** A message is at most `voicemail@max-seconds` (120; at most 600), spooled to disk as it is recorded; one
  under a second is dropped.
  `voicemail@prompt-location` names a folder (any resource location) of prompts the engine plays; it is empty by
  default.

## Chat bridges

The bridge layer ([Chat bridges](../interfaces/Chat-Bridges.md), C1) opens no port. The host routes two endpoints for
it (`crates/moqui-host/src/channels.rs`): `POST /channels/{bridge}/webhook` and `GET /channels/{bridge}/media/{token}`
(their rows are in [Attack-Surface-Routes.md](Attack-Surface-Routes.md)). Five adapters are registered
(`crates/moqui-channels/src/lib.rs`): SMS for Telnyx, Twilio and Sinch, and RCS for Twilio and Sinch.

- **Input.** No screen transition names `ChatBridgeServices.ingest#Event`: services and code in the server call it (an
  adapter, after its
  own checks of a webhook's signature or a gateway's session; [services and
  `allow-remote`](Attack-Surface-Web.md#services-and-allow-remote)). It authenticates the bridge by its inbound token
  (`check#BridgeToken`: the SHA-256 against `ChatBridge.inboundTokenHash`, in constant time; a bridge with no hash takes
  none) and refuses the whole batch, storing nothing, for an unknown or disabled bridge, a wrong token, a malformed
  transaction id, more than 200 items or 1 MB, or an item that names a local user. An item posts only as an external
  actor (no login, no `UserAccount`) into a room an administrator mapped to that bridge (`noMapping` otherwise), as a
  member a moderator has not taken out, edits and deletes only its own messages, and carries plain text (escaped to
  HTML). An item's `attachments` are refused (`attachmentsUnsupported`); its `media` (up to 10 an item) is fetched and
  stored by `store#BridgeMedia`, with a type from a fixed list (else `application/octet-stream`) and a name with no
  path part. `transportVerified` is honored only under the context mark of that bridge. A puppet link to a local user is provenance, never authority. Its messages fetch no link
  previews, reach a room's agents only under the room's `agentsReadExternal`, and never start the framework's agents.
- **Output.** A room's messages go only to the networks an administrator mapped it to; a link to an external room grants
  no rights. Delivery rows keep no message text. Two other entities do: a refused queued webhook is kept whole in
  `ChatBridgeWebhookRefused.requestJson` for 30 days (the body, the headers but cookie and authorization, the
  signature header), at most 1,000 a bridge, and a batch keeps its items in `ChatBridgeInbound.batchJson` until it is
  done. No service answers them; the database and the entity tools read them. A send that could not reach the provider
  keeps the HTTP client's error as the delivery's `lastErrorText`, and `get#EventDeliveries` answers it as `error` to
  any reader of the room (`get#Timeline` gives each message's `delivery`): the text is `HTTP POST <scheme://host/path>
  failed`, and the path of a Twilio or Sinch send holds the account SID or project id. The delivery worker calls an
  adapter's `start` once a round and, when it fails, only sets the bridge's status to not connected; `deliver_head`
  holds a row only for a disabled bridge or a missing adapter, and `send#BridgeDirect` (the system's texts) calls
  `adapter.deliver` without `start`. A sent text carries the bridge's status callback URL to the provider, and a file
  goes out as `/channels/{id}/media/{attachment}.{expiry}.{signature}`: the token (an HMAC-SHA-256 over bridge,
  attachment and expiry) is the only control of that route, and the file must belong to a message with a delivery to
  that bridge; it is served as an attachment with `nosniff`.
- **Webhooks.** On a full node an unknown bridge, a missing adapter and a disabled bridge answer 401 before any parse,
  and the adapter verifies the signature before the replay cache and the parse: Telnyx Ed25519 over `timestamp|body`,
  Twilio HMAC-SHA1 over the configured public URL and the sorted form (never from a `Host` header), Sinch HMAC-SHA256
  over `body.nonce.timestamp`. The replay window is the smaller of the adapter's tolerance and half the replay cache's
  life (`chat_bridge_web.rs`; the cache lives 300 seconds by default, so 150 seconds either side for Telnyx and Sinch),
  and the cache is shared with the system-message HMAC check. Twilio's signature has no time in it: a copy is caught
  by the replay cache while it holds it, and after that by the ids. A text is deduped for good by its
  `ChatEventExternal` row; a STOP or START keyword is applied before that, and its once-only mark is a
  `ChatBridgeInbound` row with status done, which the worker deletes after 30 days. On an edge node (or an all-in-one
  node for a paired node's bridge) the Twilio and Sinch adapters' signatures cannot be checked, so
  `chat_bridge_edge.rs` queues any POST that has the signature header, whatever its value, within 120 a minute a
  client address and 600 a minute a bridge (at most 10,000 live buckets, the client's taken first); there a bridge
  that exists answers 200, 400 or 429, a disabled one 503 and an unknown one 401, and none of these counts for web
  protection. The local node verifies the signature again (`apply#EdgeWebhook`, only for the queue's delegate user with
  the item's receipt), with the time the edge node wrote as the time checked.
- **Texts that come in.** A text is taken only for a number whose `messagingBridgeId` is the webhook's bridge. A text
  from a number not seen before makes a pair room, a member row for each user the number reaches and a joined push
  unless the spam filter hid it; only new group rooms are counted (below). Its files (each up to
  `maxInboundMediaBytes`, 5 MiB by default) are written under `chat@attachment-root` with no uploader, so no user's
  daily upload quota counts them. An inbound text runs app providers as a staff user: `file#ExternalConversation`
  signs in the room's first enabled local member (`internalLoginUser`) and calls `call#Providers` with the hook
  `external-conversation` (`mutation: true`), and the spam filter calls the `call-context` hook the same way for a
  first contact. With `VoicePolicy.smsSpamLookup` Y a first contact also causes a paid Twilio Lookup, at most
  `smsSpamLookupMaxPerDay` (100) a day. A Twilio bridge's file fetch carries the API's Basic credentials to the URL in
  the signed webhook's `MediaUrl{N}`, whatever its host (the header is dropped on a redirect to another host), and
  the paging of `ParticipantConversations` follows a `next_page_url` that starts with `conversationsBaseUrl` as a
  string. A group text makes, deletes and hooks Twilio Conversations on the provider account with the bridge's
  credentials (`providers/twilio/group.rs`).
- **Spam and blocks.** System › Voice `Spam` has five transitions to `SmsSpamServices` (`update#SpamFilter`,
  `store#SmsSpamRule`, `store#SpamVote`, `delete#SmsSpamRule`); each checks `VoiceAdmin`. Chat's `notSpam` and
  `reportSpam` need a current member row of a phone room and change the room for every member (`SmsSpam.js`: `notSpam`
  sets `notSpamDate` and clears each member's `CmfSpam`; `reportSpam` puts teammates' rooms in Spam). `roomBlock` is
  Chat's third.
- **Administration.** `find#Bridges`, `store#Bridge`, `store#RoomBridge` and `end#RoomBridge` need `ChatBridgeAdmin`.
  `ChatBridge.configJson` is an encrypted field and never returned; the inbound token is kept only as its hash.
  `retry#Delivery`: the message's sender, a moderator of the room, or `ChatBridgeAdmin`; anyone else is told "not found"
  or "not allowed" with the same parameters.
  `ChatBridgeAdmin` is granted to `ADMIN` by `ChatBridgeSetupData.xml`. The Chat screen's `bridgeChoices`,
  `storeRoomBridge` and `endRoomBridge` transitions call `find#Bridges`, `store#RoomBridge` and `end#RoomBridge` with
  authorization disabled; each service checks the permission itself. System › ChatBridges has three transitions,
  `setDisabled` (`store#Bridge`), `retryDelivery` and `replayRefused` (`replay#RefusedWebhooks`, which checks
  `ChatBridgeAdmin` too); no screen edits a bridge's `configJson`, name or token, and `store#Bridge` replaces
  `configJson` whole and checks only that it is JSON. The SMS adapter
  (`crates/moqui-channels/src/sms.rs`, Telnyx's settings in `providers/telnyx.rs`) reads from the bridge's
  `configJson`: `apiKey` (sent as `Authorization:
  Bearer`), `apiBaseUrl` (`https://api.telnyx.com` by default), `publicKey`,
  `webhookToleranceSeconds`, `maxInboundMediaBytes` (at most the adapter's 20 MiB cap) and `allowInternalAddresses`.
  The other adapters' secrets are in `configJson` too: Twilio `authToken` and `apiKeySecret`, Sinch `keySecret` and
  `webhookSecret`. `start` refuses an `apiBaseUrl` that is not `https://` (`http://` only with
  `allowInternalAddresses`); `deliver` and `send` have no scheme check of their own, and a bridge whose `start` failed
  still sends.
  Files a text names are fetched from public addresses only (3 redirects, each checked, 30 seconds, 20 MiB) unless
  `allowInternalAddresses` is set off a production instance (`sms.rs`: a production instance ignores it), which lifts
  the address guard for the API calls and the media fetch of that bridge ([the internal-address
  guard](Attack-Surface-Outbound.md#the-internal-address-guard)).
- **Group texts and phone names** (plans/Comms-Messaging.md §9). Text a number (`open#PairRoom`, Chat's `textRoom`,
  Calls History's `textNumber`) takes up to 8 numbers, comma-separated; each is checked as a send to it is
  (`check#SmsSend`) before the group's room is made, and a group's reply checks each number again before the one group
  MMS goes, each recipient counted toward the day's limits. A group MMS that comes in is the group's room only when our
  number is among its recipients on that bridge; its sender is always kept, our own numbers are never its others, a
  short code keeps a room of its own, and an outside number starts at most 5 new group rooms a day with one of our
  numbers (50 for the number; past that, the sender's pair room), so a sender cannot flood the number's users with
  rooms. `find#PhoneNames` (Chat's `phoneNames`, a GET) answers, for up to 50 E.164 numbers, only those of phone rooms
  the caller is in (their mappings), at most 600 numbers an hour a user (`moqui.sms.names.asked`), the first contact name
  the apps' `call-context` providers give the caller (`call#Providers` as the user, the apps they may open); it takes no
  other user's id, so a user learns of a number only what their own apps would show them for a call from it, and
  cannot read an app's contacts by guessing numbers. `open#PairRoom` has no count or rate: each call with a new number
  makes a room and member rows for all the number's users and pushes "joined" to them, and it makes the number one of a
  phone room the user is in, which is all `find#PhoneNames` asks, so the bound on what a user with a text number reads
  of an app's contacts is the providers' own answer for that user and the 600 an hour.
- **Internal services.** `send#Message`, `edit#Message`, `react#Message` and `redact#Message` take `senderActorId` only
  with authorization disabled. `get#LocalActors`, `get#ActorNames`, `get#RoomBridges`, `check#BridgeSupport`,
  `get#BridgeCapabilities`, `wake#BridgeDelivery`, `run#BridgeDeliveries` and the
  `ChatBridgeServices` internals are written for callers in the server. The attribute that says so is not enforced.
  The internals are `store#BridgeMedia`, `send#BridgeDirect`, `lookup#BridgeNumber`, `store#MappingThread`,
  `note#DeliveryReceipt`, `note#DeliveryPath`, `file#ExternalConversation` (`authenticate="false"`), `apply#EdgeWebhook`
  and `store#EdgeMedia`; `SmsServices` `ensure#PairRoom`, `check#SmsSend`, `note#SmsSent`, `release#SmsSend`,
  `note#SmsKeyword`, `note#SmsOptSignal` and `receive#Sms`; `SmsSpamServices` `judge#SmsText` and `note#SmsJudged`
  (`authenticate="false"`) and `purge#SpamRooms` (`authenticate="anonymous-all"`). No screen transition, REST file or
  token route names any of them (grep of `runtime/components`). `apply#EdgeWebhook` refuses any caller but the edge
  queue (`called_by_queue`); `store#EdgeMedia`, which only the queue kind `chatMedia` names, makes no such check and
  reads the `contentLocation` it is given, to serve at the bridge's media route for the `tokenHash` and `chatBridgeId`
  its caller names. `org.moqui.SmsServices.send#SmsMessage` is `authenticate="anonymous-view"` and takes `fromNumber`
  and `userId` from its caller; its callers are `UserServices.send#AuthcCodeSms` (which maps `contactNumber` and
  `message` only) and mr-mantle's `MessageDeliveries.js`, and no transition names it.

## Conf-level shell providers

A component's MoquiConf.xml may register a provider of the hooks the framework asks for whoever acts (`screen-facade/shell-providers`; only `sms-consent`, `CONF_PROVIDER_HOOKS`; any other element is logged and left out). call#Providers asks it after the apps' providers for every caller, the system with no user included, with authorization disabled, since the deployment's conf names it: such a provider answers only what its hook asks; conf providers are always asked beside an app's provider of the same name, a "no" from any provider wins, and a provider that fails refuses the text (Mantle's check#SmsConsent reads consent and suppression rows and answers yes or no, nothing else). A promotional text counts against its number's day of promotional texts (VoicePolicy smsMaxPromotionalPerNumber, 1000), and one with no user not against the system's own budget, so a campaign cannot use up sign-in codes. Marketing's texts (the ledger's SMS channel, MdcSms: each attempt is handed to the framework's `org.moqui.SmsServices.send#SmsMessage`; the consent answer and STOP handling are `mantle.party.MessageSmsServices` check#SmsConsent, which mr-mantle's `MoquiConf.xml` names as the `sms-consent` provider, and note#SmsKeyword; no screen transition names either) are sent as the system from an SMS sending profile's number, which must be one of the organization's telecom numbers that is one of ours with a messaging bridge (MESSAGE_SENDING_CONFIGURE sets it); each text is checked for `PcpSms` consent on the send's topic and the SMS suppression before the call, and again by the framework's sms-consent hook (yes only for a party holding the number with PcpSms granted with an organization that owns the sending number and no suppression there); a STOP the framework keeps becomes a PcpSms withdrawal and a suppression (an EECA on SmsOptOut, in its own transaction), and a START ends neither; a lost answer is uncertain and never resent; no link, token or tracking in a text. PASS Email preferences may leave a text message topic, never join one.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- `crates/moqui-rtc/src/sfu/` and `crates/moqui-host/src/rtc/mod.rs`, `media.rs` and `driver.rs` were not read: what the SFU lets a leg send, and so whether one participant can make a recording fail by filling the recorder's queue (4,096 frames) or reach its total size with high-bitrate video, was not settled. `push_crypto.rs` `encrypt_with` and the VAPID signer (the token's audience and lifetime), `moqui_lib::http::RestClient` (how the timeout, the resolver guard and the redirect policy are applied; whether the push timeout bounds the whole of a slow answer), the edge queue (`crate::edge::queue`: its size limits, how long a delivered item's idempotency key is kept, that `contentLocation` is always the queue's own copy) and the tests of these areas were not read.
- BasicComms: the `Settings` screens `Index`, `Calls`, `Chat`, `Devices`, `Identities`, `Rules`, `OutOfOffice` and `TextBlocks`, the `Home` and `Panel` screens, the `*Tui.xml` screens, `template/FilePickerTransitions.xml`, `ComposeServices.xml`, `CommsHomeServices.xml` and `FilePickerServices.xml` were not read. `ChatScreenServices.xml` and the `Calls`, `Inbox` and `Settings` screens were read only for their transition names, authorization calls and file responses. `Settings/Chat.xml` was read for lines 22-100 (chat preferences, each with an explicit `in-map`); its one whole-context call, `applyNotifyDefault`, was not followed into `apply#NotifyDefault`.
- `Chat.qvue`, `Call.qvue` and `CallGuest.qvue` were not read: what the clients do with relayed call frames, guest names and a forwarded message's body (`v-html`) was not checked. Nor was how they show a caption's `text` and `name` (as text or as HTML), an external sender's `displayName`, a delivery's `error` or an RCS message's `metadata`. `ChatExternalUser.avatarUrl` is stored from a bridge's item (up to 1,024 characters, any scheme), and no service that was read returns it.
- mr-boss's own people search (`ironstone.shared.ChatterServices.search#Users`) was not read. Its `get#CallTarget` (`chat-target`, `call-target`) refuses a user whose party the Comms grant's entity filters (`MANTLE_USER_ORG`) keep out.
- `mantle.party.CommunicationServices.get#RecordLinkInfo`, which mr-boss's `record-lookup` provider reaches with the request's `recordEntityName`, was not read: whether it takes any entity name or a fixed list. `ChatServices.create#Room` for `CrtRecord` when no `record-lookup` provider answers (the caller's entity string) was not read.
- Whether a guest who waits in the lobby longer than `calls@ring-seconds` can still be admitted was not traced (`expire#Calls` closes an offered leg after that time).
- Whether `web-protection` matches `/call/*/joinGuest` against the token route's path as written was not read (the guard's matcher), and whether its `max-per-client` connection cap applies to `/callws` was not traced.
- `docs/interfaces/Chat-Keys.md`, `WebRTC.md`, `SMS.md`, `Chat-Bridges.md` and `RCS.md` were not compared with the code statement by statement.
- TURN: the `turn-server-proto` crate was not read (whether it compares MESSAGE-INTEGRITY in constant time, or answers an unauthenticated request with a reply larger than the request). stun-proto's `LongTermServerAuth` keeps a nonce per client address and never removes one, so UDP requests from many forged sources grow it. The rest of `turn_probe.rs` and the TURN tests were not read.
- TURN: whether a relay is ever bound to an IPv6 address was not settled; `is_public` takes `internal_address`'s IPv6 rule and refuses NAT64 whole ([Outbound](Attack-Surface-Outbound.md#the-internal-address-guard)).
- XMPP: the edge link's authentication (`edge_link/mod.rs`, the accept side), on which the tunneled client address rests, was not read. How the HTTP listener computes `ClientAddr`, `crates/moqui-host/src/xmpp/upload.rs` (the `/xmpp-upload` route) and its web protection, and `moqui_lib::source_guard` were not read.
- SIP: whether rsipstack takes a server dialog's remote target from the INVITE's Contact (`get_or_create_server_invite`), what `InviteDialog::handle` checks of an in-dialog request besides its CSeq, and whether rsipstack sends a BYE to the Contact of a call that was answered and never ACKed (a forged trunk-address INVITE over UDP would then aim a request without credentials) were not read. How long rsipstack keeps a server transaction this side never answers, and whether `incoming_sender` or the transaction table has any bound, were not read.
- SIP: `ua.rs` lines 1-379 and 1340-1424 (the REGISTER to a trunk, `send_request`), and the host's `leg.rs` outside the worker's loop, `ivr.rs`, `edge.rs`, `owners.rs`, `trunk_lock.rs`, `prompt.rs`, `audio.rs`, `voicemail.rs` and `phones.rs` were not read. Whether a leg sends RTP before it is answered was not read. What bounds the pool that runs a leg's `note#PhoneLeg` calls was not read. Whether the host keeps rsipstack's INFO lines (whole messages) out of the log was not read past the comment in `logging.rs` ("rsipstack at WARN").
- Whether a span or an `ArtifactHit` records service parameters (a `pin` or `ha1` passed to a service) was not settled; a grep of `crates/moqui-core/src/service` found none.
- SIP: `VoicemailServices.expire#VoicemailTrash` is `authenticate="anonymous-all"`, takes no input and deletes only trash past `voicemail@trash-days`. Which callers name it was not traced.
- SIP: `receive#DeviceCall` and `refer#Call` call `internalLoginUser` inside a request-pool job of the engine. Whether each job gets a fresh execution context was not traced.
- SIP: opus-rs was not read: whether any packet reaches one of its unchecked indexes out of range was not settled. The WAV and Ogg readers of uploaded greetings and prompts (`crates/moqui-host/src/sip/audio.rs`, `prompt.rs`) were not read.
- SIP: `CallHandling.js` `evaluate` builds any user's call plan with no ownership check. No screen or script under `runtime/components/base`, `addon` or `dev`, or in `crates`, names its service by grep; other callers were not traced. The bodies of `CallHandling.js` (plans and routing), `CallQueues.js`, `CallTransfers.js` and `Ivr.js` beyond their guards were not read.
- Voicemail: a box's `notifyEmail` is any address the box's user types, with no confirmation; with `attachAudio` each message's audio is mailed there. Whether the `EmailMessage` rows made this way are rate-limited or audited was not read.
- Voicemail: whether the phone-access answers tell an existing box from a number with none after a block was not settled.
- Recording and captions: whether an account gets a personal drive with a quota by default, which decides how much the recording quota does, was not read. Captions keep running, on the starter's quota, after the starter leaves the call; whether `Calls.js` stops them when that user's last leg leaves was not read. A captions thread that panics leaves its room in `calls_running`, so a later start only sends the notice again until someone stops captions.
- Bridges and texts: whether room retention deletes `ChatEventExternal` rows (an old signed Twilio text sent again would then appear as a new message) was not read. Whether web protection's request limits apply to `/channels/` answers that are not counted as failed sign-ins was not read. `Voice.js` `grants`, `classify` and `toE164`, on which every send check rests, `CallHandling.js` `blockNumber` and `blockedUsers`, and `ChatBlocks.js` were not read. In `SmsSpam.js` a `call-context` provider that fails, or a first user who cannot be signed in, counts the sender as known, so the text is not judged; whether that can be caused from outside was not traced. `send#AuthcCodeSms` spends the system's daily budget (`smsSystemMaxPerDay`, 500); whether an anonymous caller can drive it is the login flow's question.
- Device pushes: on which thread a call's ring push (`send#WebPush`, which loops the callee's subscriptions) runs was not read. Whether a disabled or revoked account's browser subscriptions are ever removed short of logout (whether `revoke#AccountAccess` reaches `forget_visit`) was not settled, nor what the service worker and the app show for a push of a deleted message.
- XMPP: a `parse` call of rxml that answers `NeedMoreData` consumes its input (`framing::tests::a_start_tag_cut_inside_its_attributes_counts_toward_the_limit` fails without the start tag's count). Whether two usernames that differ only by case can exist (`create#UserAccount`) was not read. Whether `send#Message` refuses a plain body in a room that became end-to-end encrypted after the session cached it was not read (`session/chat.rs` reads the cache; `store#EncryptedMessage` checks the room itself). Whether a pool worker's login from `internal_login_user` ends with each call was not read past the comment that says it ends with its context.
