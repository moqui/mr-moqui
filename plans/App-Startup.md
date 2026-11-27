# App startup at native speed

The plan to make an app open as fast as a native phone app: on Android, from the installed PWA and from the
[mobile app](../crates/moqui-mobile/README.md). Plan only; nothing here is built.

## Targets

The goal is an instant start: 30 ms, two frames at 60 Hz. Each target is from the tap on the launcher icon, measured
on a mid-range Android phone on Wi-Fi.

| Measure | Cold start (no process) | Warm start (process alive) |
|---|---|---|
| First frame in the app's own colors | 30 ms | 30 ms |
| Last-known content shown and usable | 150 ms with S7, 300 ms without | 30 ms |
| Fresh content shown | 1 s | 1 s |

The floors:

- **First frame, 30 ms in both cases.** Android draws the starting window from the app's theme before the app's
  process runs. So the first frame needs only the right colors and icon (S1, S6).
- **Warm content, 30 ms.** The page is still in memory. A resume shows it with no reload and no request first. Fresh
  data comes after it.
- **Cold content.** A cold start must start a process and a WebView. On a mid-range phone that is about 200 to 400 ms
  before a page can paint. S0 measures the real floor. Under it, only content drawn without the WebView is faster
  (S7).

Also:

- No frame in a wrong theme. No blank white or black frame.
- No sign-in page on a cold start while the user's sign-in is still valid.
- The targets hold with the server far away: the network only changes when fresh content comes.

## What happens now

A screen recording (2026-11-27, Android, an installed PWA of a standalone app page, cold start) took about 5 s:

| Time | Screen |
|---|---|
| 0.0 s | Tap on the launcher icon |
| 1.0 s | OS splash with the icon |
| 2.0 s | Black screen |
| 3.0 s | The `/qapps` shell header in its light theme, empty white body |
| 4.0 s | The app's own page in its dark theme, "loading" text |
| 5.0 s | Content |

The causes, from the code:

1. **Two page loads.** The manifest's `start_url` is `/qapps` (`webroot/manifest.webmanifest`). An app tile that opens
   a standalone page answers `{redirectUrl}`, and the shell does a full page load (`WebrootVue.qvt.js`). So the full
   shell (Vue, Quasar, `clientConfig`) loads only to leave.
2. **One manifest for all apps.** One name, one icon, `background_color` `#ffffff` and `theme_color` `#1976d2`. A dark
   app shows a light splash and a light shell frame first.
3. **The page is never cached.** `sw.js` caches only versioned static files (SEC-CLI-02). Each start waits for the
   network: TLS, the session check and a server render of the page.
4. **Data comes last.** The page shows, its scripts run, then it asks for its data. Nothing shows the last-known data.
5. **The WebView paints black** before the first page paints (the mobile app and the PWA's WebAPK both).

## Quick test

A test of S2 and S3 alone, on 2026-11-27: a local server with fixed delays (400 ms for the page, 300 ms for the
data), headless Chromium 1248, CPU slowed 4 times, 10 runs. A is the page and data from the network, as now. B is the
shell document from the service worker and the last data from IndexedDB first. Cold is a new browser process; reload
is the same process. Numbers are from navigation start, median / p90, in ms.

| Variant | Content, cold | Content, reload | Fresh data, cold |
|---|---|---|---|
| A, now | 761 / 768 | 758 / 759 | 761 / 769 |
| B, S2 and S3 | 133 / 144 | 80 / 91 | 444 / 459 |

What it shows:

- S2 and S3 show content about 6 times sooner, and the time no longer depends on the network.
- The web path alone does not reach 30 ms. The service worker start and the IndexedDB read cost about 50 to 120 ms.
  So 30 ms content needs a resume with no reload (S6), and a cold start needs S7.
- A desktop browser is not a phone. The Android emulator check did not run: the device panel needs an ffmpeg with
  libx264 on this machine. S0 measures on a phone.

## Work

Do the steps in order. Each step has its check. S0 comes first, so each later step has a before and after number.

### S0. Measure

- Give `WebrootRum.js` a `startup` span: navigation start, first paint, first frame in the app's theme, last-known
  content shown, fresh content shown. Mark the run cold or warm, PWA or mobile app, cached or not.
- Let a standalone page (one outside the shell) send the same span with a small script, `js/WebrootStartup.js`, with no
  Vue.
- For the mobile app, record `adb shell am start -W` (`TotalTime`) and the span.
- Add `scripts/startup-measure.sh`: on a connected phone, N cold and N warm starts of one app, then the median and p90.
  It is a device check, run when asked, not a test.
- Show the numbers per app in System, Monitor.

**Check:** the numbers of the recording above come back within 10%.

### S1. One installed app per app

- Serve a manifest per app: `/apps/<app>/manifest.webmanifest`, from attributes on the app's root screen (name, short
  name, icon, `theme_color`, `background_color`) with the site manifest's values as defaults. `id` and `start_url` are
  the app's own page, `scope` its path.
