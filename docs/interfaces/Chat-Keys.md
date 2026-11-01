# Chat keys

End-to-end encryption for chat: each browser holds a crypto device, and the server keeps its public keys and passes
Olm ciphertext between devices. Encrypted chat rooms carry Megolm ciphertext over it, their room keys are backed up to a
recovery key the user keeps, and devices are verified by emoji or QR code with cross-signing. Encrypted calls (W8) send
their per-sender call keys over the same device layer. The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md)
§4.5, milestone C7.

- **Browser:** `webroot/screen/webroot/js/WebrootCrypto.js` (`moqui.crypto`), over matrix-sdk-crypto's `OlmMachine`
  (the vendored `@matrix-org/matrix-sdk-crypto-wasm` 18.9.0, `/libs/matrix-sdk-crypto/`).
- **Services:** `org.moqui.impl.ChatKeyServices` and `ChatKeys.js` (framework), with `ChatDevice`, `ChatDeviceKey` and
  `ChatToDevice` in `framework/entity/ChatKeyEntities.xml`, with the cross-signing and backup entities; the `/ChatKeys`
  transitions (webroot `ChatKeys.xml`).
- **Encrypted rooms:** `org.moqui.impl.ChatServices` (`ChatRoom.encrypted`, `ChatEvent.encrypted`: the C7 block of
  `ChatEntities.xml`); in the browser `moqui.chat` (`js/WebrootChat.js`: `sealMessage`, `openEvents`, `openEvent`) and
  BasicComms Chat (`Chat.qvue`).
- **Screens:** BasicComms Settings, My devices (`Settings/Devices`, `Devices.qvue`, `DevicesTui.xml`): devices,
  verification and the key backup; Chat's New room (Encrypted) and room settings.

## Devices

A device is one browser's crypto store for one user, as in Matrix: devices belong to users, not to chat actors.

- **Made on first use.** `ensureDevice()` asks the server for a device (`create#ChatDevice`: a 10-letter deviceId) the
  first time this browser needs one for the user.
  - The store is IndexedDB, named `moqui-crypto-<userId>-<deviceId>`.
  - The deviceId is kept in localStorage under the userId, so the next page opens the same store.
- **Nothing loads until needed.** The 7.8 MB WebAssembly module loads only when a caller asks for the device; pages that
  never do load none of it.
- **Keys.** The machine uploads its signed identity keys and one-time keys (`upload#Keys`). A device's identity keys
  cannot change once set.
- **Revoking.** Settings, My devices removes a device (`revoke#ChatDevice`):
  - it gets no new keys or messages;
  - its keys and the messages waiting for it are deleted;
  - it drops out of key queries.

  A browser whose device was revoked deletes its store and makes a new device the next time it needs one. Revoking does
  not sign the browser out.
- **Logout and account switch.** Logout and leaving the page close the store. A store belongs to one user and device,
  and a page of another user never opens it. The server refuses a device that is not the session user's ("not found"),
  so a tab left open across an account switch gets nothing; that tab only closes the store (it deletes it only when the
  session is still the store's user).
- **One tab holds the store; the others ask it.** A tab holds a Web Lock on the store while it is open: two
  `OlmMachine`s on one store would spoil its Olm sessions.
  - That tab is the leader. The user's other tabs of the browser are followers: they send their calls (encrypt,
    decrypt, send to devices, backup, verification) to the leader over a `BroadcastChannel` named
    `moqui-crypto:<userId>:<deviceId>:<tag>`, and hear its to-device events, new room keys and verifications. `state.role`
    says which a tab is. The tag is the first 128 bits of SHA-256 of the device's store key, so a tab finds the channel
    only after fetching that key as the device's signed-in user: a page of another account in the same browser profile
    (which can read the deviceId from localStorage) cannot ask the leader to decrypt or share keys. The leader answers
    only calls of the API's own functions, with an array of arguments. Without Web Crypto there is no relay: each tab waits
    for the store.
  - Each follower waits for the lock. When the leader's tab closes (or closes its store), one of them takes the store
    and leads; calls still waiting are asked of it. A call not answered in 30 seconds is refused (`busy`).
  - Encrypted files are encrypted and decrypted in any tab: that needs the module, not the store.
