// The Maps app's screens (plans/archive/Maps.md §3.1, §3.2; docs/tools/Maps.md): Browse's terminal template (Browse.tui.json over
// terminalMap; Browse.qvue's one render is in a browser, test/client/MapsBrowseExtent) for a Maps user, with its live
// location list (LocationSharesTui, plans/Location-Sharing.md §3.3: the user's own share and one shared with them, ages and
// coordinates, Stop for their own only), and the app refused to a user with ALL_USERS alone; the local-map dialog,
// a plain XML screen rendered once in qvt and once in html: for a Maps user the adapter lets write, the record's place,
// precision and source from the adapter's answer, Choose location and Open in Maps; for a user with ALL_USERS alone (no
// MAPS_APP, no System), who opens it, a record the adapter refuses them (DENIED) not available, with nothing of it, no Save
// location and no Open in Maps (the terminal's form is MapsTerminalWalk's); storeLocation refused for a user the adapter
// does not let write and saved through the adapter's store#RecordLocation for one it does; Configuration refused to a Maps
// user, its settings index entry, and its profile rules (the default enabled and drawable, the default not disabled, a
// change only with Configuration's update). The adapter is the framework's test one (framework
// test/service/org/moqui/mapstest), registered and granted here as a component would.
// Run: ./moq test tools --spec MapsScreens
const ADAPTER = "MAPS_SCR_ADAPTER"
const GRANT = "MAPS_SCR_GRANT"
const WRITERS = "MAPS_SCR_WRITERS"
const PLAIN = "MAPS_SCR_PLAIN"
const WRITER = "MAPS_SCR_WRITER"
const VIEWER = "MAPS_SCR_VIEWER"
const located = JSON.stringify({recordId: "LOCATED"})
const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
const make = (entityName, fields) => ec.entity.makeValue(entityName).setAll(fields).createOrUpdate()
/* A render or a transition under /apps as the user, authorization on: its status, errors and output. */
function as(userId, path, parameters, method, mode) {
    ec.user.internalLoginUser(userId ? usernameOf(userId) : "john.doe")
    try {
        let test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        if (mode) test = test.renderMode(mode)
        if (mode === "qvt") test = test.withStaticData()
        // a template mode renders the last screen alone, as the shell asks it
        const given = mode === "qvt" || mode === "qvue" ? Object.assign({lastStandalone: "true"}, parameters) : parameters || {}
        const res = test.render(path, given, method || null)
        return {status: res.status, errors: res.errorMessages.join(" | "), output: String(res.output || "")}
    } finally {
        ec.message.clearErrors()
        ec.user.logoutUser()
    }
}
const refused = res => res.status == 403 || /not authorized/i.test(res.errors)
const json = res => {
    try {
        return JSON.parse(res.output)
    } catch (e) {
        return {}
    }
}
const stored = () => ec.cache.getCache("moqui.test.maps.stored").get("LOCATED")
const noRenderError = res => !/Error rendering|Exception in render/.test(res.output)

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const userId of [PLAIN, WRITER, VIEWER])
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps screens " + userId})
        // the writer: a Maps user the test adapter lets write (MAPS_SPEC_STORE)
        make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId: WRITER, fromDate: ec.user.nowTimestamp})
        make("moqui.security.UserPermission", {
            userPermissionId: "MAPS_SPEC_STORE",
            description: "Maps spec: store a record's location"
        })
        make("moqui.security.UserGroup", {userGroupId: WRITERS, description: "Maps screens spec writers"})
        make("moqui.security.UserGroupPermission", {
            userGroupId: WRITERS,
            userPermissionId: "MAPS_SPEC_STORE",
            fromDate: ec.user.nowTimestamp
        })
        make("moqui.security.UserGroupMember", {userGroupId: WRITERS, userId: WRITER, fromDate: ec.user.nowTimestamp})
        // the viewer: Configuration's view, not its update (an ArtifactAuthz of VIEW on MAPS_ADMIN)
        make("moqui.security.UserGroup", {
            userGroupId: "MAPS_SCR_VIEWERS",
            description: "Maps screens spec configuration viewers"
        })
        make("moqui.security.UserGroupMember", {userGroupId: "MAPS_SCR_VIEWERS", userId: VIEWER, fromDate: ec.user.nowTimestamp})
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: "MAPS_SCR_VIEW_ONLY",
            userGroupId: "MAPS_SCR_VIEWERS",
            artifactGroupId: "MAPS_ADMIN",
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_VIEW"
        })
        make("moqui.maps.MapAdapter", {
            adapterKey: ADAPTER,
            description: "Spec places",
            sequenceNum: 1,
            contextServiceName: "org.moqui.mapstest.MapsTestServices.get#RecordMapContext",
            storeServiceName: "org.moqui.mapstest.MapsTestServices.store#RecordLocation"
        })
        // the adapter's own grant, from its component, to every user who opens the dialog
        make("moqui.security.ArtifactGroup", {artifactGroupId: GRANT, description: "Maps screens spec adapter"})
        make("moqui.security.ArtifactGroupMember", {
            artifactGroupId: GRANT,
            artifactName: "org\\.moqui\\.mapstest\\.MapsTestServices\\.(get#RecordMapContext|store#RecordLocation)",
            artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y",
            inheritAuthz: "N"
        })
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: GRANT,
            userGroupId: "ALL_USERS",
            artifactGroupId: GRANT,
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        // where the adapter's links go, for every app's maps: the Maps app's own screen (a Maps user holds it)
        make("moqui.maps.MapRoute", {
            mapRouteId: "MAPS_SCR_ROUTE",
            routeKey: "specRecord",
            priority: 1,
            urlTemplate: "/apps/maps/Browse?recordId={recordId}",
            screenLocation: "component://tools/screen/Maps.xml"
        })
        ec.cache.getCache("moqui.test.maps.stored").clear()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity
            .find("moqui.security.ArtifactAuthz")
            .condition("artifactAuthzId", "in", [GRANT, "MAPS_SCR_VIEW_ONLY"])
            .deleteAll()
        ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", GRANT).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", GRANT).deleteAll()
        ec.entity.find("moqui.maps.MapAdapter").condition("adapterKey", ADAPTER).deleteAll()
        ec.entity.find("moqui.maps.MapRoute").condition("mapRouteId", "MAPS_SCR_ROUTE").deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

