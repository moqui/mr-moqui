/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
(function () {
Quasar.QDialog.props.transitionDuration.default = 40;
const until = async (check, what, ms) => {
    const end = Date.now() + (ms || 5000);
    await Vue.nextTick();
    for (;;) {
        const value = check();
        if (value) return value;
        if (Date.now() > end) throw new Error("waited for " + what);
        await new Promise((resolve) => setTimeout(resolve, 10));
    }
};
const settle = async () => { await new Promise((resolve) => setTimeout(resolve, 0)); await Vue.nextTick(); };
const button = (scope, label) => Array.from((scope || document).querySelectorAll("button")).find((el) => el.textContent.trim() === label);

// ---- the package on the device, its files answered as the app's scheme answers them (206 slices by range) ----
const PACKAGE = "/__moqui-app/maps/P7/";
const manifest = {
    schemaVersion: 1,
    package: { packageId: "P7", name: "Harbor", bbox: [-71.2, 42.2, -70.9, 42.5], minZoom: 0, maxZoom: 14, revision: 3, builtDate: "2026-11-20T10:00:00.000Z" },
    configuration: { profile: { basemaps: [{ id: "OUTLINE_NE110", label: "Country outlines", kind: "outline", url: "outline.json", attribution: "Natural Earth" }] },
        layers: [], extent: [-71.2, 42.2, -70.9, 42.5] },
    sources: [{ kind: "basemap", member: "basemap/LIGHT.pmtiles", attribution: "Protomaps",
        sourceAttribution: '<a href="https://www.openstreetmap.org/copyright">&copy; OpenStreetMap</a><img src=x onerror="window.mapsAttack=1">', license: "ODbL-1.0" }],
    members: ["outline.json", "places.geojson", "postal.json", "overture-places.json", "overture-addresses.json"].map((path) => ({ path, size: 1, sha256: "x" })),
    notice: "This package holds what its owner could read when it was built."
};
const files = {
    "manifest.json": manifest,
    "outline.json": { type: "Topology", arcs: [], objects: { countries: { type: "GeometryCollection", geometries: [] } } },
    "places.geojson": { type: "FeatureCollection", features: [
        { type: "Feature", id: "p1", geometry: { type: "Point", coordinates: [-71.05, 42.36] }, properties: { name: "Ferry dock" } },
        { type: "Feature", id: "p2", geometry: { type: "Point", coordinates: [2.35, 48.85] }, properties: { name: "Ferry in Paris" } }] },
    "postal.json": { rows: [{ code: "02110", displayCode: "02110", country: "US", places: [{ name: "Boston", adminName1: "MA" }], lon: -71.05, lat: 42.357 }] },
    "overture-places.json": { rows: [{ placeName: "Harbor Café", basicCategory: "cafe", locality: "Boston", longitude: -71.04, latitude: 42.35 }] }
};
const asked = [];
const realFetch = window.fetch;
window.fetch = (input, init) => {
    const url = new URL(typeof input === "string" ? input : input.url, location.href);
    if (url.origin === location.origin && url.pathname.startsWith(PACKAGE)) {
        const headers = new Headers((init && init.headers) || (typeof input === "string" ? {} : input.headers));
        asked.push({ path: url.pathname.slice(PACKAGE.length), range: headers.get("Range"), cache: init && init.cache });
        const body = files[url.pathname.slice(PACKAGE.length)];
        if (body === undefined) return Promise.resolve(new Response("Not in this offline map", { status: 404 }));
        const bytes = new TextEncoder().encode(JSON.stringify(body));
        const range = /^bytes=(\d+)-(\d+)$/.exec(headers.get("Range") || "");
        if (!range) return Promise.resolve(new Response(bytes, { status: 200 }));
        // a slice of at most 7 bytes, as a host answers a range with a shorter one, which its Content-Range says
        const start = Number(range[1]), end = Math.min(Number(range[2]), start + 6, bytes.length - 1);
        return Promise.resolve(new Response(bytes.slice(start, end + 1), { status: 206, headers: { "Content-Range": "bytes " + start + "-" + end + "/" + bytes.length } }));
    }
    return realFetch(input, init);
};

// ---- the host: its packages (the server's list and the device's) and its outbox ----
const calls = [];
const host = {
    server: [{ packageId: "P7", name: "Harbor", revision: 4, downloadUrl: "/apps/maps/Offline/package?packageId=P7&revision=4", sizeText: "12 MB",
        builtText: "Nov 27, 2026", zoomsText: "0–14", statusText: "Ready" },
    { packageId: "P8", name: "Cape", revision: 1, downloadUrl: "/apps/maps/Offline/package?packageId=P8&revision=1", sizeText: "3 MB", zoomsText: "0–12" }],
    device: [{ packageId: "P7", name: "Harbor", revision: 3, size: 1048576, builtDate: "2026-11-20T10:00:00.000Z", bbox: manifest.package.bbox,
        minZoom: 0, maxZoom: 14, installed: true }],
    ops: [], online: true
};
window.moquiDesktop = {
    pushDelivers: true,
    storageScope: "a1",
    packages: {
        list: () => { calls.push(["list"]); return Promise.resolve(host.online ? { server: host.server, device: host.device } : { server: null, error: "offline", device: host.device }); },
        status: () => Promise.resolve(host.device),
        download: (packageId, revision, wifiOnly) => {
            calls.push(["download", packageId, revision, wifiOnly]);
            host.device = host.device.concat([{ packageId, installed: false, download: { state: "downloading", revision, done: 1048576, total: 3145728 } }]);
            return Promise.resolve({ state: "queued", revision });
        },
        cancel: (packageId) => { calls.push(["cancel", packageId]); host.device = host.device.filter((row) => row.packageId !== packageId || row.installed); return Promise.resolve(true); },
        remove: (packageId) => { calls.push(["remove", packageId]); host.device = host.device.filter((row) => row.packageId !== packageId); return Promise.resolve(); },
        addresses: (packageId, text, options) => {
            calls.push(["addresses", packageId, text, options]);
            if (!/harbor/i.test(text)) return Promise.resolve({ valid: false, reasonCode: "NO_MATCH", candidates: [] });
            return Promise.resolve({ valid: true, candidates: [{ label: "12 Harbor Street, Boston", lon: -71.045, lat: 42.355, precisionEnumId: "GPP_ADDRESS_MATCH", sourceLicenses: "CC0" }] });
        }
    },
    outbox: {
        add: (kind, payload) => { calls.push(["outbox", kind, payload]); host.ops.push({ id: payload.clientRef, kind, state: "waiting", payload: JSON.stringify(payload) }); return Promise.resolve({}); },
        list: () => Promise.resolve(host.ops),
        kinds: () => Promise.resolve({ clientApi: 4, kinds: ["chat-send", "map-place"] }),
        retry: () => Promise.resolve(true), discard: () => Promise.resolve(true), on: () => () => {}
    },
    openRemote: (path) => { calls.push(["openRemote", path]); return Promise.resolve(); }
};
moqui.storageScope = "a1";
moqui.config = { qvt: { build: "runtime", handles: ["static-template"] } };
moqui.qvt.applyRenders();

// the screen mounted as the shell mounts a bundled screen, with Quasar and the client's components
const mount = () => {
    const holder = document.createElement("div");
    document.body.appendChild(holder);
    const app = Vue.createApp(moqui.bundledComponent("Shell/MapsOffline"));
    app.use(Quasar, { plugins: moqui.qvt.quasarPlugins(Quasar) });
    Object.keys(moqui.qvt.components).forEach((name) => app.component(name, moqui.qvt.components[name]));
    app.config.globalProperties.moqui = moqui;
    const vm = app.mount(holder);
    return { vm, holder, done: () => { app.unmount(); holder.remove(); } };
};
const row = (holder, id) => holder.querySelector(".m-maps-offline-row[data-package='" + id + "']");

test("the server's and the device's packages are one list: Download (only on Wi-Fi on the phone), Update, Cancel and Delete", async () => {
    const { vm, holder, done } = mount();
    try {
        await until(() => row(holder, "P8"), "the list");
        const harbor = row(holder, "P7");
        assert.ok(harbor.textContent.includes("A newer version is on the server"), harbor.textContent);
        assert.ok(button(harbor, "Open") && button(harbor, "Update"), "installed, and a newer revision on the server");
        assert.ok(!button(row(holder, "P8"), "Open") && button(row(holder, "P8"), "Download"));
        // the phone: only on Wi-Fi when asked
        assert.ok(holder.querySelector(".m-maps-offline-wifi"), "the phone's Wi-Fi choice");
        vm.wifiOnly = true;
        button(row(holder, "P8"), "Download").click();
        await until(() => calls.some((call) => call[0] === "download"), "the download");
        assert.deepEqual(calls.find((call) => call[0] === "download"), ["download", "P8", 1, true]);
        await until(() => row(holder, "P8").textContent.includes("Downloading: 1.0 MB of 3.0 MB"), "its progress");
        button(row(holder, "P8"), "Cancel").click();
        await until(() => calls.some((call) => call[0] === "cancel" && call[1] === "P8"), "the cancel");
        row(holder, "P7").querySelector(".m-maps-offline-delete").click();
        await until(() => document.querySelector(".q-dialog") && button(document.querySelector(".q-dialog"), "Delete"), "the question");
        button(document.querySelector(".q-dialog"), "Delete").click();
        await until(() => calls.some((call) => call[0] === "remove" && call[1] === "P7"), "the delete");
        await until(() => !document.querySelector(".q-dialog"), "the dialog to close");
    } finally {
        done();
        host.device = [{ packageId: "P7", name: "Harbor", revision: 3, size: 1048576, builtDate: "2026-11-20T10:00:00.000Z", bbox: manifest.package.bbox,
            minZoom: 0, maxZoom: 14, installed: true }];
    }
});

test("offline, a package opens from the device: it says so and when it was built, its sources as text, searched and saved to", async () => {
    host.online = false;
    moqui.setOffline(true);
    const { vm, holder, done } = mount();
    try {
        await until(() => row(holder, "P7"), "the device's packages");
        assert.ok(holder.querySelector(".m-maps-offline-chip"), "it says it is offline");
        assert.equal(row(holder, "P8"), null, "the server's list is not there offline");
        button(row(holder, "P7"), "Open").click();
        await until(() => vm.mapConfig, "the package opened");
        // the manifest read by range from the package's path, slice by slice, never from an HTTP cache
        const reads = asked.filter((one) => one.path === "manifest.json");
        assert.ok(reads.length > 1 && reads.every((one) => /^bytes=\d+-\d+$/.test(one.range) && one.cache === "no-store"), JSON.stringify(reads.slice(0, 3)));
        assert.equal(vm.mapConfig.packageBase, PACKAGE);
        assert.equal(vm.mapConfig.manifest.package.name, "Harbor");
        const built = holder.querySelector(".m-maps-offline-built").textContent;
        assert.ok(built.includes("Offline") && built.includes("built") && built.includes("2026"), built);
        await until(() => holder.querySelector(".m-maps-offline-source"), "the sources");
        const sources = holder.querySelector(".m-maps-offline-sources");
        assert.ok(sources.textContent.includes("© OpenStreetMap") && sources.textContent.includes("ODbL-1.0"), sources.textContent);
        assert.equal(sources.querySelector("img, a"), null, "an archive's notice is text, never markup");
        assert.equal(window.mapsAttack, undefined);
        // a search of the package's own files, within its region, and of its addresses through the host
        vm.query = "ferry";
        await vm.search();
        assert.deepEqual(vm.results.map((result) => result.label), ["Ferry dock"], "the place outside the region is not offered");
        vm.query = "02110";
        await vm.search();
        assert.ok(vm.results.some((result) => result.label === "02110" && result.caption.includes("Boston")), JSON.stringify(vm.results));
        vm.query = "harbor";
        await vm.search();
        const labels = vm.results.map((result) => result.label);
        assert.ok(labels.includes("Harbor Café") && labels.includes("12 Harbor Street, Boston"), labels.join(", "));
        assert.deepEqual(calls.filter((call) => call[0] === "addresses").pop().slice(1, 3), ["P7", "harbor"]);
        // a place saved offline: an outbox item the host sends when the server answers, by its clientRef
        vm.show(vm.results.find((result) => result.label === "Harbor Café"));
        assert.equal(vm.placeName, "Harbor Café");
        await vm.savePlace();
        const queued = calls.filter((call) => call[0] === "outbox").pop();
        assert.equal(queued[1], "map-place");
        assert.ok(/^[A-Za-z0-9_.:-]{1,64}$/.test(queued[2].clientRef), queued[2].clientRef);
        assert.deepEqual([queued[2].label, queued[2].longitude, queued[2].latitude], ["Harbor Café", -71.04, 42.35]);
        await until(() => holder.querySelector(".m-maps-offline-queued"), "the place waiting");
        assert.ok(holder.querySelector(".m-maps-offline-queued").textContent.includes("Waits for the server"));
    } finally {
        done();
        host.online = true;
        moqui.setOffline(false);
    }
});

test("a package's files are read for the account they came from: another account's same URL is read again", async () => {
    const { vm, done } = mount();
    try {
        await until(() => vm.loaded, "the list");
        vm.open({ packageId: "P7", name: "Harbor", revision: 3 });
        await until(() => vm.mapConfig, "the package opened");
        await vm.member("postal.json");
        const before = asked.filter((one) => one.path === "postal.json").length;
        await vm.member("postal.json");
        assert.equal(asked.filter((one) => one.path === "postal.json").length, before, "kept for the account and revision");
        moqui.storageScope = "a2";
        assert.equal(await vm.member("postal.json"), null, "not another account's");
    } finally {
        moqui.storageScope = "a1";
        done();
    }
});

test("the shell's Maps entry opens Browse when the server answers and the offline screen when it does not", () => {
    const methods = moqui.bundledComponent("Shell/Root").methods;
    const shown = [];
    const shell = (isOffline, mapsApp) => ({ isOffline, mapsApp, mapsOn: true, setUrl: (url) => shown.push(url), failed: (err) => { throw err; } });
    methods.openMaps.call(shell(false, { path: "/qapps/maps" }));
    assert.deepEqual(calls.filter((call) => call[0] === "openRemote").pop(), ["openRemote", "/qapps/maps/Browse"]);
    methods.openMaps.call(shell(true, { path: "/qapps/maps" }));
    methods.openMaps.call(shell(false, null));
    assert.deepEqual(shown, ["/qapps/maps/Offline", "/qapps/maps/Offline"], "offline, or a server with no Maps app");
    methods.openApp.call(shell(true, null), { name: "maps", path: "/qapps/maps" });
    assert.equal(shown.pop(), "/qapps/maps/Offline", "the Maps app offline is the offline screen");
    methods.openApp.call(shell(true, null), { name: "boss", path: "/qapps/boss" });
    assert.equal(shown.pop(), "/qapps/boss", "another app is its page");
    // the route of the offline screen is the shell's own
    const route = methods.route.call({ commsRel: "BasicComms", componentOf: (view) => (view === "maps" ? {} : null) }, "/qapps/maps/Offline");
    assert.deepEqual([route.view, route.pathList], ["maps", ["maps", "Offline"]]);
});
})();
