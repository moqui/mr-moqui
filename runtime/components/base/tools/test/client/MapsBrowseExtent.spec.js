// Maps › Browse in qvue, its one render in the regression suite (MapsScreens renders its terminal twin): Browse.qvue is
// the MapsBrowse component and draws its m-map, and opened with an extent (a Dashboards geomap's country:
// docs/tools/Maps.md, From a dashboard) it opens on it at the first load. Its check is Browse.qvue's own: moqui.map
// (WebrootMap.js) loads only as m-map mounts, after the first view is set, so a check there left the extent unread on a
// fresh page. The view is read once the page settles and Browse has its configuration. On the same render, live location
// (plans/Location-Sharing.md §3.3): a share started from Browse, sent from the page, drawn on the live layer and stopped.
const RUSSIA = "27.2865,41.1518,-169.8983,77.6975"

/** The MapsBrowse component of the page, found in the shell's tree of components. */
function browse() {
    const find = (vnode) => {
        if (!vnode) return null
        const component = vnode.component
        if (component && component.type && component.type.name === "MapsBrowse") return component.proxy
        if (component) return find(component.subTree)
        return Array.isArray(vnode.children) ? vnode.children.map(find).find(Boolean) || null : null
    }
    return window.moqui && moqui.webrootVue ? find(moqui.webrootVue.$.subTree) : null
}

feature("Browse is the MapsBrowse component with its map, opened on an extent across the antimeridian at the first load, and its check refuses what is not one", {
    expect() {
        const page = browser.newPage({user: "john.doe"})
        page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
        page.goto("/qapps/maps/Browse?extent=" + encodeURIComponent(RUSSIA)).status == 200
        // once the page settles: the MapsBrowse component (Browse.qvue's name) with its configuration, its view and its map
        // element (the view as JSON: a reactive object does not come out of the page)
        const first = page.waitFor(`(() => { const vm = (${browse})()
            const map = document.getElementById("MapsBrowseMap")
            return vm && vm.effective && map ? { view: JSON.stringify(vm.view || null), map: true } : null })()`,
            {timeout: 10})
        const checked = page.evaluate(`(() => {
            const vm = (${browse})()
            const bad = ["", null, "1,2,3", "1,2,3,4,5", "a,2,3,4", "1,,3,4", "-181,0,10,10", "0,0,181,10", "0,-91,10,10", "0,0,10,91", "0,20,10,10"]
            return { france: JSON.stringify(vm.extentParameter("-4.5918,42.3433,8.0983,51.148")),
                spaced: JSON.stringify(vm.extentParameter("27.2865, 41.1518, -169.8983, 77.6975")),
                refused: bad.filter((text) => vm.extentParameter(text) !== null) }
        })()`)
        first.view == JSON.stringify({extent: [27.2865, 41.1518, -169.8983, 77.6975]})
        first.map == true
        checked.france == JSON.stringify([-4.5918, 42.3433, 8.0983, 51.148])
        checked.spaced == JSON.stringify([27.2865, 41.1518, -169.8983, 77.6975])
        checked.refused.length == 0

        // Live location on the same render (plans/Location-Sharing.md §3.3): the configuration carries the user's shares;
        // Share my location with a guest link starts one, sent from this page (the browser's position stood in for, as
        // watchPosition gives it) through reportPosition, its banner shown and its latest drawn on m-map's live layer as the
        // audience reads it; Stop ends it and the page stops sending
        let started, drawn, shown, panel, stopped
        try {
            started = page.evaluate(`(async () => {
                const vm = (${browse})()
                const fix = (ok) => setTimeout(() => ok({ coords: { latitude: 45.52, longitude: -122.68, accuracy: 12, heading: null, speed: null }, timestamp: Date.now() }), 0)
                Object.defineProperty(navigator, "geolocation", { configurable: true, value: {
                    watchPosition(ok) { fix(ok); return 7 }, getCurrentPosition(ok) { fix(ok) }, clearWatch() {} } })
                const configured = Array.isArray(vm.effective.locationShares)
                vm.openShare()
                vm.shareForm.link = true
                vm.shareForm.label = "Spec walk"
                const shareId = await vm.createShare()
                return { configured, shareId, token: vm.shareMade ? String(vm.shareMade.linkToken || "").length : 0,
                    geo: vm.geoUri({ label: "Sam & Co", position: { lat: 45.5, lon: -122.6 } }) }
            })()`)
            // the m-map-view component of the page (WebrootMap.js), whose live layer holds the shares
            const mapView = `(() => { const find = (vnode) => { if (!vnode) return null; const c = vnode.component
                if (c && c.type && c.type.name === "mMapView") return c.proxy
                if (c) return find(c.subTree)
                return Array.isArray(vnode.children) ? vnode.children.map(find).find(Boolean) || null : null }
                return find(moqui.webrootVue.$.subTree) })()`
            drawn = page.waitFor(`(() => { const view = ${mapView}
                const share = view && view.liveShares.find((one) => one.shareId === ${JSON.stringify(started.shareId)})
                const banner = document.querySelector('[data-maps-sharing="' + ${JSON.stringify(started.shareId)} + '"]')
                return share && share.latest && banner ? { lat: share.latest.lat, role: share.role, label: share.label, banner: banner.textContent,
                    feature: !!(view.liveSource && view.liveSource.getFeatureById("live:" + share.shareId)) } : null })()`, {timeout: 10})
            // the dialog shows the guest link's token this once, until Done
            shown = page.locator("#MapsShareLinkToken").text()
            page.getByRole("button", {name: "Done"}).click()
            page.locator(".q-dialog__backdrop").waitFor({state: "detached", timeout: 5})
            // the Live location panel: Who sees my location lists it with its audience and its acts; nobody shares with this user
            page.click("#MapsLiveButton")
            panel = page.waitFor(`(() => { const card = document.querySelector('[data-maps-mine="' + ${JSON.stringify(started.shareId)} + '"]')
                return card ? { card: card.textContent, none: !!document.getElementById("MapsSharedNone") } : null })()`, {timeout: 5})
            page.click(`[data-maps-stop="${started.shareId}"]`)
            stopped = page.waitFor(`(() => { const vm = (${browse})()
                return !document.querySelector('[data-maps-sharing="' + ${JSON.stringify(started.shareId)} + '"]') && !vm.sending.length
                    ? { mine: vm.myShares.length, watching: vm.watchId === null } : null })()`, {timeout: 10})
        } finally {
            // a run that failed on the way leaves no active share: the one the page started is stopped (again, harmlessly)
            page.evaluate(`(async () => { const vm = (${browse})()
                const id = vm && vm.shareMade && vm.shareMade.shareId
                if (id) await vm.request("stopShare", { method: "POST", params: { shareId: id } }).catch(() => null)
                return true })()`)
        }
        started.configured == true
        started.shareId != null
        started.token == 32
        shown.includes("shown this once")
        started.geo == "geo:45.500000,-122.600000?q=45.500000,-122.600000(Sam%20%26%20Co)"
        drawn.lat == 45.52
        drawn.role == "owner"
        drawn.label == "Spec walk"
        drawn.banner.includes("guest link")
        drawn.feature == true
        panel.card.includes("guest link")
        panel.card.includes("Extend")
        panel.card.includes("Stop")
        panel.none == true
        stopped.watching == true
    }
})
