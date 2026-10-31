# Call recording

A call's host can record it. Recording is off until the deployment turns it on, every leg of the call gets a notice,
and only the host and the one who recorded it can play or download the files. The design is
[plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.4 "Recording", §6.4 and D-V5. Calls themselves are in
[WebRTC](WebRTC.md); the user guide is [Comms](../tools/Comms.md#calls).

- **Writers:** `crates/moqui-rtc/src/record/` (sans-IO): Ogg Opus, WebM VP8, and the recorder that cuts tracks into
  segments.
- **Recorder:** `crates/moqui-host/src/rtc/recorder.rs`, one thread per recording on the call's media node.
- **Services:** `org.moqui.impl.CallRecordingServices` and `CallRecordings.js` (framework), with the model in the
  Recording block of `framework/entity/CallEntities.xml`.
- **Browser:** the Record button, indicator and notice in `m-call` (`webroot` `js/WebrootCall.js`); the Recordings
  screen in BasicComms Calls (`Calls/Recordings.xml`, `Recordings.qvue`, and `RecordingsTui.xml` for the terminal).

## Turning it on

| Property | Default | |
| --- | --- | --- |
| `call_recording_enabled` | `false` | Lets a call's host record it. Off: every start is refused (`recordingOff`) |
| `call_recording_ack_required` | `false` | Each leg must acknowledge the notice; the recording pauses until every current leg has |
| `call_recording_policy_revision` | `1` | Logged with each notice; change it when the policy's wording changes |
| `call_recording_retention_days` | `90` | Recordings are deleted this long after they start; `0` keeps them |
| `call_recording_queue_frames` | `4096` | Frames queued from the media thread to the recorder; a recorder further behind fails |
| `call_recording_segment_mb`, `call_recording_segment_minutes` | `64`, `30` | When a file is cut and the next segment starts |
| `call_recording_max_mb` | `2048` | A recording's total; it stops there |

The policy must meet the deployment's jurisdiction. The notice alone does not.

## What is recorded

- **Each track**, as it was sent, with no transcoding: a leg's microphone and screen audio as Opus in Ogg, and its
  camera and screen share as VP8 in WebM. A simulcast track is recorded at its best layer that sent a keyframe: it
  moves up at a better layer's keyframe, and down when its layer is silent for a second. One file per track and
  segment. A WebM file starts at a keyframe, has a cluster at each keyframe, and Cues for seeking. Browsers send a
  keyframe only now and then unless asked, so while a video track waits for one (a start, a resume, a layer switch,
  a cut) the recorder asks the room's publishers for one, at most once a second.
- **The mixed audio** of the whole call: the mix-minus mixer's `all()` (the M0 `AudioRoom`, on a 20 ms tick), coded
  with `OpusEncoder`, as Opus in Ogg. A recording of kind `mixed` has only this; `perTrack` (the default) has both.
- **Timing** comes from each frame's RTP time. A gap (lost packets, a muted or stopped track) is a jump in the Ogg
  granule position or the WebM timestamp; no audio or video is made up to fill it.
- **Segments.** Pausing closes every open file, and resuming starts the next *period*. A file is also cut at the size or
  time cap. A segment row has its period, its order in the track (`segmentSeq`), and `startOffsetMs` from the start of
  its period, so a player puts the tracks of one period together. A period can be empty.
- **Not recorded:** a composited video of the call. Playback plays the tracks together in the browser. An end-to-end
  encrypted call cannot be recorded (`encrypted`).

## How it runs

1. `start#Recording` (the host's Record button) makes the `CallRecording`, logs the notice for every current leg, and
   tells the call's media node (`CallSession.mediaNodeId`) with `send#MediaControl`
   (`{action: 'recording', op: 'start', callRecordingId, kind, period, paused, notice}`) after the commit.
2. The media node starts a recorder thread. It takes every frame of the room through a bounded queue
   (`call_recording_queue_frames`) and decodes the audio for the mix on its own thread, never on the media thread. It
   writes each file to a spool under `runtime/tmp/call-recording/`.
3. Each closed segment goes back through `store#RecordingSegment`, which streams the file with `store#RecordingFile` to
   `dbresource://moqui/call/recording/<callRecordingId>/<segment id>.ogg` (or `.webm`) and writes its track and segment
   rows. A leg that leaves (`TapEvent::Left`) closes its tracks.
4. `pause#`, `resume#` and `stop#Recording` send the same control (`op: pause | resume | stop`). The call's end stops
   the recording, and the last segments are stored.

**Failure ends the recording, not the call.** A full queue, a file error or a store error stops the recorder: its
partial files are removed, and `note#RecorderEnd` marks the recording failed with `lastErrorText`. Reaching
`call_recording_max_mb` stops it with that as its text. Either way every leg is told the recording stopped, and the
call goes on.

## The notice and consent

- Every leg gets the notice: the legs in the call when it starts, and each leg that joins later (`bind#Connection` tells
  the recording, `note#CallChange`). Each notice is a `CallRecordingConsent` row: the leg, the scope, the policy
  revision, the time and the method (`CnmWeb`, or `CnmVoice` for a phone).
- Browser and guest legs get an `rtc` frame on their socket, sent by the media node to every leg of the room and again
  to a leg right after it joins:

  ```json
  {"rtc": "recording", "callSessionId": "...", "callLegId": "...", "callRecordingId": "...",
   "state": "recording", "pauseReason": null, "kind": "perTrack", "ackRequired": false, "policyRevision": "1"}
  ```

  `state` is `recording`, `paused`, `stopped` or `failed`; `pauseReason` is `user` (the host paused it), `ack` (a leg has
  not acknowledged) or `withdrawn` (a leg withdrew). The page shows the indicator to everyone, the lobby too
  (`get#RecordingState`).
- Legs that are not browsers (SIP devices and phone numbers) get the notice through `notify#LegRecording`: a voice
  prompt, and a key to acknowledge. It does nothing until SIP legs exist (V1 fills it in).
- Staying on the call after the notice is consent. With `call_recording_ack_required`, the recording pauses while any
  current leg has not acknowledged (`ack#RecordingNotice`, the Acknowledge button); the host's own leg counts as
  acknowledged when they start it. Any leg can withdraw (`withdraw#RecordingConsent`), which pauses the recording while
  it is in the call; acknowledging again takes it back. A leg that leaves no longer holds the recording. Resuming
  always starts a new period.

## Who may do what

- **Record** (start, pause, resume, stop): a host of the call, that is its starter, or the owner of the call link it
  was opened by.
- **Play, download and list:** the one who recorded it and the call's hosts. `find#Recordings` lists only those, and
  `get#RecordingFile` serves one segment by its id; anyone else gets "not found" for the same id. A guest is told of
  the recording once let in, not in the lobby, which shows a waiting guest nothing of the call. No route serves the
  `dbresource://` locations to users, and an app's record links (BOSS) go through these services and never widen this.
- **Delete:** the one who recorded it, once it is not going (`delete#Recording`): its files and rows, its transcripts (`CallTranscript`) too.
- **Acknowledge or withdraw:** each leg for itself; a guest by the credential of its page.

## Retention and recovery

The job `expire_Recordings_hourly` (`expire#Recordings`) deletes recordings past `retentionUntilDate`, files and rows,
with their transcripts. It also recovers: a recording still marked going whose call ended or whose media node stopped or restarted since it
began fails ("the recorder stopped"), a segment that was not stored is removed with any partial file, and files under the recordings' root that no
recording row names (an upload whose rows rolled back) are deleted. Each item runs on its own, and one that fails is
logged.

## Tests

- `cargo test -p moqui-rtc record::`: valid Ogg pages and CRCs, granule positions with gaps, a WebM that parses, and
  segments at pauses and caps.
- `cargo test -p moqui-host --lib rtc::recorder`: the recorder with frames in memory, its failure and clean-up.
- `cargo test -p moqui-host --test it rtc::`: a short recorded call on a real server, with its tracks, segments, files
  and the mixed audio.
- `./moq test framework --spec CallRecording`: the policy, the notice per leg (a late joiner too), the acknowledgement
  and withdrawal pauses, access, retention, a recorder failure, and recovery after a media node restart.
- `./moq test basic-comms --spec BasicCommsCalls`: the Recordings screen and its file transition.
