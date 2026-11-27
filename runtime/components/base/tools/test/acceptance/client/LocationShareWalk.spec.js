/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// LocationShareWalk (plans/Location-Sharing.md LL7; docs/tools/Maps.md, Live location): live location in three browsers
// of three people (LocationShareWalkFixtures: loc.walk.ann, loc.walk.ben and loc.walk.cy, all in Walk family). Ann starts
// a share with Walk family from Browse's Share my location, sent from her page (the browser's position stood in for, as
// watchPosition gives it), and the app bar's chip says so; Ben sees it in Shared with me, and its marker moves on his map
// by the share's pushes, with no read of the shares between; Ann extends it, and Ben's end moves by push too; an
// administrator ends Cy's membership of the group (System › Access, the user's groups), and Cy gets no more positions and
// finds nothing at his next read; Ann stops it, her chip goes, and Ben sees it end with no read. The phone's half (the
// app's confirmation dialog, its foreground notification and that notification's Stop) is
// scripts/android-location-share.sh, on this spec's fixture.
// Run: ./moq accept tools --spec LocationShareWalk
const PASSWORD = "LocationWalk1!!"
const GROUP = "LOC_WALK_FAMILY"
// LocationShareWalkFixtures' rows of the group: this fixed fromDate
const SINCE = Date.UTC(2026, 0, 1)
const HOME = {lat: 45.52, lon: -122.68}
const MOVED = {lat: 45.5312, lon: -122.6655}
const AGAIN = {lat: 45.5405, lon: -122.6521}
const state = {}

/** The MapsBrowse component of the page, found in the shell's tree of components. */
const BROWSE = `(() => { const find = (vnode) => { if (!vnode) return null; const c = vnode.component
    if (c && c.type && c.type.name === "MapsBrowse") return c.proxy
    if (c) return find(c.subTree)
    return Array.isArray(vnode.children) ? vnode.children.map(find).find(Boolean) || null : null }
    return window.moqui && moqui.webrootVue ? find(moqui.webrootVue.$.subTree) : null })()`
/** The m-map-view component of the page (WebrootMap.js), whose live layer holds the shares. */
const MAP_VIEW = `(() => { const find = (vnode) => { if (!vnode) return null; const c = vnode.component
    if (c && c.type && c.type.name === "mMapView") return c.proxy
    if (c) return find(c.subTree)
    return Array.isArray(vnode.children) ? vnode.children.map(find).find(Boolean) || null : null }
    return window.moqui && moqui.webrootVue ? find(moqui.webrootVue.$.subTree) : null })()`
// what the page's live layer holds of a share: its latest position, its end, whether its marker is on the map; null
// when the layer does not hold it
const liveOf = shareId => `(() => { const view = ${MAP_VIEW}
    const share = view && view.liveShares.find((one) => one.shareId === ${JSON.stringify(shareId)})
    return share ? { lat: share.latest ? share.latest.lat : null, lon: share.latest ? share.latest.lon : null, thruDate: share.thruDate,
        role: share.role, label: share.label, marker: !!(view.liveSource && view.liveSource.getFeatureById("live:" + share.shareId)) } : null })()`

function mapsPage(username) {
    const page = browser.newPage({user: username, password: PASSWORD})
    // the shell asks for the user's photo and their calls, which a user without Comms is refused
    page.allowProblems(/40[34] [A-Za-z ]+ .*\/(userPhoto\?userId=\w+|activeCalls)$/)
    return page
}
// Browse opened with its configuration read and its Live location panel shown
function openBrowse(page) {
    const status = page.goto("/qapps/maps/Browse").status
    if (status !== 200) throw new Error("Browse answered " + status)
    page.waitFor(`(() => { const vm = ${BROWSE}; return !!(vm && vm.effective) })()`, {timeout: 60})
    page.click("#MapsLiveButton")
    page.locator("#MapsShareLocation").waitFor({timeout: 10})
}
// the reads of the shares the page makes from now on (Browse's locationShares transition), counted in the page
function countReads(page) {
    page.evaluate(`(() => { const vm = ${BROWSE}
        window.__walkReads = 0
        if (window.__walkCounting === vm) return true
        window.__walkCounting = vm
        const request = vm.request
        vm.request = function (name, options) { if (name === "locationShares") window.__walkReads++; return request.call(vm, name, options) }
        return true })()`)
}
const reads = page => page.evaluate("window.__walkReads")
// the stand-in position, moved: the page's watch gets it on its next fix (one a second; the page reports one a share's
// interval)
const moveTo = (page, at) => page.evaluate(`(window.__walkFix = ${JSON.stringify(at)}, true)`)

