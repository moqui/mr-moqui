# RCS Plan for the MR Moqui mobile app

Status: research and plan. Date: 2026-11-16.
Scope: the MR Moqui mobile app (`crates/moqui-mobile`), the MR Moqui server chat bridges (`crates/moqui-channels`, `crates/moqui-core/src/service/chat_bridge.rs`).

## Summary

1. A third-party Android app cannot send or receive RCS on the user's own number. Android has no public RCS API. Only Google Messages can do it.
2. The server can send and receive RCS as a verified **business** sender (RBM) through Telnyx. People can reply in a 1:1 thread. Brand vetting and carrier approval are necessary.
3. RCS end-to-end encryption (MLS, Universal Profile 3.0) is live only in Google Messages and Apple Messages. Business RCS is never end-to-end encrypted.
4. A bridge to Google Messages for Web (the mautrix-gmessages approach) gives personal RCS on the user's number. It is unofficial, AGPL, and the bridge reads all plaintext.
5. Recommendation: build a Telnyx RCS business adapter first. Add a personal RCS bridge only as an opt-in, labelled experimental feature.

## Findings

### 1. Personal RCS in a third-party default SMS app

| Item | Finding |
| --- | --- |
| Public Android RCS API | None. Google shelved planned RCS APIs in 2019. They never came back. |
| `SipDelegateManager` / IMS single registration (Android 12+) | AOSP says: "third party apps aren't allowed to access RCS single registration APIs". The app must be a **privileged** (preinstalled) app **and** the default SMS app. |
| `ImsRcsManager` | Gives registration state and capability exchange only. Some methods need carrier privileges. It does not send chat messages. |
| Carrier privileges | Not enough. The design doc says single registration is not available to downloadable apps with carrier privileges. |
| Google Jibe | US carriers (AT&T, T-Mobile, Verizon) run RCS on Google Jibe. Jibe provisioning needs DroidGuard (a Play Services internal API). Google blocks RCS on rooted phones through Play Integrity. |
| Hidden Google Messages API | XDA found one in 2021. It was restricted to Samsung apps. |
| Samsung | Samsung Messages was discontinued in the US on 2026-07-06. Google Messages is now the only Android RCS app on US carriers. |
| Android 16/17 | No new public API. Android 17 marks E2EE RCS rows as "restricted" in the telephony database. Only the default SMS app can read them. |
| EU DMA | Article 7 messaging interop does not cover Google Messages. The 2026-07-16 Article 6(7) decision makes Google expose read/write of SMS, MMS and RCS (via its Messages app) to "Qualified Services". Due in Android 18, by 2027-08-01. Scope is mainly AI services. It is not an RCS stack for third-party apps. |
| GrapheneOS | Plans its own RCS client with MLS "in the longer term". It still needs sandboxed Google Play for activation. |

**Important side effect for `sim-sms`:** Google Messages must be the default SMS app to use RCS. If the MR Moqui mobile app becomes the default, the number can stay registered for RCS. Then RCS messages from other people may not arrive. The user must turn off RCS chats in Google Messages first, or use https://messages.google.com/disable-chat.

### 2. Business RCS (RBM) through a provider

| Item | Finding |
| --- | --- |
| Model | A2P and P2A only. One business agent talks to one person. No P2P. No group chats with a business. Users cannot react with emoji to business messages. |
| Can a person reply? | Yes, in the same 1:1 thread. Replies come to the agent webhook. |
| Sender identity | A verified agent (brand name, logo, check mark). Not a phone number. It is not the user's own number. |
| Agent use cases (Google) | OTP, Transactional, Promotional, Multi-use. Fixed at launch. |
| Billing (Google) | Per carrier rate card. Basic (text up to 160 chars), Single (rich), Conversational (24 h session). |
| Onboarding | Brand verification, then per-carrier launch approval. Third-party estimates: weeks to months. SMS fallback still needs A2P 10DLC in the US. |
| Telnyx availability | Yes. US agents via API. Other countries via a portal form. |
| Telnyx costs (US) | $100 brand vetting fee. $500 T-Mobile launch fee. $0.0065 per Rich Text segment, $0.016 per Rich Media message, plus carrier pass-through fees. No platform fee. |
| Twilio | GA since 2025-08-26. $0.0083 Rich, $0.022 Rich Media, plus carrier fees. Onboarding fee figures differ by source. |
| Others | Sinch, Vonage, AWS and others resell RBM with similar rules. |

