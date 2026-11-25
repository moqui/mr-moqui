# MM spike: Android native screen share

Stage 1 of phase MM of [Comms-Whiteboard](Comms-Whiteboard.md) (§3.6, §11.1): the spike of option A, MediaProjection
and MediaCodec VP8 into `str0m` in the app's Rust, connected to the SFU as the participant's screen leg. Findings with
the commands that produced them; folded into the docs when MM is built.

Machine: Linux x86_64, the `moqui-36` AVD (Android 16, API 36, x86_64, `google_apis_playstore`), NDK 30.0.16248370.

## 1. Crypto backend: str0m for Android

**Answer: the workspace's str0m 0.25.0 with `default-features = false, features = ["aws-lc-rs"]` builds for both
Android targets in moqui-mobile's workspace with no change to the build and no new env vars.**

- The app already links `aws-lc-rs` 1.18.2 / `aws-lc-sys` 0.46.0 for Android: rustls (reqwest, rustls-platform-verifier)
  pulls it in (`cargo tree -i aws-lc-sys --target aarch64-linux-android`). str0m's `str0m-aws-lc-rs` crypto crate
  uses that same `aws-lc-rs`, so the C library is not built twice and nothing new is needed of the NDK.
- `aws-lc-sys` 0.46 builds with the `cc` crate (no cmake, no bindgen for these targets), from the variables
  `scripts/android-env.sh` already exports: `CC_<target>`, `AR_<target>` (the NDK's `<target>26-clang`, `llvm-ar`) and
  `CARGO_TARGET_<TARGET>_LINKER`. `./moq android` and `scripts/check-android.sh` source it, so nothing changes there.
- Cold build of `-p str0m` (with aws-lc-sys and the rest of its tree) in a fresh target dir, `-j 4`:
  `aarch64-linux-android` 66 s, then `x86_64-linux-android` 44 s.

  ```sh
  cd crates/moqui-mobile && . ../../scripts/android-env.sh
  cargo build -j 4 -p str0m --target aarch64-linux-android
  cargo build -j 4 -p str0m --target x86_64-linux-android
  ```
- Adding it to moqui-mobile's `Cargo.toml` locks 29 new crates (dimpl, sctp-proto, rcgen, the x509 parsers ...);
  `./moq update-app-locks` then aligned three of them (`is`, `sctp-proto`, `x509-cert`) to the repository lock's
  versions, as `crates/moqui-client/tests/app_workspaces.rs` requires.
- The alternatives str0m 0.25 offers, for the record: `openssl` / `openssl-dimpl` (an OpenSSL for Android to vendor and
  keep patched: `vendored` builds it from source, more C and a second crypto library in the app) and `rust-crypto`
  (pure Rust: RustCrypto primitives with str0m's own `dimpl` DTLS 1.2; no C at all, but a second crypto stack beside
  aws-lc and not the one the SFU's DTLS/SRTP suite test checks). None is needed: aws-lc-rs is already in the app and is
  the SFU's own backend, so both ends run the same DTLS and SRTP code.

## 2. Capture and encode on the emulator

**Answer: MediaProjection → VirtualDisplay → MediaCodec VP8 input surface works on the emulator, with the platform's
software encoder `c2.android.vp8.encoder`; the first encoded frame is out ~190–200 ms after consent.**

- The emulator's codecs (`/apex/com.android.media.swcodec/etc/media_codecs.xml`, `/vendor/etc/media_codecs*.xml`) have
  no hardware VP8 encoder: the goldfish codecs are decoders only, and the one VP8 encoder is `c2.android.vp8.encoder`
  (alias `OMX.google.vp8.encoder`), libvpx in the media-swcodec Mainline module, so on every phone from Android 10.
  `MediaCodecList.findEncoderForFormat` picks it; a phone with a hardware VP8 encoder ranks that first. The code takes
  whichever the list names and falls back to `createEncoderByType`.
- The capture: 576x1280 (the 1080x2400 screen scaled to a long side of 1280, even sides), `COLOR_FormatSurface`, CBR
  1.2 Mbps to start, `KEY_I_FRAME_INTERVAL` 60 s (keyframes on request), `KEY_REPEAT_PREVIOUS_FRAME_AFTER` 250 ms.
- **The repeat matters**: a VirtualDisplay draws only when the screen changes, and an encoder with no new input makes no
  frame, so on a still screen a keyframe request waits for the next repeat. Measured ask → keyframe out of the encoder:
  54 ms on a changing screen, 215 and 259 ms on a still one (bounded by the 250 ms repeat). A 100 ms repeat bounds it
  at ~100 ms for a few hundred bytes a second of repeated frames; to choose in the build.
- **The frame rate is the display's** (60 on the emulator) whatever `KEY_FRAME_RATE` says: a scrolling screen went at
  60 fps and ~3.4 Mbps against a 2.2 Mbps target. `KEY_MAX_FPS_TO_ENCODER` (API 29) caps what the surface hands the
  encoder: with it at 15, the same scrolling gave 15 fps (32 frames in 2.13 s, four samples).
- Keyframe on request: `setParameters(PARAMETER_KEY_REQUEST_SYNC_FRAME)` works (the next output has
  `BUFFER_FLAG_KEY_FRAME`). Bitrate: `setParameters(PARAMETER_KEY_VIDEO_BITRATE)` takes effect with no restart (the
  leg's estimate moved it 1.37 → 2.08 → 2.5 Mbps, and down to 1.95).
- **Consent for automation**: `adb shell appops set org.moqui.mobile PROJECT_MEDIA allow` makes
  `createScreenCaptureIntent()` come back OK at once with no dialog (34 ms from asking to the result); `appops reset
  org.moqui.mobile` after. Without it, Android 14+'s dialog defaults to "Share one app"; uiautomator chooses "Share
  entire screen" (tap the spinner, then the item) and taps "Share screen", which also worked.
- The foreground service: `startForeground(..., FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)` before `getMediaProjection`
  (Android 14's order), with the manifest's `FOREGROUND_SERVICE_MEDIA_PROJECTION` and
  `foregroundServiceType="mediaProjection"`; the `MediaProjection.Callback` registered before `createVirtualDisplay` (14
  throws otherwise). `dumpsys activity services` showed `isForeground=true types=0x20` with the `screen-share`
  notification; after `screenStop` there was neither (`dumpsys notification`, `dumpsys activity services`).
- The shell's WebView is paused while the app is in the background: a page promise (a request, a timer) waits until
  the app is in front again, though a bridge command is still answered. The capture and the Rust leg keep going (the
  service and the leg's thread), so the media is fine in the background, but whatever the page must do then (relay an
  SFU offer, send a leave) waits. See §4.

## 3. Send: str0m to the SFU as a screen leg

**Answer: it works end to end, signalled through the page's own socket: the browser decoded the phone's screen 320 ms
after consent (target 2 s).**

The spike's path (in the commit, but for the scripts that drove it): the page calls `addScreenLeg` (a new
`add#ScreenLeg`, a `CltScreen` leg of the user's own participant), then `moquiDesktop.screenOffer()` (Rust makes a str0m
offer: one send-only VP8 video, no candidates), sends `{rtc:'join', callLegId:<screen leg>, sdp}` on its own
notification socket, and hands the `joined` frame's SDP to `screenAnswer`: Rust takes the server's UDP candidate from
it, binds a UDP socket connected to it, and adds that socket's local address as its host candidate (the ICE-lite
server answers the phone's checks at whatever address it sees). The page relays each SFU `offer` for the leg to
`screenRemoteOffer` and its answer back, asks for the stage with `{rtc:'stage', action:'ask', kind:'screen',
callLegId:<screen leg>}`, and once the stage names the leg sends `{rtc:'publish', mid, source:'screen'}`. Encoded
frames go Kotlin → `Native.screenFrame` → a channel → the leg's thread → `writer(mid).write(pt, now, MediaTime(µs × 9 /
100, 90 kHz), frame)`; str0m's VP8 packetizer makes the RTP. The server took the leg as it is: `bind#Connection` checks
the leg's participant is the socket's user, and the stage's `change` that `personOf(leg)` is the user's and the leg's
connection is active, which holds for a leg of the user's own participant; the SFU's stage gate then forwards it.

Set-up (the scripts that drove it are throwaway, not committed):

```sh
# the worktree's path is too long for embedded Postgres's socket (108 bytes), so a throwaway Postgres
docker run -d --name mm-spike-pg --tmpfs /var/lib/postgresql -e POSTGRES_USER=moqui -e POSTGRES_PASSWORD=moqui \
  -e POSTGRES_DB=moqui -p 127.0.0.1:5491:5432 postgres:18
./moq run --debug --nobuild -p 8091 --https-port off \
  -D moqui_conf__webapp_list__webapp__WEBROOT__http_bind=127.0.0.1 \
  -D moqui_conf__webapp_list__webapp__WEBROOT__https_enabled=false -D moqui_conf__ssh_server__port=2232 \
  -D moqui_conf__media_server__port=10091 -D moqui_conf__media_server__host=10.1.0.202 \
  -D moqui_conf__calls__direct_path=false \
  -D moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres ... (user, password, sslmode, port 5491)
moqui test basic-comms --spec CallClientFixtures --db existing      # calls.client.b and calls.client.c
adb reverse tcp:8091 tcp:8091; adb shell appops set org.moqui.mobile PROJECT_MEDIA allow
google-chrome --headless=new --remote-debugging-port=9302 --use-fake-ui-for-media-stream --use-fake-device-for-media-stream
adb forward tcp:9301 localabstract:webview_devtools_remote_<pid>    # the app's debug WebView, driven over CDP
```

The media server binds the host's LAN address, which both the emulator (through its NAT) and the host's Chrome reach;
`adb reverse` is TCP only, so it cannot carry UDP media. A real phone on the LAN is the same case. Chrome
(calls.client.c) called calls.client.b, the app answered with voice, through the SFU (`calls@direct-path` false; see
§4), the relay above ran in the app's shell page over CDP, and Chrome sampled `getStats` every 20 ms.

Numbers (one run each, the debug server binary; the emulator's clock is the host's minus ~205 ms by `date +%s%3N` on
both, ±20 ms, and the times are corrected to the host's):

| Step | Time |
| --- | --- |
| the page asks consent → consent (the appop) | 153 ms (the consent activity's start) |
| consent → first encoded frame | 187 ms |
| `addScreenLeg` (the transition) | 283 ms |
| `join` sent → `joined` (the page's socket and the media node) | 22 ms |
| answer taken → str0m `Connected` (ICE and DTLS) | 2 ms |
| connected → first frame sent | 23 ms (the first keyframe after connecting) |
| the stage granted → first frame decoded in Chrome | ~40 ms |
| **consent → first frame decoded in Chrome** | **320 ms** |
| a new subscriber (Chrome's `joinCall` after a reload) → its first screen frame decoded | 500 ms; 417 ms in a second run |
| the SFU's keyframe request → keyframe out of the encoder | 54 ms (a changing screen), 215 / 259 ms (a still one, §2) |

- **Keyframes**: Chrome sent no PLI or FIR (`pliCount` 0): the SFU asks the publisher for a keyframe itself when a
  subscriber starts (`Event::KeyframeRequest` in the leg), and the leg asks the encoder. The leg also asks for one at
  `Connected` and sends nothing before a keyframe, so a subscriber never starts on a delta.
- **BWE**: `enable_bwe(1.2 Mbps)` and, at connect, `bwe().set_bitrate(150 kbps, 2.5 Mbps)` gave TWCC estimates every
  1–5 s from the SFU's feedback (1.6, 2.2, 2.5, 3.1, 4.5, 6.2 Mbps as the screen moved). The leg sets the encoder to 85 %
  of the estimate, within 150 kbps – 2.5 Mbps, at most once a second and on a 15 % change: up 1.37 → 2.08 → 2.5 Mbps,
  and down 2.5 → 1.95 Mbps when the estimate fell to 2.30 Mbps. A real constraint was not measured:
  `adb emu network speed 500:20000` did not shape this path (the estimate and the bytes sent did not change), so the
  way down under loss is str0m's estimator as the SFU already uses it for its subscribers; a shaped link test is for
  the build (§5).
- Over ~3 minutes and ~4800 frames, Chrome counted 0 packets lost and 0 NACKs (loopback through the emulator's NAT).

What did not hold, each a change for the build (§4):

- **Leaving the call did not end the share.** The page left (`leave#Call`) but, in this call of two, the call went on:
  the screen leg is an answered leg of the same participant, so the participant was still in it. Chrome stayed
  connected and on the stage, and the leg and the capture went on for 200 s, until `screenStop`.
- **Stopping the share did not tell the server.** After `screenStop` the frames stopped, but Chrome's stage still named
  the screen leg; the SFU would find the peer gone only at its ICE timeout.
- **The page's frames are not per leg.** `onFrame` keeps frames by call, not by leg, so the screen leg's `joined`,
  `offer`, `track`, `layer` and `quality` frames reach the page's own peer connection; the spike wrapped `handleFrame`
  to drop another leg's. (Measuring a new subscriber with the page's `call.rejoin()` while its connection was fine sent
  Chrome into a rejoin loop, generation 10 to 83 in seconds with `unknownLeg`; that is `rejoin` misused, not the screen
  leg, and a fresh `joinCall` after a reload was clean.)
- **The SFU treats the screen leg as any peer**: it offers it every other leg's tracks (the spike answers and drops
  them) and sends it `participant`, `track` and `layer` frames, and before the `publish` frame its video is a `camera`
  track. It should be publish-only from its join, and its video the screen at once.

## 4. What the build needs, and changes to the design

Signalling through the page holds: it took no new credential path, and the server's checks (the socket's user owns the
leg, the stage's owner check, generation fencing) applied unchanged. The changes:

- **Server** (Calls.js, CallStage.js, rtc.rs, the SFU):
  - `add#ScreenLeg` as committed (a `CltScreen` leg, one at a time, refused in an E2EE call and without an answered
    browser leg), plus: a screen leg never keeps its participant in the call (a call of two ends with the other browser
    leg; the participant's `leave#Call`, hold, move or socket gone closes its screen leg); closing it takes it off the
    stage (`CallStage.leave`); the media node is told at bind that the leg is a screen leg, so the SFU makes it
    publish-only (no subscriptions, offers or participant frames to it) and labels its video `screen` from the join;
    `follow` stays refused for it (it is not `CltWeb`).
  - The direct path: a `CltScreen` leg makes `directPath()` false (it is not `CltWeb`), so a direct call of two moves to
    the media server when the share starts (the existing move, with its short gap). Either accept that or refuse the
    share on the direct path; the spike ran with `calls@direct-path` false. Decision needed.
  - The call's `view` gives the leg type `screen` (committed), for the pages to merge it into its participant's tile.
- **Page** (WebrootCall.js): Share screen in the app (where `moquiDesktop.screenStart` exists and there is no
  `getDisplayMedia`) runs the relay of §3; frames are routed by `callLegId` (the screen leg's to the bridge, never the
  page's pc); Stop and every way the page leaves the call send `leave` for the screen leg and call `screenStop`; the
  native end (the notification's Stop, the system's stop, the leg's ICE failure) tells the page (`tell_page`, a new
  `screenEnded`) so it sends the leave. Because the WebView is paused in the background, the server must also close the
  screen leg itself when the page's leg goes (above), and the native leg end on its ICE timeout. In the tiles a
  `screen` leg's track is its participant's share.
- **Kotlin**: as committed (consent each time, the mediaProjection service, the encoder with the fps cap and the
  repeat, keyframe and bitrate from Rust, Stop in the notification), plus: a rotation or a resize
  (`onCapturedContentResize`: the encoder again at the new size, then a keyframe); one projection per session (Android
  14 refuses a second `createVirtualDisplay`); the app's own `FLAG_SECURE` screens come out black, as is right for an
  encrypted room; the JVM tests of the size and bitrate rules.
- **Rust**: as committed (`screen.rs`), plus: IPv6 and more than one server candidate; ICE-TCP when UDP is blocked (the
  SFU's TCP candidate); the leg's end told to the page; unit tests (`server_address`, `is_vp8_keyframe`, an offer and
  answer against a str0m ICE-lite peer on the host).
- **E2EE**: as planned: `sframe` in moqui-mobile, the sender key from the page's `moqui.callKeys` over the bridge (a new
  shell-only command), the leg named with the page's crypto device so `bindLeg` admits it. To check first: SFrame over
  the whole VP8 frame (as WebrootSFrame.js does) leaves str0m's VP8 packetizer reading an encrypted payload descriptor,
  so the encrypted leg needs str0m's RTP mode or a generic packetizer.
- **Tests**: moqui-rtc: a screen leg is publish-only and its participant's share; leaving ends it. Specs:
  `Calls.spec.js` for `add#ScreenLeg` (the refusals, one at a time, closed with the page's leg, the stage cleared).
  Client: `WebrootCall`'s routing by leg against a fake socket. The emulator walk (consent by the appop, Chrome by CDP,
  as here) is an acceptance test, run when asked.

## 5. Open points

- The direct path (above).
- Whether a moqui-mobile host test may depend on `moqui-rtc` (a path dependency in the app's own workspace) to run the
  leg against a real `Sfu` over the testnet's shaped links, which would measure the bitrate's way down; or a str0m
  ICE-lite peer in the test alone.
- The repeat interval (keyframe latency on a still screen against idle traffic): 250 ms as now, or 100 ms.