/* A LocationServices call as the user (plans/Location-Sharing.md §3.2): its results. */
function shareCall(userId, verbNoun, parameters) {
    ec.user.internalLoginUser(usernameOf(userId))
    try {
        return (
            ec.service
                .sync()
                .name("org.moqui.maps.LocationServices." + verbNoun)
                .parameters(parameters || {})
                .call() || {}
        )
    } finally {
        ec.message.clearErrors()
        ec.user.logoutUser()
    }
}

feature("Browse renders its terminal twin for a Maps user, and the app is refused to a user with ALL_USERS alone", {
    when() {
        // live location (plans/Location-Sharing.md §3.3): the writer's own share and one shared with them, each with a
        // position, for the twin's list under the map (LocationSharesTui) and its Stop
        const thruDate = new Date(Date.now() + 3600000)
        const own = shareCall(WRITER, "create#Share", {audienceList: [{audienceTypeEnumId: "LsaUser", userId: PLAIN}], thruDate}).shareId
        shareCall(WRITER, "report#Position", {shareId: own, latitude: "51.5", longitude: "-0.12"})
        const theirs = shareCall(PLAIN, "create#Share", {
            audienceList: [{audienceTypeEnumId: "LsaUser", userId: WRITER}],
            thruDate,
            label: "Plain walk"
        }).shareId
        shareCall(PLAIN, "report#Position", {shareId: theirs, latitude: "48.85", longitude: "2.35"})
        const shares = as(WRITER, "apps/maps/LocationSharesTui", {}, null, "tui")
        as(WRITER, "apps/maps/LocationSharesTui/stopShare", {shareId: theirs}, "post")
        as(WRITER, "apps/maps/LocationSharesTui/stopShare", {shareId: own}, "post")
        const live = {
            own,
            theirs,
            shares,
            ownActive: Array.from(shareCall(WRITER, "find#MyShares").shareList || []).some(row => row.shareId === own),
            theirsActive: Array.from(shareCall(WRITER, "find#SharedWithMe").shareList || []).some(row => row.shareId === theirs)
        }
        shareCall(PLAIN, "stop#Share", {shareId: theirs})
        return {
            live,
            tui: as(WRITER, "apps/maps/Browse", {}, null, "tui"),
            terminal: as(
                WRITER,
                "apps/maps/Browse/terminalMap",
                {adapterKey: ADAPTER, recordKey: located, postal: "US 00000"},
                "get"
            ),
            // the view a dashboard geomap opens: a point, marked first; an area around the record's place (Chicago); and
            // Russia's, across the antimeridian, which leaves it out
            point: as(WRITER, "apps/maps/Browse/terminalMap", {longitude: "2.35", latitude: "48.85", label: "Paris"}, "get"),
            around: as(WRITER, "apps/maps/Browse/terminalMap", {adapterKey: ADAPTER, recordKey: located, extent: "-90,40,-85,43"}, "get"),
            russia: as(WRITER, "apps/maps/Browse/terminalMap",
                {adapterKey: ADAPTER, recordKey: located, extent: "27.2865,41.1518,-169.8983,77.6975"}, "get"),
            plainConfig: as(PLAIN, "apps/maps/Browse/getConfig", {}, "get")
        }
    },
    then({live, tui, terminal, point, around, russia, plainConfig}) {
        // Browse.tui.json: the map component over terminalMap, whose places carry their precision as words, and the live
        // location list under it
        tui.status == 200
        tui.errors == ""
        tui.output.includes('"name":"map"')
        tui.output.includes("terminalMap")
        tui.output.includes('"t":"dynamicContainer"')
        tui.output.includes("LocationSharesTui")
        // the list: the writer's own share with whom and Stop, and the one shared with them, each its age and coordinates;
        // Stop ends the writer's own, and another's share is not theirs to stop
        live.shares.status == 200
        live.shares.errors == ""
        noRenderError(live.shares)
        live.shares.output.includes("You, with Maps screens MAPS_SCR_PLAIN")
        live.shares.output.includes("51.50000, -0.12000")
        live.shares.output.includes("Plain walk")
        live.shares.output.includes("48.85000, 2.35000")
        live.shares.output.includes("just now")
        live.shares.output.includes("stopShare")
        live.shares.output.includes(live.own)
        live.ownActive == false
        live.theirsActive == true
        terminal.status == 200
        json(terminal).attribution == "Natural Earth"
        json(terminal).points.some(
            point => point.label === "Spec place" && point.precision === "Postal-area representative point"
        )
        // its overlays (none yet), and a postal search said with its result: no Geo data has the code here
        Array.isArray(json(terminal).overlays)
        json(terminal).postalSearch.query == "US 00000"
        json(terminal).postalSearch.count == 0
        json(terminal).postalSearch.reason != null
        // the view it is opened on: a point marked first and centered on; an area's places within it, its middle the
        // center, across the antimeridian too (Russia's middle is in Siberia, not the Atlantic)
        json(point).points[0].id == "given"
        json(point).points[0].label == "Paris"
        json(point).center[0] == 2.35
        json(around).points.some(place => place.label === "Spec place")
        json(around).extent[0] == -90
        !json(russia).points.some(place => place.label === "Spec place")
        json(russia).extent[2] == -169.8983
        json(russia).center[0] > 100 && json(russia).center[0] < 120
        // the app is MAPS_APP's
        refused(plainConfig)
        !plainConfig.output.includes("profileId")
    }
})