**Telnyx RCS API shape:**

| Function | Telnyx API |
| --- | --- |
| Send | `POST /v2/messages/rcs` with `agent_id`, `to`, `messaging_profile_id`, `agent_message.content_message` (`text` or `rich_card`). |
| Rich content | `standalone_card`, `carousel_card` (2-10 cards), media by `file_url`. |
| Suggestions | `reply` (text + `postback_data`), `open_url_action`, `dial_action`, `view_location_action`. |
| Fallback | `sms_fallback` (text only) or `mms_fallback` (with `media_urls`). Response `data.type` tells RCS or SMS. |
| Inbound | `message.received` webhook to the URL on the RCS agent. `body` holds `text`, `user_file` (file + thumbnail), `location` or `suggestion_response`. |
| Status | `message.sent`, `message.finalized`, `message.read` to the messaging profile URL. |
| Read receipts | Inbound: `message.read`. Not all devices send it. |
| Typing | Google RBM supports user `IS_TYPING` and agent `IS_TYPING`/`READ` events. Telnyx docs do not show them yet. |
| Signature | `telnyx-signature-ed25519` + `telnyx-timestamp`. Same as the existing SMS adapter. |
| Capability check | RCS Capabilities endpoint (is a number RCS-capable). |
| Agent management | `/v2/rcs/brands`, `/v2/rcs/agents`, test devices, launch, carrier approvals. |

### 3. RCS end-to-end encryption

| Item | Finding |
| --- | --- |
| Standard | GSMA Universal Profile 3.0 (2025-03-14) defines MLS (RFC 9420) for RCS. |
| Rollout | Apple and Google started cross-platform E2EE RCS in beta on 2026-05-11. Needs iOS 26.5 and current Google Messages, plus supported carriers. On by default. A lock icon shows. |
| Android to Android | Google Messages had its own E2EE since 2020. Google says "only Google Messages supports end-to-end encrypted RCS" on Android. |
| Third-party reach | None. MLS keys live in Google Messages or Apple Messages. No API exposes them. |
| Business RCS | Google: "Verified RCS chats between businesses and users aren't end-to-end encrypted." Provider, carrier and Google can see content. |
| Bridge (section 5) | The phone decrypts, then sends plaintext to the bridge over the web-pairing channel. The bridge sees plaintext. |

### 4. Open-source RCS clients

| Project | Status | Practical? |
| --- | --- | --- |
| `Hirohumi/rust-rcs-client` (Apache-2.0) | Rust RCS client library. Works mainly on Chinese carriers. CPM standalone, HTTP file transfer, chatbots. | No for US/EU. It needs carrier auto-configuration and IMS access. Jibe needs DroidGuard. |
| `android-rcs/rcsjta` | Old GSMA Java RCS stack. Inactive. | No. Old profile. Same access problems. |
| AOSP Messaging | No RCS. | No. |
| GrapheneOS plan | Future RCS + MLS client. Still needs sandboxed Google Play. | Watch only. |
| Rooted phone + privileged app | Could get `PERFORM_IMS_SINGLE_REGISTRATION`. | No. Carrier and Jibe servers reject non-certified clients. Play Integrity blocks root. |

### 5. Bridging Google Messages for Web

**How mautrix-gmessages works:**

- It is a Go Matrix puppeting bridge. Its protocol library is `pkg/libgm`.
- It acts like a Google Messages for Web session (messages.google.com).
- Login: Google account cookies, then the user taps a matching emoji in Google Messages on the phone. QR pairing seems disabled by Google.
- Messages go from the phone, through Google relay servers, to the bridge. Payloads use keys from pairing (`libgm/crypto`: AES-CTR, AES-GCM).
- It carries SMS, MMS and RCS (including E2EE RCS chats, after the phone decrypts them).

**Limits and risks:**

| Risk | Detail |
| --- | --- |
| License | AGPL-3.0, with exceptions only for Beeper and Element. Any port or linked use of `libgm` is AGPL. |
| ToS | Not an official Google interface. Google can change or block it at any time. Account action is possible. |
| Phone online | The phone must stay online. All messages go through Google Messages on the phone. |
| One web session | Only one Messages for Web session is active. The user's own browser session pauses the bridge. |
| Login fragility | Needs browser cookies. Device Bound Session Credentials in Chrome break it. |
| Conflict with `sim-sms` | Google Messages must stay the default SMS app. So the same phone cannot use `sim-sms` and this bridge at the same time. |
| E2EE | Breaks E2EE at the bridge. The bridge and the MR Moqui server store plaintext. |

