// Maps › Browse opened with an extent (a Dashboards geomap's country: docs/tools/Maps.md, From a dashboard) opens on it
// at the first load. Its check is Browse.qvue's own: moqui.map (WebrootMap.js) loads only as m-map mounts, after the
// first view is set, so a check there left the extent unread on a fresh page. The view is read as soon as Browse has
// its configuration, before a later load could set it.
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

feature("Browse opened with an extent across the antimeridian opens on it at the first load, and its check refuses what is not one", {
    expect() {
        const page = browser.newPage({user: "john.doe"})
        page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
        page.goto("/qapps/maps/Browse?extent=" + encodeURIComponent(RUSSIA)).status == 200
        // the view as Browse first has its configuration (effective), the map then drawn from it
        const first = page.waitFor(`(() => { const vm = (${browse})(); return vm && vm.effective ? { view: JSON.stringify(vm.view || null) } : null })()`,
            {timeout: 10})
        const checked = page.evaluate(`(() => {
            const vm = (${browse})()
            const bad = ["", null, "1,2,3", "1,2,3,4,5", "a,2,3,4", "1,,3,4", "-181,0,10,10", "0,0,181,10", "0,-91,10,10", "0,0,10,91", "0,20,10,10"]
            return { france: JSON.stringify(vm.extentParameter("-4.5918,42.3433,8.0983,51.148")), refused: bad.filter((text) => vm.extentParameter(text) !== null) }
        })()`)
        first.view == JSON.stringify({extent: [27.2865, 41.1518, -169.8983, 77.6975]})
        checked.france == JSON.stringify([-4.5918, 42.3433, 8.0983, 51.148])
        checked.refused.length == 0
    }
})