feature("LocalMap shows the record's place, Choose location and Open in Maps to a Maps user the adapter lets write", {
    when() {
        return {
            qvt: as(WRITER, "apps/maps/LocalMap", {adapterKey: ADAPTER, recordKey: located, dialogId: "SpecDialog"}, null, "qvt")
        }
    },
    then({qvt}) {
        qvt.status == 200
        qvt.errors == ""
        noRenderError(qvt)
        qvt.output.includes("<m-map")
        // the place, its precision and source, in the map's configuration (the adapter's answer, read in the actions)
        qvt.output.includes("Spec place")
        qvt.output.includes("Postal-area representative point")
        qvt.output.includes("Spec data set")
        qvt.output.includes("-87.6298")
        // Choose location: the form a click on the map fills, closing its dialog and reloading the page behind it
        qvt.output.includes("ChooseLocation")
        qvt.output.includes("pickForm")
        qvt.output.includes("Save location")
        qvt.output.includes("SpecDialog")
        qvt.output.includes("_screen")
        qvt.output.includes("Open in Maps")
        // the record's link a route of no app resolves (the one to the record itself left out)
        qvt.output.includes('href="/apps/maps/Browse?recordId=RELATED"')
        qvt.output.includes("Spec related")
        !qvt.output.includes("recordId=LOCATED")
    }
})

