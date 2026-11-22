# 07. Secrets, guest share, notifications, voice and usage

Part of [Agent Code](README.md). T3 sources: `t3:docs/user/{secrets,guest-links,mobile-notifications,voice-input,
usage}.md`, `t3:packages/contracts/src/{secretRequest,secretReference,guestShare,speech,usage}.ts`,
`t3:packages/secret-provider-*`, `t3:apps/server/src/guest/http.ts`, `t3:apps/server/src/speech/*`.

## 1. Secrets

### 1.1 Store

- MR Moqui has one field key for all `encrypt="true"` fields today. The sealed-secret plan
  ([plans/Sealed-Secrets.md](../Sealed-Secrets.md)) and vaults ([plans/Vault-Instances.md](../Vault-Instances.md))
  are not built.
- Agent Code does not wait for them. It adds one interface, `SecretStore` in `crates/moqui-code/src/secret/`, with
  these backends:

| Kind (`AgentSecretSource.kindEnumId`) | Where the value is | When |
|---|---|---|
| value | An encrypted field of `AgentSecretSource` (today's field key) | Phase 1 |
| sealed | `moqui.security.SecretValue` (Sealed-Secrets S1) | When S1 is built; a move job re-encrypts `value` rows |
| onepassword | 1Password through the `op` CLI on the server host (desktop sign-in, service account, Connect server), as T3 | Phase 1 |
| vault | A vault lease (Vault-Instances) | When vaults are built; best fit for a run's provider credential and for scheduled tasks |

- A field that takes a credential (provider env, host account token, webhook secret, device host identity, voice
  cloud key, MCP auth) has a source picker: **Value** or a reference such as `op://vault/item/field`. A check button
  reads it now (`check#SecretSource`, T3 `server.checkSecretSource`).
- Secrets are read when needed (for example at provider start) and kept in memory only for that use.
- A locked provider shows an error that names it. Agent Code never puts another login in place of an unreadable
  credential.
- Removing a provider lists and clears the fields that used it.

### 1.2 `request_secret`

| T3 | MR Moqui |
|---|---|
| Card: the user types the value and picks **Save securely**, **Decline**, or a provider reference; answerable from web, desktop and phone | `AgentSecretRequest` row plus an `AgentQuestion` with kind secret. Card in the thread on all surfaces. |
| `SecretRequestAnswerInput {threadId, turnItemId, answer}`; answer `save {secret}`, `save {values}` (several fields), `decline`, `reference {reference}` (servers that advertise `secretRequestReferences`) | `answer#SecretRequest` with the same shapes. The descriptor ([09](09-Wire-API.md)) advertises `secretRequestReferences`. |
| The value goes under a one-use `SecretRef`; the thread learns only the status; the value never appears in the conversation | `secretRef` = 128-bit random id, value in the store of §1.1, spent by the first tool that reads it. The answer service never writes the value to `AgentMessage`, `AgentRunEvent`, logs or the change feed. |
| `delivery: file`: a private file the agent's commands can read for about 24 h | `fileLocation` in `agent_code_home/secrets/<requestId>` mode 0600 owned by the run-as account (14 §5). `fileExpireDate` = 24 h. A job deletes expired files. |
| Notes to the agent: declined ("Don't ask for it in chat..."), cancelled ("Ask again with a new clientRequestId only if...") | Same texts as tool results. |
| A card may open an http(s) page (`secretRequestPageUrl`) | `pageUrl` (https only, or http to loopback). |
| RPC `secrets.answerRequest` | `/agentws` `secrets.answerRequest`. |

- Answering needs a recent sign-in when the request comes from a peer link or a remote session.
- Guests ([§3](#3-guest-share)) never see a secret card.

## 2. Notifications

### 2.1 Topics

Seed `NotificationTopic` rows in the component:

| Topic | When | Decidable |
|---|---|---|
| `AgentCode` | A run finishes or fails; a scheduled task fails; a PR watch wakes with a failure | No |
| `AgentCodeApproval` | A run waits for an approval | Yes: `approveServiceName` and `refuseServiceName` are the approval services, so a phone can decide from the notification |
| `AgentCodeInput` | A run asks a question or a secret | No (opens the thread) |
| `AgentCodeGuest` | A guest message waits | No |

- `linkTemplate` opens the thread: `/qapps/code/t/${conversationId}`. The app deep link maps it ([12 §5](12-Mobile.md#5-push-live-updates-and-widgets)).
- Foreground rule (as T3): a device whose app is in front with the thread open gets no alert. Viewing on another
  device does not silence the phone.
- Client setting `notificationMode` (default off) on web and desktop: off, notifications, sound, notifications-and-sound
  ([10 §9.2](10-Web-UI.md#92-client-settings)).

### 2.2 Delivery

- Web Push and FCM exist (`crates/moqui-core/src/service/web_push.rs`). Agent Code uses them.
- APNs is new: provider `PushApns`, a `crates/moqui-core/src/service/apns.rs` client (HTTP/2, token auth, key in the
  secret store). Built with iOS in MD7.
- Live Activities and Live Updates: [12 §5](12-Mobile.md#5-push-live-updates-and-widgets). They use
  `WebPushSubscription.purposeEnumId` WpsLiveActivity and WpsPushToStart.
- T3 needs T3 Connect for push. MR Moqui does not: the server sends FCM and APNs itself. A server behind NAT still
  sends, because push is outbound.
- Push is a hint. The payload carries title, link, topic and message id, never the body. The app reads the thread on
  open.
- Relay endpoint `POST /api/connect/live-activity-preference` maps to `set#LiveActivityPreference`.

## 3. Guest share

| T3 | MR Moqui |
|---|---|
| Beta, **Settings > Labs > Guest links** (`guestSharingEnabled`, default false); needs T3 Connect; link points at the T3 Connect address | Same setting. Needs a public URL: the server's `webapp_https_url` or the edge node's public URL. |
| **Share with guest** from sidebar menu, thread menu, palette, phone long-press | Same. `create#GuestShare` needs `AgentCodeUse` and ownership of the thread. |
| One browser per link; expires after 24 h | `AgentGuestShare.browserKeyHash` set on redeem (a random key in a cookie). Other browsers get 403. `expireDate` = 24 h. |
| Guest messages wait as **Request**; owner chooses **Send** or **Dismiss**; status pending, sent, dismissed | `AgentGuestMessage`. **Send** dispatches a normal run with `AgentRun.guestMessageId`. |
| Guest sees recent history and live stream (messages, files, replies, images, reasoning, plans, questions, work, command output); no diffs, fork, revert, file open or answering | A guest view at `/guest/{shareId}`. It reads through a filter that removes diffs, file changes, approvals, secret cards and tool inputs that hold paths outside the work root. |
| Attachments: 4 files and 10 MB per message; images PNG, JPEG, GIF, WebP; pending total 20 MB | Same limits. Files in `ContentBlob`, scanned like chat attachments. |
| Ends on **Stop sharing**, archive, new link, or Connect unlink | Ends on Stop sharing, archive, a new link, the owner losing access, or the public URL going away. |
| Routes under `/api/guest`: `POST /redeem`, `POST /messages`, `GET /stream`, `/session`, `/render`, `/item`, `/history`, `/command-output`, `/attachment`, `/media` | Routes under `/guest/api/` in `moqui-host`, same names. No Moqui session. Auth by the share cookie. Rate limit 30 requests per minute per share. |
| RPC `guestShare.create` | `/agentws` `guestShare.create`. |

- The guest is never a `UserAccount`. Rows written for the guest carry `userId` = the owner and
  `createdByKindEnumId` AgckGuest.
- The guest view has no Moqui shell, no menu and no links to other screens.
- Audit: each redeem and each Send writes an `AgentAuditLog` row.

## 4. Voice input

### 4.1 What exists

- `org.moqui.call.SpeechServices.transcribe#Audio` (`crates/moqui-core/src/service/speech.rs`): speech to text
  through the LLM model bound to the `transcribe` feature on an OpenAI-compatible endpoint. Internal only.
- `speech-local` (`crates/moqui-host/src/speech_local.rs`): local Whisper on candle, not built (a stub).

### 4.2 Port

| T3 | MR Moqui |
|---|---|
| Dictate into the composer and other editors (citation comments, diff annotations, PR comments, review summaries, desktop browser annotations); `mod+shift+d`; max 5 minutes; `Esc` discards; Send while dictating queues the send | Same, in a shared `m-dictate` component. |
| **Dictation shortcut** Auto, Hold, Toggle | Client setting `dictationShortcutMode`. |
| **Transcribe with** an environment (per device) | Client setting per device. Any linked environment that has voice on. |
| **Apple Speech** on macOS 26+ desktop | Desktop host command `speech.start` on macOS (later). |
| Cloud: OpenAI, xai, OpenRouter (`CloudSpeechProvider`) | `LlmEndpoint` rows bound to the new feature `code-voice` (falls back to `transcribe`). Keys through the secret picker. |
| Mobile transcribes on the device by default | Android `SpeechRecognizer` in the Kotlin host; iOS Speech with MD7. |
| Local models: download in **Settings > Voice > Transcription**; reuse copies from other apps' folders or the Hugging Face cache; **Use existing** path; default `handy-computer/parakeet-unified-en-0.6b-gguf` | `AgentVoiceModel` rows per host. Local engine: build `speech-local` (open decision O10: Whisper on candle as planned, or Parakeet GGUF as T3). |
| Model states downloadable, downloading, verifying, installed; speech states missing-model, ready, transcribing; tier cpu, accelerated | Same enums. |
| **Advanced**: GPU/CPU (Auto retries once on CPU), unload timeout (never, immediately, min_2, min_5, min_10, min_15, hour_1), **Streaming transcription** | Same settings in `AgentCodeSetting`. |
| Language (Auto), Translate to English, Dictionary with aliases | `UserPreference` `code.voice.*`. |
| **Clean up transcripts** with the text generation model; prompt mode default, custom | Feature `code-voice-cleanup` ([03 §8](03-Providers.md#8-text-generation)). |
| HTTP `/api/voice/status`, `/models`, `/models/prepare|download|select|cancel|remove|link`, `/acceleration`, `/model-unload-timeout`, `/transcribe`, `/post-process`, `/cloud/keys`, `/cloud/:provider/key`, `/cloud/openrouter/models`, `/cloud/:provider/transcribe` | Services in `VoiceServices.xml` of the component, at `/rest/s1/code/voice/...`. Upload size 25 MB. |
| Live stream WebSocket `/ws/voice` | `/agentws` binary sub-stream `voice` (audio frames in, partial text out). |

## 5. Usage

| T3 | MR Moqui |
|---|---|
| **Usage** page (sidebar, palette, `mod+u`) | `/qapps/code/Usage`. |
| Combines Codex, Claude Code, Grok Build, OpenCode, Antigravity, Pi, Muse Code and Cursor history from connected environments | `AgentHarnessUsageDaily` per host, plus the native engine's `LlmUsageDaily`. Linked environments add theirs through the peer link ([13 §6](13-Servers-Linking.md#6-peer-links)). |
| Token use, cache savings, model breakdown, estimated API-equivalent cost by token type and speed; **Premium** (fast surcharge); **Other** | Same columns. |
| Cost source providerReported, modelPriced, unpriced; resolution day, hour; source status ok, missing, partial, failed; pricing status fresh, cached, unavailable | Same enums. Hour resolution is computed from run rows, not stored. |
| Reads native histories (CODEX_HOME, CLAUDE_CONFIG_DIR, GROK_HOME, OpenCode SQLite, Antigravity DBs, Pi and Muse sessions); Cursor reads its API | Job `import#HarnessUsage` per host every 15 min, one reader per driver in `crates/moqui-code/src/usage/`. A reader that fails logs its name; the rest go on. |
| `usageHistoryRetentionDays` 92 or unlimited; views show 90 days | Same setting. A cleanup job removes older daily rows. |
| `usage-host-id` or `T3CODE_HOST_ID` | `hostInstanceId` of the node. |
| **Model prices**: custom USD rates per million tokens, **Map to**, **Reset to automatic**, **Retry failed saves** | `AgentModelPriceOverride`. Native models use `LlmModel` prices. |
| `usageModelFilter {include[], exclude[]}` | Server setting. |
| Environment filter; progressive results | Same. Each environment streams its part. |
| RPCs `server.getUsageSummary`, `server.refreshUsageRates`; `provider.consumeResetCredit` | `get#UsageSummary`, `refresh#UsageRates`, `consume#ResetCredit`. |
| **Usage limits** per provider account on the same page | From `AgentProviderAccount.limitsJson` ([03 §7](03-Providers.md#7-usage-limits-and-attachments)). |

- A user sees their own usage. `AgentCodeAdmin` sees all users and can filter by user.
- Usage joins the existing agent budget checks: a harness run counts against the user's `AgentBudget` when the cost is
  known.
