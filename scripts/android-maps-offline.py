#!/usr/bin/env python3
# This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
"""MapsOfflinePackage, the device half of the offline map walk (plans/archive/Maps-Offline.md OF4), over the WebView's DevTools
socket of a debug build, as scripts/android-maps-offline.sh runs it once the server half (the tools acceptance spec
MapsOfflinePackage) serves its package and the app is started with no account:

- signs in to the server as maps.offline (the bridge's signIn, as the sign-in form calls it), opens Offline maps from
  the shell's drawer and downloads the package (Download: packages_download, Android's PackageDownloadWorker) until it
  is installed;
- cuts the network (airplane mode, Wi-Fi and mobile data off, and the adb reverse the server is reached by removed) and
  starts the app again, its process ended in the background as Android ends one (a connection the app opened through
  adb reverse outlives the rule's removal, so only a new process has none), whose shell starts offline from the account's store; the drawer's Maps then opens the offline
  screen;
- opens the package: the map draws its tiles (the earth and water colours of the packaged light style in the map's
  canvases), says it is offline, and finds a saved place, a postal code, an Overture place and an address in the package;
- saves a place, which waits in the outbox and is not on the server; then restores the network and waits until the
  outbox has sent it and the server lists it among the user's places.

Each step waits a bounded time and the walk fails at the first that does not happen, naming it. The network is restored
however the walk ends.

    python3 -P android-maps-offline.py --devtools PORT --serial SERIAL --adb ADB --port SERVER_PORT --user NAME --password PASS
"""
import argparse
import base64
import json
import subprocess
import sys
import time
import urllib.parse
import urllib.request

import websocket

args = argparse.ArgumentParser()
args.add_argument("--devtools", type=int, required=True)
args.add_argument("--serial", required=True)
args.add_argument("--adb", required=True)
args.add_argument("--port", type=int, required=True)
args.add_argument("--user", required=True)
args.add_argument("--password", required=True)
opts = args.parse_args()

started = time.time()
# the server as the app reaches it (adb reverse), and as this machine does
DEVICE_SERVER = f"http://localhost:{opts.port}"
HOST_SERVER = f"http://127.0.0.1:{opts.port}"
# a name of this run's, so a server kept from an earlier run cannot have it
PLACE = f"Walk offline pin {int(started) % 100000}"


def say(text):
    print(f"[{time.time() - started:6.1f}s] {text}", flush=True)


def fail(text):
    say("FAILED: " + text)
    sys.exit(1)


def adb(*command, check=True):
    done = subprocess.run([opts.adb, "-s", opts.serial, *command], capture_output=True, text=True, timeout=60)
    if check and done.returncode != 0:
        fail(f"adb {' '.join(command)}: {done.stderr.strip() or done.stdout.strip()}")
    return done.stdout


APP = "org.moqui.mobile"
ws = None
next_id = 0


def connect():
    """The DevTools socket of the shell's page in the app's process now (adb forward to its WebView's socket)."""
    global ws
    deadline = time.time() + 30
    pid = ""
    while time.time() < deadline and not pid:
        pid = adb("shell", "pidof", APP, check=False).strip()
        if not pid:
            time.sleep(0.5)
    if not pid:
        fail("the app is not running")
    adb("forward", f"tcp:{opts.devtools}", f"localabstract:webview_devtools_remote_{pid}")
    page = None
    while time.time() < deadline and page is None:
        try:
            pages = json.load(urllib.request.urlopen(f"http://127.0.0.1:{opts.devtools}/json", timeout=5))
            page = next((p for p in pages if p.get("type") == "page" and "moqui-shell" in p.get("url", "")), None)
        except OSError:
            page = None
        if page is None:
            time.sleep(0.5)
    if page is None:
        fail("no page of the shell in the app's WebView")
    if ws is not None:
        ws.close()
    # no Origin header: the DevTools socket refuses one it was not started to allow
    ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=30, suppress_origin=True)


def cdp(method, **params):
    global next_id
    next_id += 1
    ws.send(json.dumps({"id": next_id, "method": method, "params": params}))
    while True:
        message = json.loads(ws.recv())
        if message.get("id") == next_id:
            return message