**Could an MR Moqui `ChatBridgeAdapter` do the same?**

| Approach | Effort | License | Maintenance |
| --- | --- | --- | --- |
| A. Run mautrix-gmessages + a Matrix homeserver, then bridge Matrix to MR Moqui | High (two bridges) | AGPL stays in its own process | Upstream maintains the protocol |
| B. Small Go sidecar on `libgm` with a local JSON/WebSocket API, plus a Rust adapter | Medium | Sidecar is AGPL. Publish its source. Keep it a separate process and repo. Get a legal check. | Track `libgm` releases |
| C. Port `libgm` to Rust | High | A translation is a derivative work, so AGPL. A clean-room port is slow. | We must follow every Google protocol change |

Option B is the most practical if the user wants personal RCS.

## Options

| Option | How it works | Who can use it | E2EE? | Cost / approval | Effort | Risks | Recommendation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1. Native RCS in the app | App talks RCS to carrier/Jibe | Nobody (no API) | n/a | Not possible | n/a | n/a | Do not build |
| 2. Hand-off to Google Messages | App opens the thread in Google Messages by intent | Any user with Google Messages | Yes, in Google Messages | Free | Low | Two apps; not parity | Build as fallback |
| 3. Telnyx RCS business agent | Server adapter on Telnyx RBM API | Operators with a verified brand | No | $100 vetting, $500 T-Mobile, per-message fees | Medium | Approval time; A2P only; not the user's number | **Build first** |
| 4. Other RBM providers (Twilio, Sinch, Vonage) | Same as 3 | Same as 3 | No | Similar | Medium each | Same as 3 | Later, if needed |
| 5. Google Messages Web bridge (sidecar) | Go `libgm` sidecar + Rust adapter | One user per paired phone | No (plaintext at bridge) | Free | Medium-high | ToS, AGPL, breakage, phone online, no `sim-sms` | Optional, experimental, off by default |
| 6. Open-source IMS client | Own RCS stack | Rooted/privileged only | Possible in theory | n/a | Very high | Carriers reject it | Do not build |
| 7. EU DMA / GrapheneOS / future API | Wait for an official path | Unknown | Unknown | Unknown | Unknown | May never cover chat apps | Watch |

## Phased plan

### Phase 0: Safe `sim-sms` with no RCS loss

1. Detect Google Messages and its RCS state when the user enables `sim-sms` (`crates/moqui-mobile`, Android plugin).
2. Show a warning before the app takes the SMS role. Explain that RCS stops.
3. Give two buttons: "Turn off RCS in Google Messages" and "Open disable-chat page".
4. Add a per-thread "Open in Google Messages" action (Option 2). Use `sms:` / `smsto:` intents with the Google Messages package.
5. Label every SIM SMS/MMS thread "SMS - not encrypted".

### Phase 1: Telnyx RCS business adapter (server)

1. Add `crates/moqui-channels/src/telnyx_rcs.rs` next to `telnyx.rs`. Name the adapter `rcs`.
2. Add a chat bridge type for RCS (for example `CbtRcs`). Keep `configJson` encrypted, like the SMS adapter.
3. Config keys: `apiKey`, `publicKey`, `agentId`, `messagingProfileId`, `smsFallbackFrom`, `mmsFallback`, `apiBaseUrl`.
4. Reuse the Ed25519 webhook check from `telnyx.rs` and `webhook.rs`. Move shared code into a small `telnyx_common` module.
5. Map `deliver()` to `POST /v2/messages/rcs`. Send attachments as `rich_card` media by `file_url`.
6. Map inbound `text`, `user_file`, `location` and `suggestion_response` to `ChatBridgeServices.ingest#Event`.
7. Map `message.sent`, `message.finalized` and `message.read` to delivery and read status.
8. Record `data.type` (RCS or SMS) on each sent message. Show the real path in the UI.
9. Set `Capabilities`: `edit=false`, `delete=false`, `reaction=false`, `idempotent=false`.
10. Add a capability check before send. Fall back to the SMS bridge when the number is not RCS-capable.
11. Add tests with a fake Telnyx server, like the current SMS tests.
12. Write an operator guide in `docs/interfaces/`: brand, agent, test devices, launch, fees.

