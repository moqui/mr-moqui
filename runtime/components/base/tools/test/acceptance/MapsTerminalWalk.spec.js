// The Maps terminal walk (plans/Maps.md §3.2, §9): each Maps screen in the tui render mode, as the terminal client asks it, with
// authorization on, on the walks' fixture (MapsWalkFixtures): Browse's map component and its terminalMap (the saved place, the
// record layer's features, the cut said, the attribution); LocalMap's map with the record's place and Choose location, whose
// typed latitude and longitude Save location posts through the adapter; the record a user may not read not available; Saved
// views, Preferences and, for the administrator, Configuration.
// Run: ./moq accept tools --spec MapsTerminalWalk
spec({dependsOn: ["MapsWalkFixtures"]})

const located = JSON.stringify({recordId: "LOCATED"})
function as(username, path, parameters, method, mode) {
    ec.user.internalLoginUser(username)
    try {
        let test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        if (mode) test = test.renderMode(mode)
        const res = test.render(path, parameters || {}, method || null)
        return {status: res.status, errors: res.errorMessages.join(" | "), output: String(res.output || "")}
    } finally {
        ec.message.clearErrors()
        ec.user.logoutUser()
    }
}
const parsed = res => {
    try {
        return JSON.parse(res.output)
    } catch (e) {
        return {}
    }
}
// the nodes of a tui document, flattened
const nodes = doc => {
    const out = []
    const walk = node => {
        if (!node || typeof node !== "object") return
        if (Array.isArray(node)) return node.forEach(walk)
        out.push(node)
        Object.values(node).forEach(walk)
    }
    walk(doc)
    return out
}

feature("Browse in the terminal: the map component, its places, the cut said and the attribution", {
    when() {
        return {
            screen: as("maps.walk", "apps/maps/Browse", {}, null, "tui"),
            data: as("maps.walk", "apps/maps/Browse/terminalMap", {}, "get")
        }
    },
    then({screen, data}) {
        screen.status == 200
        nodes(parsed(screen)).some(node => node.t === "component" && node.name === "map" && node.props?.source === "terminalMap")
        const map = parsed(data)
        map.points.some(point => point.label === "Walk harbor")
        map.points.some(point => point.label === "Feature 0")
        map.notes.some(note => note.includes("Only some of the records are shown"))
        map.attribution == "Natural Earth"
    }
})

feature("LocalMap in the terminal: the record's place and Choose location, saved through the adapter from typed numbers", {
    when() {
        ec.cache.getCache("moqui.test.maps.stored").remove("LOCATED")
        const record = {adapterKey: "MAPS_WALK_ADAPTER", recordKey: located}
        return {
            screen: as("maps.walk", "apps/maps/LocalMap", record, null, "tui"),
            saved: as(
                "maps.walk",
                "apps/maps/LocalMap/storeLocation",
                Object.assign({recordVersion: "1", latitude: "41.95", longitude: "-87.65"}, record),
                "post"
            ),
            stored: ec.cache.getCache("moqui.test.maps.stored").get("LOCATED"),
            denied: as(
                "maps.walk.plain",
                "apps/maps/LocalMap",
                {adapterKey: "MAPS_WALK_ADAPTER", recordKey: JSON.stringify({recordId: "DENIED"})},
                null,
                "tui"
            )
        }
    },
    then({screen, saved, stored, denied}) {
        screen.status == 200
        screen.errors == ""
        const all = nodes(parsed(screen))
        all.some(
            node =>
                node.t === "component" &&
                node.name === "map" &&
                (node.props?.points || []).some(point => point.label === "Spec place")
        )
        all.some(node => node.t === "field" && node.name === "latitude")
        screen.output.includes("Save location")
        screen.output.includes("Open in Maps")
        saved.errors == ""
        JSON.parse(stored).point.latitude == 41.95
        denied.status == 200
        denied.output.includes("This record's place is not available")
        !denied.output.includes("Spec place")
        !denied.output.includes("Open in Maps")
    }
})

feature("Saved views, Preferences and Configuration in the terminal", {
    when() {
        return {
            views: as("maps.walk", "apps/maps/Views", {}, null, "tui"),
            preferences: as("maps.walk", "apps/maps/Preferences", {}, null, "tui"),
            configuration: as("john.doe", "apps/maps/Configuration", {}, null, "tui"),
            refused: as("maps.walk", "apps/maps/Configuration", {}, null, "tui")
        }
    },
    then({views, preferences, configuration, refused}) {
        views.status == 200
        views.output.includes("Walk harbor")
        preferences.status == 200
        preferences.output.includes("Restore my last view")
        preferences.output.includes("Walk records")
        configuration.status == 200
        configuration.output.includes("Map profiles")
        configuration.output.includes("OpenFreeMap")
        configuration.output.includes("Walk records")
        refused.status == 403 || /not authorized/i.test(refused.errors)
    }
})