spec({
    timeout: 180,
    stepwise: true,
    setupSpec() {
        state.ann = mapsPage("loc.walk.ann")
        state.ben = mapsPage("loc.walk.ben")
        state.cy = mapsPage("loc.walk.cy")
    },
    cleanupSpec() {
        // a run that failed on the way leaves no active share: the one Ann started is stopped (again, harmlessly)
        try {
            if (state.ann && state.shareId)
                state.ann.evaluate(`(async () => { const vm = ${BROWSE}
                    if (vm) await vm.request("stopShare", { method: "POST", params: { shareId: ${JSON.stringify(state.shareId)} } }).catch(() => null)
                    return true })()`)
        } catch (err) {
            log.warn("LocationShareWalk: the share was not stopped at the end: " + (err && err.message ? err.message : err))
        }
    }
})

feature("Ann shares her location with Walk family from Browse's Share my location, sent from her page, and her chip says so", {
    expect() {
        const ann = state.ann
        openBrowse(ann)
        // the browser's position, as watchPosition gives it: a fix a second at window.__walkFix
        ann.evaluate(`(() => {
            window.__walkFix = ${JSON.stringify(HOME)}
            const watchers = []
            const fix = (ok) => ok({ coords: { latitude: window.__walkFix.lat, longitude: window.__walkFix.lon, accuracy: 10, heading: null, speed: null },
                timestamp: Date.now() })
            Object.defineProperty(navigator, "geolocation", { configurable: true, value: {
                watchPosition(ok) { watchers.push(ok); setTimeout(() => fix(ok), 0); return watchers.length },
                getCurrentPosition(ok) { setTimeout(() => fix(ok), 0) },
                clearWatch(id) { watchers[id - 1] = null } } })
            setInterval(() => watchers.forEach((ok) => ok && fix(ok)), 1000)
            return true })()`)
        ann.click("#MapsShareLocation")
        ann.locator("#MapsShareDialog").waitFor({timeout: 10})
        // the group, chosen in Groups you are in; one hour; exact; no trail
        ann.waitFor(`(() => { const vm = ${BROWSE}; return !!(vm && vm.shareTargets.loaded) })()`, {timeout: 10})
        // the group chosen as its select's option sets it (the dialog's form), among the groups the dialog offers her
        const offered = ann.evaluate(`(() => { const vm = ${BROWSE}
            const group = vm.shareTargets.groups.find((one) => one.userGroupId === ${JSON.stringify(GROUP)})
            if (group) vm.shareForm.groups = [group.userGroupId]
            return group ? group.label : null })()`)
        offered == "Walk family"
        ann.locator("#MapsShareDialog").getByText("A group is seen by its members now").waitFor({timeout: 5})
        ann.click("#MapsShareCreate")
        // no link: the dialog closes once the share is made, and the page sends it
        const made = ann.waitFor(`(() => { const vm = ${BROWSE}
            return vm && vm.shareMade && vm.shareMade.shareId && !vm.shareShown ? { shareId: vm.shareMade.shareId, sending: vm.sending.slice() } : null })()`,
            {timeout: 20})
        state.shareId = made.shareId
        made.sending.includes(made.shareId)
        const banner = ann.waitFor(`(() => { const el = document.querySelector('[data-maps-sharing="' + ${JSON.stringify(made.shareId)} + '"]')
            return el ? el.textContent : null })()`, {timeout: 10})
        banner.includes("Walk family")
        // her own position on her own map, as the server took it
        const own = ann.waitFor(`(() => { const share = ${liveOf(made.shareId)}; return share && share.lat ? share : null })()`, {timeout: 20})
        own.role == "owner"
        own.lat == HOME.lat
        // Who sees my location: the share, its audience (the group) and its acts
        const card = ann.waitFor(`(() => { const el = document.querySelector('[data-maps-mine="' + ${JSON.stringify(made.shareId)} + '"]')
            return el ? el.textContent : null })()`, {timeout: 10})
        card.includes("Walk family") && card.includes("Extend") && card.includes("Stop")
        // the app bar's chip on every screen: Sharing location, sending here
        const chip = ann.waitFor(`(() => { const el = document.getElementById("m-indicator-locationSharing")
            return el && /sending here/.test(el.textContent) ? el.textContent : null })()`, {timeout: 15})
        chip.includes("Sharing location")
    }
})

