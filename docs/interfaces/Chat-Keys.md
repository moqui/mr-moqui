# Chat keys

The device and key layer of end-to-end encryption: each browser holds a crypto device, and the server keeps its public
keys and passes Olm ciphertext between devices. Encrypted calls (W8) send their per-sender call keys over it. The design
is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §4.5, milestone C7; this is its core (C7 core). Encrypted
chat rooms, key backup, encrypted attachments and device verification come in a later C7 pass.

- **Browser:** `webroot/screen/webroot/js/WebrootCrypto.js` (`moqui.crypto`), over matrix-sdk-crypto's `OlmMachine`
  (the vendored `@matrix-org/matrix-sdk-crypto-wasm` 18.9.0, `/libs/matrix-sdk-crypto/`).
- **Services:** `org.moqui.impl.ChatKeyServices` and `ChatKeys.js` (framework), with `ChatDevice`, `ChatDeviceKey` and
  `ChatToDevice` in `framework/entity/ChatKeyEntities.xml`; the `/ChatKeys` transitions (webroot `ChatKeys.xml`).
- **Screens:** BasicComms Settings, My devices (`Settings/Devices`, `Devices.qvue`, `DevicesTui.xml`).

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
  so a tab left open across an account switch gets nothing.

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
| `sync` | `sync#ToDevice` | `/sync`: `to_device`, `device_lists`, `device_one_time_keys_count`, `device_unused_fallback_key_types` |

- **Request shape.** Each is a POST with a JSON body (`devices` and `whoami` are GETs) and the session's CSRF token.
- **Device.** The device is named by `deviceId` in the body, where Matrix takes it from the access token.
- **Errors.** A refusal answers `{errors, reasonCode}`: `notFound`, `revoked`, `keysChanged`, `keyExists`, `badKeys`,
  `tooManyKeys`, `tooManyDevices`, `tooLarge`, `tooMany`.

**User IDs.** A Matrix user ID is `@<localpart>:<chat_server_name>`.
- The localpart is the userId with Matrix's mapping of other character sets: an upper-case letter is `_` and the letter,
  `_` is `__`, and any other byte is `=xx`.
- Set `chat_server_name` (default `localhost`) before the first device. A change makes every device new, and C6a needs
  it to be the domain.

## The browser API

For encrypted calls (W8), in `moqui.crypto`:

| Function | What |
| --- | --- |
| `ensureDevice()` | Loads the module and the store, makes the device on first use, uploads its keys. Resolves to `{userId, deviceId, matrixUserId}` |
| `sendToDevices(targets, eventType, content)` | Olm-encrypts `content` as an event of `eventType` for each device of `targets` and sends it. `targets` is a list of userIds (every device of each) or `{userId, deviceIds}`. It never goes to this device. Resolves to `{sent, devices, skipped}`: `sent` is how many the server queued; `devices` and `skipped` are lists of `{userId, deviceId}` |
| `onToDevice(fn)` | `fn({type, content, senderUserId, senderMatrixId, senderDeviceId, senderCurve25519Key, verified})` for each to-device event that decrypted. Returns the unsubscribe |
| `devicesOf(userId)` | The devices of a user as this store knows them: `[{deviceId, displayName, ed25519Key, verified}]` |
| `sync()`, `close()`, `forget()` | A sync now; close the store; revoke this device and delete its store |
| `thisDeviceId()` | This browser's deviceId for the signed-in user, without loading anything |
| `state` | `{status, deviceId, matrixUserId, error}`; `status` is `idle`, `loading`, `ready` or `error` |

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
  - device-list changes, and a revoked device.
- **Browser:** `./moq test-client basic-comms --serve --spec CryptoFlow`. Two Chromium pages of two users who share a
  room:
  - each makes a device;
  - one Olm-encrypts a to-device event, and the other decrypts it with the sender's device;
  - no body the sender posted holds the plaintext;
  - after the receiver revokes its device, a send reaches nothing.

  About 5 seconds.

**Not checked live:** Element or another Matrix client against these endpoints (C6a serves them), and Firefox and
Safari (the client spec runs Chromium).
