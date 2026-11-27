#!/usr/bin/env python3
# This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
"""LocationShareWalk's phone half (plans/archive/Location-Sharing.md LL7; docs/ui/Mobile-Client.md, Live location), over the
WebView's DevTools socket of a debug build and Android's own UI (uiautomator), as scripts/android-location-share.sh runs
it once the server half (the tools acceptance spec LocationShareWalkFixtures) serves and the app is started with no
account:

- signs in as loc.walk.ann (the bridge's signIn), opens Maps from the shell's drawer (Browse, the server's page) and
  shares her location with Walk family from Share my location, the emulator's position set by `adb emu geo fix`;
- answers the app's own confirmation dialog ("Send your location from this phone to Walk family (3 members) until
  ...?") with Send, as a person taps it, once Send is enabled;
- sees the foreground notification ("Sharing your location with Walk family ... until ..."), Browse's page saying the
  phone sends it, and Ben (loc.walk.ben, the server read as he reads it) given the phone's position; the position moved
  with the app in the background, and Ben given the new one;
- taps the notification's Stop in the notification shade, and sees the share end: Ben's read lists it no more, the
  notification is gone, and Ann's own read has no active share.

Each step waits a bounded time and the walk fails at the first that does not happen, naming it.

    python3 -P android-location-share.py --devtools PORT --serial SERIAL --adb ADB --port SERVER_PORT --password PASS
"""
import argparse
import base64
import json
import re
import subprocess
import sys
import time
import urllib.request
import xml.etree.ElementTree as ElementTree

import websocket

args = argparse.ArgumentParser()
args.add_argument("--devtools", type=int, required=True)
args.add_argument("--serial", required=True)
args.add_argument("--adb", required=True)
args.add_argument("--port", type=int, required=True)
args.add_argument("--password", required=True)
opts = args.parse_args()

started = time.time()
DEVICE_SERVER = f"http://localhost:{opts.port}"
HOST_SERVER = f"http://127.0.0.1:{opts.port}"
OWNER = "loc.walk.ann"
VIEWER = "loc.walk.ben"
# the emulator's positions (longitude, latitude), far enough apart that the share's latest moves
HOME = (-122.68, 45.52)
MOVED = (-122.6655, 45.5312)
APP = "org.moqui.mobile"


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


ws = None
next_id = 0


def connect():
    """The DevTools socket of the app's page (adb forward to its WebView's socket): the shell's, or a server page's."""
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
            page = next((p for p in pages if p.get("type") == "page"), None)
        except OSError:
            page = None
        if page is None:
            time.sleep(0.5)
    if page is None:
        fail("no page in the app's WebView")
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


def wait_for(what, check, timeout):
    """check() (an expression evaluated in the page, or a function here) until truthy; its value. Fails the walk naming
    what did not happen, with the last value."""
    deadline = time.time() + timeout
    value = None
    while time.time() < deadline:
        try:
            value = check() if callable(check) else evaluate(check)
        except (websocket.WebSocketException, OSError) as err:
            value = {"__error": str(err)}
        if value and not (isinstance(value, dict) and "__error" in value):
            return value
        time.sleep(0.5)
    fail(f"{what} (in {timeout} s; last: {json.dumps(value)[:400]})")


def click(selector):
    return evaluate(f"(() => {{ const el = document.querySelector({json.dumps(selector)}); if (el) el.click(); return !!el; }})()")


# ---- the server, as this machine reads it (Basic auth) ----


def shares_of(username):
    """The live location shares Browse reads for the user (its locationShares transition): their own and those shared
    with them, each with its latest position as that user may see it."""
    request = urllib.request.Request(HOST_SERVER + "/apps/maps/Browse/locationShares")
    token = base64.b64encode(f"{username}:{opts.password}".encode()).decode()
    request.add_header("Authorization", "Basic " + token)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response).get("locationShares") or []


def shared_with(username, share_id):
    return next((one for one in shares_of(username) if one.get("shareId") == share_id), None)


def near(latest, at):
    """Whether a share's latest position is the emulator's (longitude, latitude), to about 10 m."""
    if not isinstance(latest, dict):
        return False
    lat = latest.get("latitude", latest.get("lat"))
    lon = latest.get("longitude", latest.get("lon"))
    return lat is not None and lon is not None and abs(float(lat) - at[1]) < 1e-4 and abs(float(lon) - at[0]) < 1e-4