feature("Ben sees it in Shared with me, and its marker moves on his map by push, with no read of the shares", {
    expect() {
        const ben = state.ben
        openBrowse(ben)
        const row = ben.waitFor(`(() => { const el = document.querySelector('[data-maps-shared="' + ${JSON.stringify(state.shareId)} + '"]')
            return el ? el.textContent : null })()`, {timeout: 20})
        row.includes("Ann Walker") && row.includes("until")
        const seen = ben.waitFor(`(() => { const share = ${liveOf(state.shareId)}; return share && share.lat ? share : null })()`, {timeout: 20})
        seen.role == "viewer"
        seen.lat == HOME.lat
        seen.marker == true
        state.firstEnd = seen.thruDate
        // Ann moves: Ben's marker follows by the share's push, his page reading nothing meanwhile
        countReads(ben)
        moveTo(state.ann, MOVED)
        const moved = ben.waitFor(`(() => { const share = ${liveOf(state.shareId)}; return share && share.lat === ${MOVED.lat} ? share : null })()`,
            {timeout: 30})
        moved.lon == MOVED.lon
        moved.marker == true
        reads(ben) == 0
    }
})

feature("Ann extends it to two hours, and Ben's end moves by push", {
    expect() {
        const ann = state.ann
        const ben = state.ben
        countReads(ben)
        ann.locator(`[data-maps-mine="${state.shareId}"] [data-maps-extend]`).click()
        ann.locator(".q-menu").getByText("2 h from now").click()
        const extended = ben.waitFor(`(() => { const share = ${liveOf(state.shareId)}
            return share && share.thruDate > ${state.firstEnd} + 30 * 60000 ? share : null })()`, {timeout: 20})
        extended.thruDate > state.firstEnd
        reads(ben) == 0
        // Ann's own page shows the new end too
        ann.waitFor(`(() => { const vm = ${BROWSE}
            const share = vm && vm.myShares.find((one) => one.shareId === ${JSON.stringify(state.shareId)})
            return share && share.thruDate > ${state.firstEnd} + 30 * 60000 })()`, {timeout: 20})
    }
})

feature("Cy, taken out of Walk family by an administrator, gets no more positions and finds nothing at his next read", {
    expect() {
        const cy = state.cy
        openBrowse(cy)
        const seen = cy.waitFor(`(() => { const share = ${liveOf(state.shareId)}; return share && share.lat ? share : null })()`, {timeout: 20})
        seen.lat == MOVED.lat
        // System › Access, the user's groups: Cy's membership of the group ended now, as its form posts it
        const admin = session({user: "john.doe"})
        const ended = admin.post("/apps/system/Security/UserAccount/UserAccountDetail/updateUserGroupMember", {
            form: {userId: "LOC_WALK_CY", userGroupId: GROUP, fromDate: String(SINCE), thruDate: String(Date.now() - 1000)},
            accept: "json"
        })
        ended.status == 200
        !ended.json().errors
        // the server's read, as Cy's next one: the share is not his any more
        const cyRead = session({user: "loc.walk.cy", password: PASSWORD}).get("/apps/maps/Browse/locationShares", {accept: "json"})
        const cyShares = (cyRead.json().locationShares || []).map(one => one.shareId)
        cyShares.includes(state.shareId) == false
        // Ann moves again: Ben gets it, Cy does not
        moveTo(state.ann, AGAIN)
        state.ben.waitFor(`(() => { const share = ${liveOf(state.shareId)}; return share && share.lat === ${AGAIN.lat} })()`, {timeout: 30})
        sleep(3000)
        const held = cy.evaluate(liveOf(state.shareId))
        held == null || held.lat == MOVED.lat
        // his next read: nothing shared with him
        openBrowse(cy)
        cy.waitFor(() => !!document.getElementById("MapsSharedNone"), {timeout: 20})
        cy.evaluate(`!document.querySelector('[data-maps-shared="' + ${JSON.stringify(state.shareId)} + '"]')`) == true
        cy.evaluate(liveOf(state.shareId)) == null
    }
})

feature("Ann stops it: her chip goes, and Ben sees it end with no read", {
    expect() {
        const ann = state.ann
        const ben = state.ben
        countReads(ben)
        ann.locator(`[data-maps-mine="${state.shareId}"]`).getByRole("button", {name: "Stop", exact: true}).click()
        ann.waitFor(`(() => { const vm = ${BROWSE}
            return !document.querySelector('[data-maps-sharing="' + ${JSON.stringify(state.shareId)} + '"]') && !vm.sending.length
                && !document.getElementById("m-indicator-locationSharing") })()`, {timeout: 20})
        ben.waitFor(`(() => !${liveOf(state.shareId)} && !document.querySelector('[data-maps-shared="' + ${JSON.stringify(state.shareId)} + '"]')
            && !!document.getElementById("MapsSharedNone"))()`, {timeout: 20})
        reads(ben) == 0
        state.shareId = null
    }
})
