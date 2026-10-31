# Voicemail

A call to a user's number that nobody answers goes to their voicemail box: the SIP engine answers it with no one else
in it, plays the greeting and a beep, and records the caller. Its owner plays it in the browser, reads its transcript,
and can listen by phone with a PIN. The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.3
(Zimbra's voicemail actions and preferences), §5.4 and §5.5, milestone V2. Phone calls themselves are in
[SIP](SIP.md); the user guide is [Comms](../tools/Comms.md#voicemail).

- **Engine:** `crates/moqui-host/src/sip/voicemail.rs` (the call's state and its prompts, `Machine`, with no I/O) and
  `audio.rs` (Ogg Opus read and written with moqui-rtc's `record/ogg.rs` writer and the M0 Opus codec;
  `convert#GreetingAudio`); the leg's thread plays and records (`leg.rs`).
- **Services:** `org.moqui.impl.VoicemailServices` and `Voicemail.js` (framework), with `VoicemailBox`, `Voicemail` and
  `VoicemailPinFailure` in the V2 block of `framework/entity/VoiceEntities.xml` and the transcript a `CallTranscript`
  (`voicemailId`). `store#VoicemailFile` is Rust (moqui-core `service/call_media.rs`).
- **Screens:** BasicComms Calls, History's Voicemail tab; `Calls/Voicemail` (`Voicemail.qvue`, `VoicemailTui.xml`);
  `Settings/Phone` (`Phone.qvue`, `PhoneTui.xml`); the Voicemail card on Home.

## When a call goes to voicemail

An inbound call to a user's number (`PhoneNumber` routed `PnrUser`, from `receive#PhoneCall`) goes to their box:

- **No answer.** The engine starts a timer when the call rings: the box's `rings` (6 seconds each, 1 to 10), else
  `call_ring_seconds`. Unanswered by then, it goes. The call's ring expiry is set 30 seconds later, so `expire#Calls`
  does not end it first.
- **Busy.** The user is in another call (`UserStatus.inCall`): the call is not rung at all and goes at once.
- **Declined.** The user declines it and nobody else is rung for it (`decline#Call` sends the engine the control
  `voicemail`).

Each time the engine asks `begin#Voicemail` first, which moves the call there in one transaction (`Calls.js`
`phoneToVoicemail`): the user's ringing leg times out, their `CallUserLog` is missed (a decline stays declined), the
ring bars close, and the call is active with the phone alone. Someone who answered first wins: `begin#Voicemail`
refuses (`answered`) and the call goes on as answered. A user's box is made, with the defaults, the first time it is
needed.

## Leaving a message

1. The engine answers the call (200 with the SDP it offered when it rang).
2. The greeting: the user's own, else the default (a tone pattern, then `greeting.wav` when the deployment has one).
3. A beep (1000 Hz, half a second).
4. The caller's audio, decoded by the leg (G.711 or Opus to 48 kHz), each 20 ms written as Opus in Ogg to a spool
   file under `runtime/tmp/voicemail` (a missing packet is silence, so the length is the call's).
5. It ends when the caller hangs up, presses `#`, or `voicemail_max_seconds` passes; then goodbye and the hang-up.
6. `store#Voicemail` moves the file (`store#VoicemailFile`, which takes only that spool and writes only under
   `dbresource://moqui/call/voicemail/`) and makes the `Voicemail` in the inbox. A message shorter than a second is
   dropped. Audio that does not store keeps the message without it (`VmsAudioFailed`, with why), so its owner still
   knows of the call.

Then the owner is told, each part on its own:

- a `chatUser` push (`{kind: 'voicemail', action: 'new', voicemailId, from, unheardCount}`) and Web Push "New voicemail
  from …", linking to the message (`comms_app_root` `/Calls/Voicemail?voicemailId=`);
- an email to the box's `notifyEmail`, when there is one, through the framework's mail (the `SYSTEM` email server):
  who, when, how long and the link (absolute when `webapp_http_host` is set). The audio is attached only with
  `attachAudio`, and the transcript is in it only with `attachTranscript`. Both are off by default, are only allowed
  with an address, and turning one on answers a warning: mail cannot be recalled. With the transcript in it, the email
  waits for the transcription. It is sent once.

## Transcription

Off unless `voicemail_transcribe` is `true`: the deployment's recording and transcription policy decides whether
voicemail audio goes to the speech provider. When on, after the message commits, `transcribe#Voicemail` sends the
Ogg file to `SpeechServices.transcribe#Audio` (the model bound to `transcribe`; the owner's quota; purpose
`voicemail:<id>`; `voicemail_transcribe_language`) and writes its `CallTranscript`: `CtsPending`, then `CtsDone` with
the text or `CtsFailed` with why. A failure never touches the audio. The owner sees the state and retries a failed one
(`retry#VoicemailTranscript`, Retry on the screen).

## Phone access

With a PIN set (Settings, Phone), the owner listens by phone:

- **Their own number.** Press `*` during the greeting.
- **The access number.** A `PhoneNumber` routed `PnrVoicemail` (System, Voice, Numbers): enter the box's number (the
  user's number, as many of its last digits as make it one number) and `#`.

Then the PIN and `#`. The new messages play oldest first, each after a short tone; then the menu tone. Keys, during a
message or after it:

| Key | |
| --- | --- |
| `7` | Delete: to the trash |
| `9` | Save: marked heard |
| `#` | Next, leaving it new |

After the last one, a tone and goodbye. With no key, the menu plays three times and the call ends. `*` while entering
a number or a PIN starts it again.

**The PIN** is 4 to 12 digits (not one digit repeated or a run like 1234), kept as an argon2 hash (`pinHash`). Over the
phone, `check#VoicemailPin` answers accepted or not, never an error, so its counts commit:

- **Per box:** 5 failures in 15 minutes block the box's PINs for 15 minutes, from any caller.
- **Per source:** the caller's number, which a caller can fake, so the box's limit is the real one: 10 failures in an
  hour block it for an hour, for every box.
- A box number with no box, a box with no PIN, and a blocked box are refused the same way, after the same work (a hash
  check against a stand-in), and counted the same way: nothing tells a caller which boxes exist.
- Three refusals end the call. Setting a new PIN clears the box's count.

## Prompts

Moqui ships no recorded speech. Each prompt is a tone pattern, then the deployment's recording when
`voicemail_prompt_location` names a folder (any resource location, such as `dbresource://voicemail/prompts`,
uploaded through the Resources screen, or `component://my-component/prompts`). The files are 16-bit PCM WAV, mono or
stereo, 8 to 48 kHz, at most a minute. A file that is missing plays the tones only.

| File | Played | Tones |
| --- | --- | --- |
| `greeting.wav` | The default greeting (a user's own replaces it, tones and all) | 660, 880 Hz |
| (none) | The beep before the recording | 1000 Hz, 500 ms |
| `enter-box.wav` | The access number: "enter your number, then #" | 880 Hz three times |
| `enter-pin.wav` | "Enter your PIN, then #" | 880 Hz twice |
| `pin-refused.wav` | A refused PIN | 480, 380 Hz |
| `menu.wav` | After each message: "7 delete, 9 save, # next" | 700, 900 Hz |
| `no-messages.wav` | No new messages, or no more | 600 Hz |
| `goodbye.wav` | Before the engine hangs up | 880, 660, 440 Hz |

## Greetings

Recorded in the browser (Settings, Phone, Record: MediaRecorder, then the page decodes what it recorded and sends a
48 kHz mono WAV, which works the same in every browser) or uploaded (WAV or Ogg Opus, at most 10 MB; a minute is
kept). `store#VoicemailGreeting` checks the file's kind by its first bytes, then `convert#GreetingAudio` (the `sip`
feature) decodes it and keeps it as Ogg Opus under `dbresource://moqui/call/voicemail/greeting/<box>/`, which the
browser plays and the engine decodes for callers. A build without phone calls keeps the upload as it is. Use the
default greeting removes it.

## Access

- Only the box's owner lists, plays, downloads, marks heard or unheard, deletes, puts back, empties the trash, and
  retries a transcript (a ring group's box will be its members', V3). Anything else answers "not found", the same as an
  id that does not exist.
- The audio is read only through `get#VoicemailAudio`, from the Calls Voicemail screen's `audio` transition; no route
  serves the `dbresource://moqui/call/voicemail/` locations. A record link from an app goes through the same service.
- The engine's services (`begin#`, `store#Voicemail`, `store#VoicemailFile`, `check#VoicemailPin`,
  `act#PhoneVoicemail`, `transcribe#Voicemail`, `send#VoicemailNotice`, `convert#GreetingAudio`) are internal
  (`allow-remote="false"`).

## Retention

Delete moves a message to the trash; deleting it there, Empty trash, or `expire_VoicemailTrash_daily` (messages in a
trash longer than `voicemail_trash_days`, 30 by default; 0 keeps them) delete it for good: its audio file, its
transcripts and its row.

## Properties

| Property | Default | |
| --- | --- | --- |
| `voicemail_max_seconds` | `120` | The longest message, 10 to 600 |
| `voicemail_prompt_location` | empty | The folder of the prompts' recorded speech ([Prompts](#prompts)) |
| `voicemail_trash_days` | `30` | Days in the trash before a message goes; 0 keeps them |
| `voicemail_transcribe` | `false` | Send each message to the speech provider for a transcript |
| `voicemail_transcribe_language` | empty | An ISO 639-1 code; empty lets the model tell |
| `call_ring_seconds` | `45` | How long a call rings before voicemail when the box has no `rings` |

## Not yet

- **Speech from text.** The default prompts are tones and the deployment's recordings; no text-to-speech endpoint is
  called (moqui-llm has none yet).
- **The `external` room.** A voicemail is not yet an event in the number pair's `external` room: those rooms come with
  C1.
- **Ring groups.** A box has `ringGroupId` as a plain id until V3; nothing routes a call to one.
- **Forward and reply by email**, Zimbra's other voicemail actions, are not here; download and Call back are.

## Tests

- `cargo test -p moqui-host --lib sip::voicemail`: an unanswered call's greeting, beep and recording to Ogg Opus until
  `#`; a recording's limit; the PIN wrong then right and the menu keys; a block or three refusals ending the call; the
  access number; the prompts' audio; a WAV greeting converted to Ogg Opus.
- `cargo test -p moqui-host --test it -- sip::a_busy`: a fake trunk calls a busy user on a real server and leaves a
  message, stored as Ogg Opus in their inbox.
- `./moq test framework --spec Voicemail`: busy and unanswered routing, begin and decline, private audio refused for
  another user with the same id, the PIN with its refusal and its limits per box and per source, the notices
  (attachments off by default), a failed transcription and its retry, the trash's retention, greeting uploads.
- `./moq test basic-comms --spec BasicCommsCalls`: the Voicemail tab and its audio transition for its owner only;
  `BasicComms`: every screen in qvt and tui.
