# App startup at native speed

The plan to make an app open as fast as a native phone app: on Android, from the installed PWA and from the
[mobile app](../crates/moqui-mobile/README.md). Plan only; nothing here is built.

## Targets

The goal is an instant start: content in 50 ms. Each target is from the tap on the launcher icon, measured on a
mid-range Android phone on Wi-Fi.

| Measure | Cold start (no process) | Warm start (process alive) |
|---|---|---|
| First frame in the app's own colors | 30 ms | 30 ms |
| Last-known content shown and usable | 50 ms with S7, 150 ms without | 50 ms |
| Fresh content shown | 1 s | 1 s |

The floors, from the [prototype](#prototype):

- **First frame, 30 ms in both cases.** Android draws the starting window from the app's theme before the app's
  process runs. So the first frame needs only the right colors and icon (S1, S6).
- **Warm content, 50 ms.** On a resume the page is still in memory and shows with no reload (S6). A reload from the
  caches of S2 and S3 paints content in 52 ms on a CPU slowed 4 times.
- **Cold content on the web path, about 130 ms.** A new browser process paints its first frame at 120 to 140 ms on
  a CPU slowed 4 times, whatever the page: the service worker, the page size and the shell libraries change it by
  less than 20 ms. So cold content in 50 ms needs content drawn without the WebView (S7).

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

## Prototype

[scripts/app-startup-proto](../scripts/app-startup-proto/README.md) measures the steps in headless Chromium 1248 with
a local server: 400 ms for the page (TLS, session check, render) and 300 ms for the data. Each page loads the
`/qapps` shell libraries (Vue, Quasar). 10 runs per row, 2026-11-27. Cold is a new browser process; reload is the same
process. The time is from navigation start to the paint of the first content row, median / p90, in ms.

| Variant | CPU 1×, cold | CPU 1×, reload | CPU 4×, cold | CPU 4×, reload |
|---|---|---|---|---|
| a: now (network, blocking libraries) | 748 / 764 | 732 / 732 | 836 / 852 | 804 / 816 |
| b: S2 + S3 (cached page, `localStorage` drawn inline) | 60 / 68 | 44 / 48 | 136 / 172 | 64 / 128 |
| d: the cached page has the content in it | 56 / 64 | 44 / 48 | 144 / 188 | 56 / 68 |
| e: as d, first screen only, no libraries | 48 / 56 | 32 / 48 | 128 / 144 | 52 / 92 |
| f: as b, no service worker (HTTP cache) | 40 / 52 | 432 / 440 | 136 / 168 | 484 / 512 |

What it proves:

- S2 and S3 paint content in the first frame, 6 times sooner than now on a slow CPU and 12 to 15 times on a fast one, and the network no longer changes it.
- On a fast CPU, the cold start is 44 to 60 ms and a reload 32 to 44 ms: the 50 ms target.
- Shell libraries loaded with `defer` and an async stylesheet delay the first paint by at most 12 ms (b against e).
- The service worker costs 10 to 20 ms on a fast CPU and nothing measurable on a slow one (b against f).
- `stale-while-revalidate` does not apply to a page in Chromium: a page from the HTTP cache waits for the network on
  each reload (f), and an earlier run with it paints at 436 ms. So S2 needs the service worker.
- On a CPU slowed 4 times, a cold start is 120 to 140 ms in each variant: the new process is the floor, not the page.

Not proved: a phone. CPU slowdown is not a phone, and a browser process is not a WebView in a cold app process. The
Android emulator did not run here (the device panel needs an ffmpeg with libx264). S0 measures on a phone, and S7 is
not built.

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
- Load each script with `defer` and each stylesheet the first frame does not need after the first paint.
- This keeps SEC-CLI-02: the cache still has no answer that depends on who is signed in.

**Check:** with the server stopped, an installed app shows its shell document. A spec checks that the cached shell
document is the same for two users.

### S3. Last-known content

- A page keeps the data of its first screen in `localStorage`, under the user and the server. An inline script at the
  end of the shell document draws it before the first paint, with no other script. Then the page asks for fresh data
  and changes only what changed. More data than the first screen goes in IndexedDB, read after the first paint.
- `localStorage` and not IndexedDB for the first screen: it is synchronous, so the content is in the first frame. In a
  first run, content read from IndexedDB painted one frame after the first paint (120 ms, then 133 ms).
- The shell document stays the same for each user (variant b). Keeping the page with the content in it (variant d)
  is no faster and puts user data in the service worker's cache.
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

**Check:** on a cold start, the content shows in 50 ms, and the change to the WebView shows no flash.

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