- The site manifest stays for `/qapps`.
- A tile for a standalone page is a plain link to the page, with no JSON redirect.
- Each page sends `<meta name="theme-color">` and the background color in an inline style in the first bytes of the
  `<head>`.

**Check:** an installed app opens its own page with no `/qapps` request, and the splash and first frame have its colors.

### S2. The app's page from the service worker

- Split a page in two: a **shell document** with no user data and no session token, and the data the page asks for.
- The shell document is versioned and answered `Cache-Control: public`. `sw.js` keeps it, answers it cache-first, and
  gets the new version in the background.
- The page gets its session token from a small transition after it shows (`/qapps/sessionToken`, no-store).
- Turn on navigation preload, so a page not cached yet does not wait for the worker to start.
- This keeps SEC-CLI-02: the cache still has no answer that depends on who is signed in.

**Check:** with the server stopped, an installed app shows its shell document. A spec checks that the cached shell
document is the same for two users.

### S3. Last-known content

- A page keeps its last data on the device (IndexedDB), under the user and the server. It shows that data at once,
  then asks for fresh data and changes only what changed.
- Logout, an account switch and a 401 delete the user's store. The store has an age limit per app.
- The mobile app already keeps each account's data in a store for offline use. Use the same contract, so one page
  code runs in both.
- This is private data on the device, which SEC-CLI-02 does not allow now. Decision D1 allows it.

**Check:** a cold start in airplane mode shows the last content. A spec checks that logout empties the store.

### S4. No sign-in on a cold start

- Find whether a session survives a server restart and a long idle phone.
- If not: a device key per installed app (as the mobile app's device login keys), so the page signs in again with no
  page change and no user step.

**Check:** a cold start after a server restart shows content with no Login page.

### S5. A lighter first paint

- Inline the CSS the first frame needs. Load the rest after it.
- Send `Link: rel=preload` (and HTTP 103 Early Hints where the proxy passes them) for the page's scripts and fonts.
- Fonts with `font-display: swap`. Brotli for text files. `immutable` on versioned files.
- In the `/qapps` shell: load the code of the open screen first, and Quasar parts on use.

**Check:** first paint of a page not cached is under 300 ms after its first byte, per S0.

### S6. The mobile app

- Keep the Android 12 splash on screen (`setKeepOnScreenCondition`) until the page sends "content shown". This
  removes the black frame.
- Set the WebView's background color to the app's theme before the first load.
- Make the WebView early, in `Application.onCreate`, in parallel with the Rust start.
- Show the last-known store before the first server request (S3).

- On a resume, show the page as it was. Ask for fresh data after the first frame, never before it.

**Check:** `am start -W` and the S0 span meet the targets on the `moqui-36` AVD and on one phone.

### S7. Cold content without the WebView (mobile app)

- When the app goes to the background, keep a snapshot of its screen: the image, or the last-known content as a small
  list the Kotlin side can draw.
- On a cold start, draw that snapshot natively in the first frames, while the process and the WebView start behind it.
- Change to the WebView page with no visible change when it has painted the same content.
- The snapshot is private data on the device: D1 applies. Screen security (`FLAG_SECURE`) turns snapshots off.

**Check:** on a cold start, the content shows in 150 ms, and the change to the WebView shows no flash.

## Tests

- Fast specs only, each well under 15 s: the per-app manifest fields; a standalone tile is a plain link; the cached
  shell document has no user data; logout empties the last-known store; the service worker answers a shell document
  with the server stopped.
- Timing on a phone is a device check in [Testing-Needed](Testing-Needed.md), run when asked. It is not a test, and no
  test asserts a time.

## Decisions

Decided 2026-11-27.

| ID | Question | Decision | Why |
|---|---|---|---|
| D1 | May a page keep a user's last data on the device (S3, S7)? | Yes. Per user and server, deleted on logout, account switch and 401, with an age limit per app. Update SEC-CLI-02 to say so. | Without it no cold start shows content before the network answers. |
| D2 | A manifest per app, or one manifest with shortcuts? | A manifest per app. | Each app gets its own icon, colors and splash. Shortcuts still start the one `/qapps` app. |
| D3 | A session that survives restarts, or a device key (S4)? | A device key per installed app, as the mobile app's device login keys. | It also covers a long idle phone and an expired session, and the server keeps no more session state. |