# ---- Android's own UI ----


def screen_nodes():
    """The nodes of what Android shows now (uiautomator's dump): text, content description, class, clickable, enabled
    and the centre of each."""
    adb("shell", "uiautomator", "dump", "/sdcard/location-walk-ui.xml", check=False)
    raw = adb("exec-out", "cat", "/sdcard/location-walk-ui.xml", check=False)
    try:
        root = ElementTree.fromstring(raw[raw.index("<"):] if "<" in raw else "<hierarchy/>")
    except ElementTree.ParseError:
        return []
    nodes = []
    for node in root.iter("node"):
        bounds = re.findall(r"\d+", node.get("bounds", ""))
        if len(bounds) != 4:
            continue
        x1, y1, x2, y2 = (int(b) for b in bounds)
        nodes.append(
            {
                "text": node.get("text", ""),
                "desc": node.get("content-desc", ""),
                "cls": node.get("class", ""),
                "package": node.get("package", ""),
                "clickable": node.get("clickable") == "true",
                "enabled": node.get("enabled") == "true",
                "x": (x1 + x2) // 2,
                "y": (y1 + y2) // 2,
            }
        )
    return nodes


def node_with(nodes, pattern, **match):
    regex = re.compile(pattern, re.I)
    for node in nodes:
        if (regex.search(node["text"]) or regex.search(node["desc"])) and all(node.get(k) == v for k, v in match.items()):
            return node
    return None


def tap(node):
    adb("shell", "input", "tap", str(node["x"]), str(node["y"]))


def notification_text():
    """The app's posted notifications as dumpsys lists them (their titles and texts, unredacted)."""
    dump = adb("shell", "dumpsys", "notification", "--noredact", check=False)
    texts = []
    for block in dump.split("NotificationRecord(")[1:]:
        if f"pkg={APP}" not in block:
            continue
        texts += re.findall(r"android\.(?:title|text|bigText)=String \((.*?)\)\s*$", block, re.M)
    return " | ".join(texts)


def geo_fix(at):
    adb("emu", "geo", "fix", str(at[0]), str(at[1]))