### Phase 2: Rich features and labels

1. Add suggested replies and actions to the chat model. Show them as chips in the mobile UI.
2. Add rich cards and carousels as a message kind. Render them in the mobile UI.
3. Add typing and read events when Telnyx exposes RBM `IS_TYPING` and `READ`.
4. Add an `encryption` field to `Capabilities` in `chat_bridge.rs`. Values: `None`, `Transport`, `BridgeDecrypted`, `EndToEnd`.
5. Send the field to clients. Show the labels below in thread headers and message info.

### Phase 3 (optional): Personal RCS bridge, experimental

1. Ask the user to approve the ToS and AGPL risks first.
2. Create a separate AGPL sidecar (Go, on `libgm`). Keep it in its own public repo.
3. Expose a local API: pair, send, receive, media, typing, read, reactions.
4. Add `crates/moqui-channels/src/gmessages.rs` behind a cargo feature `gmessages-bridge`. Default off.
5. Add a pairing screen in the MR Moqui mobile app: Google login, then emoji confirm.
6. Block `sim-sms` and `gmessages-bridge` on the same phone number. Show why.
7. Show health: phone offline, web session taken, login expired.
8. Label all these threads "RCS via bridge - not end-to-end encrypted".

### Phase 4: Watch items

1. Check the EU DMA Article 6(7) eligibility terms (draft due 2027-02-01).
2. Check Android 18 for new Messages APIs.
3. Check GrapheneOS RCS client progress.
4. Check Telnyx docs for RBM typing events and non-US API onboarding.

## UI labels for E2EE status

| Path | Label (thread header) | Detail text (message info) | Icon |
| --- | --- | --- | --- |
| MR Moqui chat, E2EE on | "End-to-end encrypted" | "Only people in this chat can read it." | Closed lock |
| SIM SMS / MMS (`sim-sms`) | "SMS - not encrypted" | "Your carrier can read these messages." | Open lock |
| Telnyx SMS / MMS bridge | "SMS - not encrypted" | "Telnyx, carriers and this server can read these messages." | Open lock |
| RCS business (Telnyx RBM) | "RCS Business - not end-to-end encrypted" | "Encrypted in transit only. Telnyx, Google, carriers and this server can read it." | Open lock + verified badge |
| RCS business, sent as SMS fallback | "Sent as SMS - not encrypted" | "The phone did not support RCS. This message went as SMS." | Open lock |
| RCS via Google Messages bridge | "RCS via bridge - not end-to-end encrypted" | "Google Messages on your phone decrypts it. The bridge and this server can read it." | Open lock + bridge icon |
| Hand-off to Google Messages | "Opens in Google Messages" | "Google Messages shows its own encryption status." | External link |

Rules:

- Never show a closed lock for any RCS path in this app.
- Do not copy the Google Messages lock state to bridged threads.
- Show the per-message path when it differs from the thread (RCS vs. SMS fallback).

## Open questions for the user

1. Do you accept that RCS from the server comes from a business agent name, not your own number?
2. Will you pay the Telnyx brand vetting ($100) and T-Mobile launch ($500) fees?
3. Which agent use case fits: Transactional or Multi-use?
4. Which countries do you need? Telnyx API onboarding is US only today.
5. Do you want Phase 3 (personal RCS bridge)? It has ToS risk and AGPL code.
6. If yes, do you accept a separate AGPL sidecar repo?
7. If yes, is it acceptable that the server stores bridged RCS as plaintext?
8. For `sim-sms` users, should the app warn and turn off RCS, or should it keep Google Messages as default and use only the hand-off?
9. Is a Twilio or Sinch adapter needed, or is Telnyx enough?

## Sources

Android and personal RCS:

- AOSP, IMS single registration: https://source.android.com/docs/core/connect/ims-single-registration
- AOSP design PDF: https://source.android.com/static/docs/core/connect/ims_single_registration_v1_1_1.pdf
- ImsRcsManager reference: https://developer.android.com/reference/android/telephony/ims/ImsRcsManager
- SipDelegateManager source: https://android.googlesource.com/platform/frameworks/base/+/refs/heads/master/telephony/java/android/telephony/ims/SipDelegateManager.java
- MakeUseOf, 2026-11-01, "Google Messages is the only Android app allowed to send RCS": https://www.makeuseof.com/google-messages-is-the-only-android-app-allowed-to-send-rcs-and-that-should-worry-you/
- How-To Geek, SMS app article: https://www.howtogeek.com/the-sms-app-is-dead-why-google-messages-is-now-the-only-way-to-text-on-android/
- XDA, hidden RCS API (Samsung only): https://www.xda-developers.com/google-messages-rcs-api-third-party-apps/
- Android Central, Samsung Messages clone: https://www.androidcentral.com/phones/samsung-galaxy/this-app-claims-to-brings-back-samsung-messages-complete-with-rcs
- SyncTech, restricted RCS rows: https://www.synctech.com.au/faqs/why-are-some-messages-missing-from-the-backup/
- Google Messages RCS FAQ: https://support.google.com/messages/answer/9487020?hl=en
- Google RCS deregistration: https://messages.google.com/disable-chat
- 9to5Google, deregistration form: https://9to5google.com/2020/06/19/google-messages-disable-rcs-form/
- EU DMA Article 6(7) dossier: https://research.eutechreg.com/dossiers/google-android-interoperability-dma/
- EU DMA case summary: https://ec.europa.eu/competition/digital_markets_act/cases/202619/DMA_100220_2145.pdf
- Element, DMA scope: https://element.io/blog/the-eu-digital-markets-act-is-here/

Business RCS:

- Telnyx, send RCS: https://developers.telnyx.com/docs/messaging/messages/send-an-rcs-message
- Telnyx, RCS webhooks: https://developers.telnyx.com/docs/messaging/messages/receiving-rcs-webhooks
- Telnyx, RCS getting started: https://developers.telnyx.com/docs/messaging/messages/rcs-getting-started
- Telnyx, RCS API onboarding guide: https://support.telnyx.com/en/articles/16624885-rcs-api-onboarding-guide
- Telnyx, agent management API: https://telnyx.com/release-notes/rcs-agent-management-api-release
- Telnyx pricing: https://telnyx.com/pricing/messaging
- Google RBM, agent use cases: https://developers.google.com/business-communications/rcs-business-messaging/guides/learn/agent-use-cases
- Google RBM, user events: https://developers.google.com/business-communications/rcs-business-messaging/guides/build/events
- Google RBM, agent events: https://developers.google.com/business-communications/rcs-business-messaging/guides/build/events/send-events
- Google Messages, RCS chats with businesses: https://support.google.com/messages/answer/7513219?hl=en
- Twilio RCS GA: https://www.twilio.com/en-us/blog/products/launches/rcs-generally-available
- Twilio RCS page: https://www.twilio.com/en-us/messaging/channels/rcs
- Sinch, RCS pricing: https://sinch.com/blog/rcs-pricing-explained/
- Vonage, RBM use cases: https://api.support.vonage.com/hc/en-us/articles/15331569271836-RBM-Supported-Use-Cases
- SimplyRCS, provider price comparison (vendor source): https://simplyrcs.ai/blog/how-much-does-rcs-cost/

RCS E2EE:

- GSMA, UP 3.0 E2EE: https://www.gsma.com/newsroom/article/rcs-encryption-a-leap-towards-secure-and-interoperable-messaging/
- Apple Newsroom, 2026-05-11: https://www.apple.com/newsroom/2026/05/end-to-end-encrypted-rcs-messaging-begins-rolling-out-today-in-beta/
- Google Messages, E2EE help: https://support.google.com/messages/answer/10262381
- 9to5Google, UP 3.0: https://9to5google.com/2025/03/14/rcs-end-to-end-encryption-update/

Open-source clients:

- rust-rcs-client: https://github.com/Hirohumi/rust-rcs-client
- rcsjta: https://github.com/android-rcs/rcsjta
- GrapheneOS Messaging issue #44: https://github.com/GrapheneOS/Messaging/issues/44
- heise, GrapheneOS RCS plan: https://www.heise.de/en/news/GrapheneOS-plans-its-own-messenger-solution-with-RCS-and-E2EE-11445618.html

Bridges:

- mautrix-gmessages: https://github.com/mautrix/gmessages
- License exceptions: https://github.com/mautrix/gmessages/blob/main/LICENSE.exceptions
- Authentication docs: https://docs.mau.fi/bridges/go/gmessages/authentication.html
- libgm crypto: https://github.com/mautrix/gmessages/tree/main/pkg/libgm/crypto
- Go package: https://pkg.go.dev/go.mau.fi/mautrix-gmessages