- **At rest.** The store is encrypted with its store key, a random passphrase the server keeps for the device
  (`ChatDevice.storeKey`, an encrypted field). It is given only to the device's own signed-in user (`/ChatKeys/storeKey`)
  and dropped when the device is revoked. A copy of the browser profile alone does not open the store; with a signed-in
  session it does. A store made before the store key (unencrypted) does not open with it: the browser makes a new device.
  - What it does not protect against: anyone who can sign in as the user (or holds a stolen session) together with the
    browser profile; whoever reads the server's database with its field-encryption key, or an administrator with entity
    data access in Tools (the field is encrypted at rest on the server, not hidden from those tools); and script running in
    a page of this origin while the user is signed in. It is a lock against a copied or stolen profile, not against the
    server.
- **A refused key upload.** The machine's upload is not retried as it is: a device whose keys the server refuses
  (`keysChanged`, `keyExists`, `badKeys`, `tooLarge`) is revoked and made again once; a second refusal stops this page's
  uploads (`state.status` error). `tooManyKeys` stops uploads until keys are claimed.

## Who reaches whom

A user's key queries, claims and to-device sends reach their own devices and the devices of users who share a room (both
members now) or a live call (not ended, neither denied nor removed) with them. Others are left out as if they had no
devices.

- **One-time keys.** A claim locks the device's row and takes its oldest unclaimed one-time key, so two claims at once
  never get the same key. With none left, the device's fallback key answers.
- **To-device messages.** Each is queued per device at the device's next stream position, and kept until the device
  acknowledges it. A sync from a position deletes everything at or before it.
  - A queued message wakes the recipient's tabs with `{kind: 'toDevice'}` on `chatUser:<userId>`. The push carries
    nothing of the message.
  - A retried send whose transaction ID is still queued is not queued again.
- **Device lists.** A sync answers the users in reach whose devices changed since the last one, so the machine queries
  them again.

**The server keeps** public keys, Olm ciphertext and routing only. Plaintext and private keys never leave the browser.

**Encrypted calls (W8)** use this reach as it is: their browsers send each other their SFrame keys as the to-device event
`io.moqui.call.key`, to the devices the call's roster names ([WebRTC, Encrypted calls](WebRTC.md#encrypted-calls)).
Revoking a device also takes its legs out of the encrypted calls it is in, so the others rotate their keys without it.
Call guests have no device: an encrypted call takes no guest, so the reach rules are unchanged.

## Endpoints

The services take and answer Matrix's payloads, so the Matrix client API (C6a) can serve the same services at
`/_matrix/client/v3/…`.

| `/ChatKeys/…` | Service | Matrix |
| --- | --- | --- |
| `device`, `devices`, `rename`, `revoke`, `whoami` | `create#`, `find#`, `update#`, `revoke#ChatDevice`, `get#ChatKeyIdentity` | login's device, `devices`, `account/whoami` |
| `upload` | `upload#Keys` | `keys/upload` |
| `query` | `query#Keys` | `keys/query` |
| `claim` | `claim#Keys` | `keys/claim` |
| `send` | `send#ToDevice` | `sendToDevice/{eventType}/{txnId}` |
| `sync` | `sync#ToDevice` | `/sync`: `to_device`, `device_lists` (`changed`, `left`), `device_one_time_keys_count`, `device_unused_fallback_key_types` |
| `storeKey` | `get#ChatDeviceStoreKey` | none: the store passphrase |
| `signingUpload` | `upload#CrossSigningKeys` | `keys/device_signing/upload` |
| `signaturesUpload` | `upload#KeySignatures` | `keys/signatures/upload` |
| `backupVersionCreate`, `backupVersion`, `backupVersionUpdate`, `backupVersionDelete` | `create#`, `get#`, `update#`, `delete#KeyBackupVersion` | `room_keys/version` (POST, GET, PUT, DELETE) |
| `backupKeysPut`, `backupKeys`, `backupKeysDelete` | `store#`, `get#`, `delete#BackupKeys` | `room_keys/keys[/{roomId}[/{sessionId}]]` (PUT, GET, DELETE) |