def evaluate(expression):
    answer = cdp("Runtime.evaluate", expression=expression, returnByValue=True, awaitPromise=True)
    result = answer.get("result", {})
    if "exceptionDetails" in result:
        details = result["exceptionDetails"]
        return {"__error": (details.get("exception") or {}).get("description") or details.get("text")}
    return result.get("result", {}).get("value")


def wait_for(what, expression, timeout):
    """Evaluate until truthy; its value. Fails the walk naming what did not happen, with the last value."""
    deadline = time.time() + timeout
    value = None
    while time.time() < deadline:
        try:
            value = evaluate(expression)
        except (websocket.WebSocketException, OSError) as err:
            value = {"__error": str(err)}
        if value and not (isinstance(value, dict) and "__error" in value):
            return value
        time.sleep(0.5)
    fail(f"{what} (in {timeout} s; last: {json.dumps(value)[:400]})")


def click(selector):
    return evaluate(f"(() => {{ const el = document.querySelector({json.dumps(selector)}); if (el) el.click(); return !!el; }})()")


def fill(selector, value):
    """Set a q-input's value as typing does (its v-model reads the input event)."""
    return evaluate(
        f"""(() => {{ const input = document.querySelector({json.dumps(selector)}); if (!input) return false;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, {json.dumps(value)});
        input.dispatchEvent(new Event('input', {{ bubbles: true }})); return true; }})()"""
    )


def server_places(query):
    """The user's places the server's Browse search lists for the text, as this machine asks it (Basic auth)."""
    url = HOST_SERVER + "/apps/maps/Browse/search?" + urllib.parse.urlencode({"q": query})
    request = urllib.request.Request(url)
    token = base64.b64encode(f"{opts.user}:{opts.password}".encode()).decode()
    request.add_header("Authorization", "Basic " + token)
    with urllib.request.urlopen(request, timeout=30) as response:
        results = json.load(response).get("results") or []
    return [r for r in results if r.get("kind") == "place" and str(r.get("label", "")) == query]


def default_network():
    """Android's default network as dumpsys names it: a number, or none."""
    active = adb("shell", "dumpsys", "connectivity", check=False)
    line = next((line for line in active.splitlines() if "Active default network" in line), "")
    return line.split(":", 1)[-1].strip() or "?"


def network(on):
    """The device's network on or off, and the adb reverse the app reaches the server by (adb's own channel, which no
    network setting cuts); then waits for Android to say so (a default network, or none)."""
    if on:
        adb("shell", "cmd", "connectivity", "airplane-mode", "disable", check=False)
        adb("shell", "svc", "wifi", "enable", check=False)
        adb("shell", "svc", "data", "enable", check=False)
        adb("reverse", f"tcp:{opts.port}", f"tcp:{opts.port}")
    else:
        adb("reverse", "--remove", f"tcp:{opts.port}", check=False)
        adb("shell", "cmd", "connectivity", "airplane-mode", "enable", check=False)
        adb("shell", "svc", "wifi", "disable", check=False)
        adb("shell", "svc", "data", "disable", check=False)
    deadline = time.time() + 30
    while time.time() < deadline and (default_network() == "none") == on:
        time.sleep(0.5)
    return default_network()


# the map's canvases: how many sampled pixels have the light style's earth (#e2dfda) and water (#80deea), which only the
# package's tiles draw (the outline and the background have other colours)
DRAWN = """(() => {
    const root = document.querySelector('#MapsOfflineMap');
    if (!root || !root.querySelector('.ol-viewport')) return null;
    const near = (d, i, rgb) => Math.abs(d[i] - rgb[0]) <= 4 && Math.abs(d[i + 1] - rgb[1]) <= 4 && Math.abs(d[i + 2] - rgb[2]) <= 4;
    let earth = 0, water = 0, canvases = 0;
    for (const canvas of root.querySelectorAll('canvas')) {
        if (!canvas.width || !canvas.height) continue;
        const context = canvas.getContext('2d');
        if (!context) continue;
        canvases++;
        const data = context.getImageData(0, 0, canvas.width, canvas.height).data;
        const step = Math.max(1, Math.floor((canvas.width * canvas.height) / 20000));
        for (let p = 0; p < canvas.width * canvas.height; p += step) {
            const i = p * 4;
            if (data[i + 3] < 200) continue;
            if (near(data, i, [226, 223, 218])) earth++;
            else if (near(data, i, [128, 222, 234])) water++;
        }
    }
    return earth > 20 && water > 20 ? { canvases, earth, water } : null;
})()"""