feature("a user with ALL_USERS alone opens LocalMap; a record the adapter refuses them is not available, with nothing of it", {
    when() {
        return {html: as(PLAIN, "apps/maps/LocalMap", {adapterKey: ADAPTER, recordKey: JSON.stringify({recordId: "DENIED"})})}
    },
    then({html}) {
        html.status == 200
        html.errors == ""
        noRenderError(html)
        html.output.includes("place is not available")
        !html.output.includes("Spec place")
        !html.output.includes("41.87810")
        // read-only, and no way into an app they do not have
        !html.output.includes("Save location")
        !html.output.includes("Open in Maps")
    }
})

feature("Save location is refused for a user the adapter does not let write, and saved through the adapter for one it does", {
    when() {
        const post = userId =>
            as(
                userId,
                "apps/maps/LocalMap/storeLocation",
                {adapterKey: ADAPTER, recordKey: located, recordVersion: "1", latitude: "41.9", longitude: "-87.6"},
                "post"
            )
        const refusal = post(PLAIN)
        const afterRefusal = stored()
        const saved = post(WRITER)
        const stale = as(
            WRITER,
            "apps/maps/LocalMap/storeLocation",
            {adapterKey: ADAPTER, recordKey: located, recordVersion: "0", latitude: "41.9", longitude: "-87.6"},
            "post"
        )
        const outOfRange = as(
            WRITER,
            "apps/maps/LocalMap/storeLocation",
            {adapterKey: ADAPTER, recordKey: located, recordVersion: "1", latitude: "95", longitude: "-87.6"},
            "post"
        )
        return {refusal, afterRefusal, saved, stale, outOfRange, kept: JSON.parse(stored() || "{}")}
    },
    then({refusal, afterRefusal, saved, stale, outOfRange, kept}) {
        refusal.errors.includes("You may not change this record's location")
        afterRefusal == null
        saved.errors == ""
        kept.point.latitude == 41.9
        kept.point.longitude == -87.6
        kept.recordVersion == "1"
        kept.userId == "MAPS_SCR_WRITER"
        stale.errors.includes("The record changed since it was read")
        outOfRange.errors.includes("Choose a point on the map")
    }
})

feature("Configuration is refused to a Maps user; its settings entry and its profile rules", {
    when() {
        const S = "org.moqui.maps.MapServices.store#ProfileSettings"
        const call = parameters => {
            const out = ec.service.sync().name(S).parameters(parameters).call() || {}
            const errors = ec.message.getErrorsString()
            ec.message.clearErrors()
            return Object.assign({}, out, {errors})
        }
        // as the administrator (john.doe), who may open Configuration
        ec.user.internalLoginUser("john.doe")
        let rules
        try {
            rules = {
                // OpenFreeMap's provider is seeded disabled: the profile is enabled, but it cannot be the default
                enable: call({profileId: "OPENFREEMAP", enabled: "Y"}),
                makeDefault: call({profileId: "OPENFREEMAP", makeDefault: true}),
                disableDefault: call({profileId: "OUTLINE", enabled: "N"}),
                disable: call({profileId: "OPENFREEMAP", enabled: "N"}),
                unknown: call({profileId: "MAPS_SCR_NOT_A_PROFILE", enabled: "Y"})
            }
        } finally {
            ec.user.logoutUser()
        }
        // one who may view Configuration but not change it
        ec.user.internalLoginUser(usernameOf(VIEWER))
        let viewer
        try {
            viewer = call({profileId: "OPENFREEMAP", enabled: "Y"})
        } catch (e) {
            viewer = {errors: String(e.message || e)}
            ec.message.clearErrors()
        } finally {
            ec.user.logoutUser()
        }
        return {
            viewer,
            user: as(WRITER, "apps/maps/Configuration", {}),
            userCall: as(
                WRITER,
                "apps/maps/Configuration/Settings/storeProfile",
                {profileId: "OUTLINE", makeDefault: "Y"},
                "post"
            ),
            entry: ec.entity
                .find("moqui.server.SettingsEntry")
                .condition("settingsKey", "maps.configuration")
                .disableAuthz()
                .one(),
            rules,
            installed: ec.entity
                .find("moqui.security.UserGroupPreference")
                .condition({userGroupId: "ALL_USERS", preferenceKey: "maps.profile"})
                .disableAuthz()
                .one()?.preferenceValue,
            profile: ec.entity.find("moqui.maps.MapProfile").condition("profileId", "OPENFREEMAP").disableAuthz().one(),
            // enabled then disabled by the administrator: two audit rows naming them
            audited: Array.from(
                ec.entity
                    .find("moqui.entity.EntityAuditLog")
                    .condition({
                        changedEntityName: "moqui.maps.MapProfile",
                        changedFieldName: "disabled",
                        pkPrimaryValue: "OPENFREEMAP"
                    })
                    .disableAuthz()
                    .list()
            )
        }
    },
    then({viewer, user, userCall, entry, rules, installed, profile, audited}) {
        viewer.applied !== true
        viewer.reasonCode == "PERMISSION_DENIED" || /authoriz/i.test(viewer.errors)
        refused(user)
        refused(userCall)
        entry.destinationRoute == "mapsConfiguration"
        entry.screenLocation == "component://tools/screen/Maps/Configuration.xml"
        rules.enable.applied == true
        rules.enable.reason.includes("not drawn yet")
        rules.makeDefault.applied == false
        rules.makeDefault.reasonCode == "PROFILE_UNAVAILABLE"
        rules.disableDefault.applied == false
        rules.disableDefault.reasonCode == "DEFAULT_PROFILE"
        rules.disable.applied == true
        rules.unknown.reasonCode == "RECORD_UNAVAILABLE"
        installed == "OUTLINE"
        profile.disabled == "Y"
        audited.length >= 2
        audited.every(row => !!row.changedByUserId)
    }
})