The key counts (`one_time_key_counts`, `device_one_time_keys_count`) always hold `signed_curve25519`, 0 when none is
left: the machine updates its count only from an algorithm the answer names, so a device whose keys were all claimed
would otherwise never upload more. A sync's `since` past the last position queued counts as that position.

- **Request shape.** Each is a POST with a JSON body (`devices`, `whoami`, `backupVersion` and `backupKeys` also take a
  GET) and the session's CSRF token. Matrix's path parts are body fields: `version`, `roomId`, `sessionId`.
- **Device.** The device is named by `deviceId` in the body, where Matrix takes it from the access token.
- **Errors.** A refusal answers `{errors, reasonCode}` and the refusal's own fields: `notFound`, `revoked`,
  `keysChanged`, `keyExists`, `badKeys`, `noMasterKey`, `tooManyKeys`, `tooManyDevices`, `tooLarge`, `tooMany`,
  `badRequest`, `algorithmChanged`, `wrongVersion` (with `current_version`); in the browser also `busy` (another tab has
  the store).

**Store key.** `storeKey` (`{deviceId}`) answers `{storeKey}`, the passphrase the browser passes to
`OlmMachine.initialize` as `store_passphrase`, so the IndexedDB store is encrypted: 43 characters of URL-safe base64,
made on the first ask and the same after. Only the device's own user gets it, so a copied browser profile without a
signed-in session cannot open the store. A revoke clears it, so a revoked device's store cannot be opened again. It is
never logged and in no other answer.

**Cross-signing.**
- `signingUpload` takes `{master_key, self_signing_key, user_signing_key, reset}`, each key a Matrix
  `CrossSigningKey` of the session's user with one ed25519 key, and answers `{changed}`. The session stands for Matrix's
  user-interactive auth:
  - the first upload is allowed, and the same keys again change nothing;
  - a different master key is refused (`keysChanged`) unless `reset` is true, the user's explicit "reset my identity".
    A reset deletes the old keys and every signature by or over them;
  - without `master_key` the user must have one (`noMasterKey`). A self-signing or user-signing key must carry a
    signature by the master key.
- `signaturesUpload` takes `{signed}`, where `signed` is Matrix's request body: Matrix user ID to key ID (a device ID
  or a cross-signing public key) to the signed object. It answers `{failures}`, Matrix user ID to key ID to
  `{errcode, error}` for each key refused. Only signatures by the caller's own keys are stored, where that key may sign:
  - the caller's devices by their self-signing key;
  - their master key by their devices;
  - their self-signing and user-signing keys by their master key;
  - the master key of another user in reach by their user-signing key.
- The server checks who may sign what, not the signatures themselves; the clients verify them.
- `query#Keys` answers `master_keys` and `self_signing_keys` of the users in reach, and `user_signing_keys` of the
  caller only. The stored signatures are merged into each signed object's `signatures`: a user's own to everyone in
  reach, another's only to the signer.
- A change of a user's cross-signing keys or signatures is a device-list change, which syncs name in
  `device_lists.changed`.

**Device lists left.** A sync's `device_lists.left` names the users the caller was in reach of through a room and no
longer is (no shared room, no live call), because a membership ended since `deviceListSince`. The client stops
tracking them.

**Key backup** (`m.megolm_backup.v1.curve25519-aes-sha2`; the server stores `session_data` as the browser made it).
- **Versions.** Versions are `"1"`, `"2"`, … per user and never reused.
  - `backupVersionCreate` takes `{algorithm, auth_data}` and answers `{version}`; the new version is the current one.
  - `backupVersion` takes `{version}`, or nothing for the current (the latest not deleted). It answers
    `{algorithm, auth_data, count, etag, version}`, or `notFound`.
  - `backupVersionUpdate` takes `{version, algorithm, auth_data}`; the algorithm cannot change (`algorithmChanged`).
  - `backupVersionDelete` takes `{version}`: the version is marked deleted and its sessions go.