share_id = None
try:
    geo_fix(HOME)
    connect()
    # ---- signed in, and Maps' Browse opened ----
    wait_for("the shell's bridge", "!!(window.moquiDesktop && moquiDesktop.signIn && document.querySelector('.m-shell-sign-in, .q-layout'))", 60)
    signed = evaluate(
        "moquiDesktop.signIn("
        + json.dumps({"server": DEVICE_SERVER, "username": OWNER, "password": opts.password, "deviceMode": "personal"})
        + ").then((answer) => JSON.stringify(answer || 'ok'), (err) => 'error: ' + String(err && (err.message || err)))"
    )
    if str(signed).startswith("error"):
        fail(f"sign-in as {OWNER} at {DEVICE_SERVER}: {signed}")
    say(f"signed in as {OWNER} at {DEVICE_SERVER}")
    wait_for("the drawer's Maps", "!!document.querySelector('.m-shell-maps')", 60)
    click(".m-shell-maps")
    # the server's page in the same webview: Browse, its configuration read
    browse = """(() => { const find = (vnode) => { if (!vnode) return null; const c = vnode.component;
        if (c && c.type && c.type.name === 'MapsBrowse') return c.proxy;
        if (c) return find(c.subTree);
        return Array.isArray(vnode.children) ? vnode.children.map(find).find(Boolean) || null : null; };
        return window.moqui && moqui.webrootVue ? find(moqui.webrootVue.$.subTree) : null; })()"""
    deadline = time.time() + 60
    while time.time() < deadline:
        try:
            if evaluate(f"!!(({browse}) && ({browse}).effective && window.moquiDesktop && moquiDesktop.location)") is True:
                break
        except (websocket.WebSocketException, OSError):
            connect()
        time.sleep(0.5)
    else:
        fail("Maps' Browse opened from the drawer, with moquiDesktop.location")
    say("Browse open in the app, moquiDesktop.location there")

    # ---- Share my location, with Walk family ----
    click("#MapsLiveButton")
    wait_for("Share my location", "!!document.querySelector('#MapsShareLocation')", 20)
    click("#MapsShareLocation")
    wait_for("the share dialog's choices read", f"(() => {{ const vm = {browse}; return !!(vm && vm.shareTargets.loaded); }})()", 20)
    chosen = evaluate(
        f"""(() => {{ const vm = {browse}; const group = vm.shareTargets.groups.find((one) => one.userGroupId === 'LOC_WALK_FAMILY');
        if (!group) return null; vm.shareForm.groups = [group.userGroupId]; return group.label; }})()"""
    )
    if not chosen:
        fail("Walk family offered among the groups Ann is in")
    say(f"Share my location: {chosen}, 1 h, exact")
    click("#MapsShareCreate")

    # ---- the app's own confirmation dialog, answered Send ----
    dialog = wait_for(
        "the app's confirmation dialog (Send your location from this phone to ...)",
        lambda: (lambda nodes: {"words": node_with(nodes, r"Send your location from this phone")["text"], "nodes": nodes}
                 if node_with(nodes, r"Send your location from this phone") else None)(screen_nodes()),
        60,
    )
    say(f"the app asks: {dialog['words']!r}")
    if "Walk family" not in dialog["words"]:
        fail(f"the dialog names Walk family (got {dialog['words']!r})")
    # Send is enabled a second after the dialog has the window's focus
    send = wait_for("the dialog's Send enabled", lambda: node_with(screen_nodes(), r"^send$", enabled=True), 15)
    tap(send)
    say("Send tapped")
    share_id = wait_for(
        "Browse saying the phone sends the share (the app answered started)",
        f"(() => {{ const vm = {browse}; return vm && vm.shareMade && vm.nativeSending.includes(vm.shareMade.shareId) ? vm.shareMade.shareId : null; }})()",
        30,
    )
    say(f"share {share_id}: sent from this phone")

    # ---- the foreground notification, and the position on the server ----
    shown = wait_for("the foreground notification (Sharing your location with ...)",
                     lambda: notification_text() if "Sharing your location" in notification_text() else None, 30)
    say(f"notification: {shown!r}")
    if "Walk family" not in shown:
        fail(f"the notification names Walk family (got {shown!r})")
    seen = wait_for(
        "Ben given the phone's position (his read of the share)",
        lambda: (lambda one: one if one and near(one.get("latest"), HOME) else None)(shared_with(VIEWER, share_id)),
        90,
    )
    say(f"Ben sees {seen.get('label')!r} at the phone's position")
    # the app in the background: the service goes on sending, and Ben gets the moved position
    adb("shell", "input", "keyevent", "KEYCODE_HOME")
    time.sleep(1)
    geo_fix(MOVED)
    wait_for(
        "Ben given the position moved while the app is in the background",
        lambda: (lambda one: one if one and near(one.get("latest"), MOVED) else None)(shared_with(VIEWER, share_id)),
        120,
    )
    say("moved with the app in the background: Ben sees the new position")

    # ---- the notification's Stop ----
    adb("shell", "cmd", "statusbar", "expand-notifications")
    time.sleep(1.5)
    stop = None
    deadline = time.time() + 20
    while time.time() < deadline and stop is None:
        nodes = screen_nodes()
        stop = node_with(nodes, r"^stop$")
        if stop is None:
            # the notification collapsed: expanded by a swipe down on it, which shows its actions
            card = node_with(nodes, r"Sharing your location")
            if card:
                adb("shell", "input", "swipe", str(card["x"]), str(card["y"]), str(card["x"]), str(card["y"] + 300), "300")
            time.sleep(1)
    if stop is None:
        fail("the notification's Stop in the notification shade")
    tap(stop)
    say("the notification's Stop tapped")
    wait_for("Ben's read no longer lists the share", lambda: shared_with(VIEWER, share_id) is None or None, 60)
    wait_for("the notification gone", lambda: "Sharing your location" not in notification_text() or None, 30)
    wait_for(
        "Ann's own read has no active share",
        lambda: not any(one.get("role") == "owner" for one in shares_of(OWNER)) or None,
        30,
    )
    adb("shell", "cmd", "statusbar", "collapse", check=False)
    say("stopped: Ben sees it no more, the notification is gone, Ann has no active share")
    share_id = None
    say("PASSED: LocationShareWalk (the phone)")
finally:
    # the UI dump, which can hold the dialog's and the notification's words, removed from the device
    adb("shell", "rm", "-f", "/sdcard/location-walk-ui.xml", check=False)
    if ws is not None:
        ws.close()