def search(query, expect, what):
    fill(".m-maps-offline-search input", query)
    time.sleep(0.3)
    click(".m-maps-offline-search button[type=submit]")
    found = wait_for(
        f"the offline search for {query!r} finds {what}",
        f"""(() => {{ const texts = Array.from(document.querySelectorAll('.m-maps-offline-result')).map((el) => el.innerText);
        const hit = texts.find((text) => {json.dumps(expect)}.every((part) => text.includes(part)));
        return hit ? hit.replace(/\\s+/g, ' ') : null; }})()""",
        20,
    )
    say(f"search {query!r}: {found}")


restored = False
try:
    connect()
    # ---- signed in, and the package downloaded ----
    wait_for("the shell's bridge", "!!(window.moquiDesktop && moquiDesktop.signIn && document.querySelector('.m-shell-sign-in, .q-layout'))", 60)
    signed = evaluate(
        "moquiDesktop.signIn("
        + json.dumps({"server": DEVICE_SERVER, "username": opts.user, "password": opts.password, "deviceMode": "personal"})
        + ").then((answer) => JSON.stringify(answer || 'ok'), (err) => 'error: ' + String(err && (err.message || err)))"
    )
    if str(signed).startswith("error"):
        fail(f"sign-in as {opts.user} at {DEVICE_SERVER}: {signed}")
    say(f"signed in as {opts.user} at {DEVICE_SERVER}")
    wait_for("the drawer's Offline maps (an account with the Maps app)", "!!document.querySelector('.m-shell-maps-offline')", 60)
    click(".m-shell-maps-offline")
    row = wait_for(
        "the server's package listed with Download",
        "(() => { const row = document.querySelector('.m-maps-offline-row'); return row && row.querySelector('.m-maps-offline-download') ? row.dataset.package : null; })()",
        60,
    )
    say(f"package {row} listed by the server")
    # the package's row on the offline screen, found by its id given as JSON (never pasted into a selector)
    row_js = f"Array.from(document.querySelectorAll('.m-maps-offline-row')).find((el) => el.dataset.package === {json.dumps(row)})"
    in_row = lambda selector: f"(() => {{ const row = {row_js}; return row ? row.querySelector({json.dumps(selector)}) : null; }})()"
    evaluate(f"(() => {{ const button = {in_row('.m-maps-offline-download')}; if (button) button.click(); return !!button; }})()")
    downloaded = wait_for(
        "the package downloaded and installed (Open)",
        f"""(() => {{ const row = {row_js};
        if (!row) return null; if (row.querySelector('.m-maps-offline-open')) return 'installed';
        const failed = row.querySelector('.m-maps-offline-state.text-negative'); return failed ? 'failed: ' + failed.innerText : null; }})()""",
        240,
    )
    if downloaded != "installed":
        fail(f"the package's download: {downloaded}")
    installed = evaluate(
        f"Promise.resolve(moquiDesktop.packages.status()).then((all) => (all || []).find((one) => one.packageId === {json.dumps(row)}) || null)"
    )
    if not isinstance(installed, dict) or "__error" in installed:
        fail(f"the device's package status lists no package {row} after its download (got {json.dumps(installed)[:300]})")
    say(f"installed: revision {installed.get('revision')}, {installed.get('size')} bytes")

    # ---- the network cut, the shell started again offline ----
    if network(False) != "none":
        fail("the network cut: Android still has a default network after 30 s")
    say("network cut: airplane mode, Wi-Fi and data off (no default network), adb reverse removed")
    # put in the background and ended as Android ends a background app (a force-stop would have the app say so)
    adb("shell", "input", "keyevent", "KEYCODE_HOME")
    time.sleep(1)
    adb("shell", "am", "kill", APP)
    deadline = time.time() + 15
    while time.time() < deadline and adb("shell", "pidof", APP, check=False).strip():
        time.sleep(0.5)
        adb("shell", "am", "kill", APP, check=False)
    if adb("shell", "pidof", APP, check=False).strip():
        fail("the app's process did not end in the background")
    adb("shell", "am", "start", "-W", "-n", f"{APP}/.MainActivity")
    connect()
    wait_for("the shell started offline (its offline chip)", "!!document.querySelector('.m-shell-offline')", 60)
    say("the app started again, its shell offline")
    wait_for("the drawer's Maps", "!!document.querySelector('.m-shell-maps')", 30)
    click(".m-shell-maps")
    wait_for("the drawer's Maps opening the offline screen, offline", "!!document.querySelector('.m-maps-offline-chip')", 30)
    wait_for("the installed package offered (Open)", f"!!{in_row('.m-maps-offline-open')}", 30)
    evaluate(f"(() => {{ const button = {in_row('.m-maps-offline-open')}; if (button) button.click(); return !!button; }})()")
    say("the drawer's Maps opened the offline screen; Open")

    # ---- the map drawn from the package, and searched ----
    drawn = wait_for("the package's tiles drawn on the map", DRAWN, 60)
    say(f"map drawn: {drawn}")
    built = evaluate("(document.querySelector('.m-maps-offline-built') || {}).innerText || ''")
    mode = evaluate("((document.querySelector('.m-maps-offline-mode .q-chip__content') || {}).innerText || '').replace(/cloud_off|offline_pin/, '')")
    if "Offline" not in built or "Offline" not in mode:
        fail(f"the opened package says it is offline and when it was built (got {built!r}, {mode!r})")
    say(f"says: {mode.strip()}; {built.strip()}")
    problems = evaluate("(document.querySelector('.m-maps-offline [role=alert]') || {}).innerText || ''")
    if problems:
        fail(f"the offline screen shows an error: {problems}")
    search("Walk Harbor Cafe", ["Walk Harbor Cafe", "Your saved place"], "the saved place")
    search("02110", ["02110", "Boston"], "the postal code")
    search("Walk Harbor Bakery", ["Walk Harbor Bakery", "bakery"], "the Overture place")
    search("100 Harbor Street", ["100", "Harbor", "Address"], "the Overture address")

    # ---- a place saved offline: queued, not on the server ----
    click(".m-maps-offline-result")
    fill(".m-maps-offline-save input", PLACE)
    time.sleep(0.5)
    click(".m-maps-offline-save button[type=submit]")
    wait_for(
        "the place saved offline waiting in the outbox",
        f"""moquiDesktop.outbox.list().then((ops) => (ops || []).some((op) => op.kind === 'map-place' && op.state !== 'acked'
            && JSON.stringify(op.payload).includes({json.dumps(PLACE)})))""",
        20,
    )
    wait_for("the queued place listed on the screen", f"((document.querySelector('.m-maps-offline-queued') || {{}}).innerText || '').includes({json.dumps(PLACE)})", 10)
    time.sleep(3)
    if server_places(PLACE):
        fail("the place reached the server while the network was cut")
    say(f"{PLACE!r} queued in the outbox, not on the server")

    # ---- the network back: the outbox sends it ----
    back = network(True)
    restored = True
    say(f"network restored (default network {back}), adb reverse again")
    wait_for(
        "the outbox sending the place (no map-place left unsent)",
        "moquiDesktop.outbox.list().then((ops) => !(ops || []).some((op) => op.kind === 'map-place' && op.state !== 'acked'))",
        120,
    )
    deadline = time.time() + 30
    on_server = []
    while time.time() < deadline and not on_server:
        on_server = server_places(PLACE)
        if not on_server:
            time.sleep(1)
    if not on_server:
        fail(f"{PLACE!r} is not among the user's places on the server")
    say(f"sent: the server lists {PLACE!r} ({on_server[0].get('id')})")
    say("PASSED: MapsOfflinePackage")
finally:
    if not restored:
        network(True)
    if ws is not None:
        ws.close()