- **Keys.**
  - `backupKeysPut` takes `{version, rooms}`, `rooms` as Matrix's body (room ID to `{sessions}`, session ID to
    `{first_message_index, forwarded_count, is_verified, session_data}`), and answers `{count, etag}`. Only the current
    version takes keys: another is `wrongVersion` with `current_version` (Matrix's `M_WRONG_ROOM_KEYS_VERSION`).
  - A session already there is replaced only by a better one: a verified one, then a lower `first_message_index`, then
    a lower `forwarded_count`.
  - `backupKeys` takes `{version, roomId, sessionId, offset}` and answers `{rooms, has_more}` in that shape, empty when
    nothing matches; at most 10,000 sessions from `offset`.
  - `backupKeysDelete` takes `{version, roomId, sessionId}` and answers `{count, etag}`.
- **Etag and room IDs.** `etag` is a string that changes with the version's keys. Room IDs are the Matrix room IDs the
  browser names (`!<chatRoomId>:<chat_server_name>`), opaque to the server, at most 255 characters.
- **Bounds.** `session_data` up to 16 KB each, 1,000 sessions in one put, 100,000 held by a user (`tooMany`).

**User IDs.** A Matrix user ID is `@<localpart>:<chat_server_name>`.
- The localpart is the userId with Matrix's mapping of other character sets: an upper-case letter is `_` and the letter,
  `_` is `__`, and any other byte is `=xx`.
- Set `chat_server_name` (default `localhost`) before the first device. A change makes every device new, and C6a needs
  it to be the domain.

## Encrypted rooms

A direct or private room may be made encrypted (`create#Room` with `encrypted`; New room, Encrypted), and that cannot be
turned off. A public or record room is never encrypted (`encryptedRoom`), and an encrypted room stays members only. An
encrypted direct room is a room of its own beside the plain direct room of the same people.

**What the server keeps.** A message, edit or reaction is the Megolm ciphertext the sending browser made (the
`m.room.encrypted` content, in `contentJson.encrypted`, `ChatEvent.encrypted` Y). In the clear, as in Matrix:
- the relation targets: `replyToEventSeq`, `threadRootEventSeq`, `editsEventSeq`, `relatesToEventSeq` (what an edit,
  reply, reaction or redaction points at);
- the mentioned users (`mentionUserIdList` on send and edit): they are counted and notified, without text.

Bodies, reaction keys, file names and types are in the ciphertext. No `bodyText`, snippet or notification summary is
written; the room list and the terminal show "Encrypted message". A file is uploaded encrypted and kept as an opaque blob
named `encrypted`, `application/octet-stream`; its name, type, key and hash travel in the message (`io.moqui.files`, each
with Matrix's `EncryptedFile` info). The Files pane lists them without names.

**Refused in an encrypted room** (`reasonCode` `encryptedRoom`, each spec'd with the parameters that pass in a plain room):
- a plaintext body, edit or reaction key (and ciphertext in a plain room: `notEncrypted`);
- server search of the room, link previews, agent members (`add#Member` and `set#MemberRole` with `CmrAgent`), assist
  cards and agent delivery;
- polls; forwards into or out of the room; records on a message;
- Create from and File on record (framework transitions, and Mantle's `findOrCreate#ChatCommunicationEvent`), Chatter's
  chat source (Mantle `get#RecordTimeline` leaves it out), BOSS Home's snippets ("Encrypted message");
- bridges (`store#RoomBridge`); XMPP serves no encrypted room (C3a); call-link chat (W8);
- the terminal's send ("open it in a browser").

**Keys and rotation.** Before a message, the sending browser shares the room's Megolm key with every device of each
member that has none (Olm to-device, `encryptRoom`). The machine starts a new session when a member or a device it shared
with is gone (it compares with the members the page gives), and the room page starts one at once when a member leaves or
is removed (`rotateRoom`). A device revoked by its user drops out of key queries, so the next message's session leaves it
out. `device_lists.left` tells a browser to stop tracking users it shares no room with. Rotation cannot take back what a
device already holds. A member who joins later reads earlier messages only if a member's device shares those keys, or
from their own key backup.

**Decrypting.** The room page decrypts each page of the timeline and each pushed event before it is shown
(`moqui.chat.openEvents`, `openEvent`). The browser decrypts an event as `$<chatRoomId>.<eventSeq>:<server>` in
`!<chatRoomId>:<server>`, as the server numbers it. An edit shows its own ciphertext (`encryptedEventSeq`); reactions come
as `encryptedReactions`, which the page decrypts and counts, and an encrypted reaction is taken back by its own
`reactionEventSeq`. Decrypted HTML goes through DOMPurify. A message whose key this device lacks says so, and is decrypted
again when keys arrive (`onRoomKeys`).

**No plaintext leaves the browser.** The outbox (localStorage) keeps the ciphertext; the text shown while a message waits
is kept in the page only. Drafts of an encrypted room are kept in the page, not in storage.

**Key backup.** Settings, My devices, Set up key backup:
- sets up cross-signing when the user has none (an identity made on another device is left as it is);
- makes a recovery key (`BackupDecryptionKey`, shown once in Matrix's encoding: base58 of 0x8B 0x01, the key and a
  parity byte, in groups of four);
- makes a backup version (`m.megolm_backup.v1.curve25519-aes-sha2`); from then on the machine backs up each room key,
  encrypted to the recovery key, to `room_keys/keys`.

A new device reads older messages only after Restore with the recovery key (or when another of the user's devices shares
the keys). A wrong key is refused (`wrongRecoveryKey`) and nothing is reset.

**Verification.** My devices shows each device's verification as this browser's store holds it (trust is the clients'):
Verify asks another device; the two compare emoji (SAS), or one shows a QR code that the other reads with its camera
(where the browser has `BarcodeDetector`). The steps travel as to-device events. A verified device of one's own is signed
with the self-signing key (`keys/signatures/upload`), so the user's other devices trust it too. `ChatDevice.verifiedDate`
stays unused: the server's flag would establish no trust.

## The browser API

In `moqui.crypto` (the store's tab runs each call; another tab's call is relayed to it):

| Function | What |
| --- | --- |
| `ensureDevice()` | Loads the module and the store, makes the device on first use, uploads its keys. Resolves to `{userId, deviceId, matrixUserId}` |
| `sendToDevices(targets, eventType, content)` | Olm-encrypts `content` as an event of `eventType` for each device of `targets` and sends it. `targets` is a list of userIds (every device of each) or `{userId, deviceIds}`. It never goes to this device. Resolves to `{sent, devices, skipped}`: `sent` is how many the server queued; `devices` and `skipped` are lists of `{userId, deviceId}` |
| `onToDevice(fn)` | `fn({type, content, senderUserId, senderMatrixId, senderDeviceId, senderCurve25519Key, verified})` for each to-device event that decrypted. Returns the unsubscribe |
| `devicesOf(userId)` | The devices of a user as this store knows them: `[{deviceId, displayName, ed25519Key, verified}]` |
| `sync()`, `close()`, `forget()` | A sync now; close the store; revoke this device and delete its store |
| `thisDeviceId()` | This browser's deviceId for the signed-in user, without loading anything |
| `state` | `{status, deviceId, matrixUserId, error, role, verifications}`; `status` is `idle`, `loading`, `ready` or `error`; `role` is `leader` or `follower` |
| `encryptRoom(chatRoomId, memberUserIds, eventType, content)` | Shares the room key with the members' devices that lack it, and resolves to the Megolm ciphertext of the event |
| `decryptRoom(chatRoomId, {eventSeq, sentDate, senderUserId, encrypted})` | `{type, content, verified}`, or `{error}` when this device has no key for it |
| `rotateRoom(chatRoomId)` | A new session for the room's next message |
| `onRoomKeys(fn)` | `fn([{roomId, sessionId}])` when room keys arrive (from a member, or a restore). Returns the unsubscribe |
| `encryptFile(blob)`, `decryptFile(data, info)` | `{blob, info}` for upload; the decrypted bytes (the hash is checked) |
| `enableBackup()`, `backupStatus()`, `restoreBackup(recoveryKey)` | `{version, recoveryKey}`; `{version, enabled, total, backedUp}`; `{version, imported, total, failed}` |
| `crossSigningStatus()`, `setupCrossSigning(reset)` | `{hasMaster, hasSelfSigning, hasUserSigning, identity, verified}` |
| `verifyDevice(userId, deviceId)`, `verification(flowId, action, arg)`, `dismissVerification(flowId)` | Start a verification (its flowId); a step: `accept`, `sas`, `confirm`, `mismatch`, `cancel`, `showQr`, `scanned` (the code's bytes), `qrConfirm`. `state.verifications` lists them with their emoji or QR image |

In `moqui.chat`: `sealMessage(roomId, memberIds, html, files)` (the send parameters `{encryptedJson, mentionUserIds}`),
`sealReaction`, `sealFile`, `openFile(url, file)` (a blob URL), `openEvents(roomId, list, attachmentUrl)`,
`openEvent(roomId, event, attachmentUrl)` and `myReaction(roomId, messageSeq, key, me)`.

## Settings

| Property | Default | What |
| --- | --- | --- |
| `chat_server_name` | `localhost` | The server name in device user IDs |
| `chat_to_device_retention_days` | `30` | How long a to-device message waits for a device that does not acknowledge it (the job `clean_ChatToDevice_daily`, which also deletes one-time keys claimed over a day ago) |

**Content Security Policy.** WebAssembly needs `'wasm-unsafe-eval'` in a policy that sets `script-src`. The default
policy sets none.

## Tests

- **Server:** `./moq test framework --spec ChatKeys`. It covers:
  - user IDs that map back, and identity keys that cannot change;
  - another user refused with the very same parameters;
  - reach by room and by live call;
  - concurrent claims (8 threads, 32 claims of 20 keys);
  - the queue's cursor, acknowledgement and repeated transactions, and a wake push with no content;
  - device-list changes, a revoked device, a member who left the room, and a sender the server sets.
- **Server, trust and backup:** `./moq test framework --spec ChatKeyBackup`. It covers:
  - a store key answered only to its own user, the same at each ask, absent from the device list, cleared by a revoke;
  - cross-signing keys uploaded once, the same again a no-op, a different master key refused unless reset;
  - signatures stored only by keys that may sign, refused out of reach and by another signer, and merged into key
    queries; a reset deleting every signature by or over the old keys;
  - backup versions, a put to another version refused with `current_version`, the better-session rule, keys by room
    and by session, deletes, counts and etags;
  - `device_lists.left` when the only shared room ends.
- **Server, encrypted rooms:** `./moq test basic-comms --spec BasicCommsEncrypted` (6 features). Every refusal above with
  the parameters that pass in a plain room; no plaintext in `bodyText`, the snippet, search, the ChatMention notification
  or the stored content; the timeline's ciphertext of an edit and its reactions; the terminal.
- **Browser, encrypted rooms:** `./moq test-client basic-comms --serve --spec EncryptedChatFlow` (three users, three
  pages; about 11 seconds with the server):
  - an encrypted message and an encrypted image both ways, the image shown from a decrypted blob; nothing posted, and
    nothing in localStorage, holds the plaintext;
  - a member who leaves cannot decrypt the next message, which is in a new session;
  - a second tab (a window of the same browser) reads through the leader, and takes the store when the leader closes it;
  - a new device of the same user reads nothing older until it restores the backup with the recovery key; a wrong key is
    refused.
- **Browser:** `./moq test-client basic-comms --serve --spec CryptoFlow`. Two Chromium pages of two users who share a
  room:
  - each makes a device;
  - one Olm-encrypts a to-device event, and the other decrypts it with the sender's device;
  - no body the sender posted holds the plaintext;
  - after the receiver revokes its device, a send reaches nothing.

  About 5 seconds.

**Not checked live:** Element or another Matrix client against these endpoints (C6a serves them), and Firefox and
Safari (the client specs run Chromium). Emoji and QR verification are built on the SDK's flows but no client spec drives
them, and reading a QR code needs a camera and `BarcodeDetector` (Chrome on Android, macOS and ChromeOS; not headless).