feature("an app's own allow of Maps.xml opens neither Configuration nor a Geo data service: MAPS_ADMIN is the user's own grant", {
    when() {
        // a group of another app given the Maps app (an inheritable allow of Maps.xml), not MAPS_USERS and not MAPS_ADMIN
        const GROUP = "MAPS_SCR_APP_GROUP"
        const USER = "MAPS_SCR_APP_USER"
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: USER, username: usernameOf(USER), userFullName: "Maps screens app user"})
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Maps screens spec: an app's group"})
        make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: USER, fromDate: ec.user.nowTimestamp})
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: GROUP,
            userGroupId: GROUP,
            artifactGroupId: "MAPS_APP",
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        ec.artifactExecution.enableAuthz()
        try {
            const screen = as(USER, "apps/maps/Configuration/Archives", {})
            // a service MAPS_ADMIN names, called under Maps.xml, whose allow the frame passes down
            ec.user.internalLoginUser(usernameOf(USER))
            let service
            try {
                ec.artifactExecution.push("component://tools/screen/Maps.xml", "AT_XML_SCREEN", "AUTHZA_VIEW", true)
                try {
                    const out = ec.service.sync().name("org.moqui.maps.GeoDataServices.find#GeoDataSets").call() || {}
                    service = {errors: ec.message.getErrorsString(), dataSets: out.dataSetList}
                } finally {
                    ec.artifactExecution.pop()
                }
            } catch (e) {
                service = {errors: String(e.message || e)}
            } finally {
                ec.message.clearErrors()
                ec.user.logoutUser()
            }
            return {screen, service}
        } finally {
            ec.artifactExecution.disableAuthz()
            ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", GROUP).deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", GROUP).deleteAll()
            ec.entity.find("moqui.security.UserGroup").condition("userGroupId", GROUP).deleteAll()
            for (const entity of ["moqui.security.ArtifactAuthzFailure", "moqui.security.UserAccount"])
                try {
                    ec.entity.find(entity).condition("userId", USER).deleteAll()
                } catch (e) {
                    ec.logger.warn("MapsScreens: " + entity + " of " + USER + " not removed: " + (e.message || e))
                }
            ec.message.clearErrors()
            ec.artifactExecution.enableAuthz()
            ec.user.internalLoginUser("john.doe")
        }
    },
    then({screen, service}) {
        refused(screen)
        !screen.output.includes("Register an archive")
        String(service.errors).includes("PERMISSION_DENIED") || /not authorized/i.test(service.errors)
        !service.dataSets
    }
})
