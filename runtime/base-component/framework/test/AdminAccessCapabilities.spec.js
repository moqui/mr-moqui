// Capabilities, row rules and the access review, org.moqui.admin.AccessServices (mr-boss plan/impl-admin.md P9; design-admin §5
// Capabilities and RowRules, §16): the matrix keeps a launcher apart from a view and a change and its list twin holds the same
// cells, with a rule no registered bundle describes read only; a row rule is bound only as a filter set a registered bundle
// names for that artifact group, on a binding of the registered type, at the grant revision, and never removed; the review
// leaves sealed grants out, counted, as of its date. With artifact authorization on, as a spec access administrator who holds
// ADMIN_ACCESS_VIEW and ADMIN_ACCESS_MANAGE through a group of its own (not ADMIN). The grant rules themselves are
// AdminAccessServices'. And the matrix as the Capabilities screen shows and changes it (get#MatrixContext, apply#MatrixChanges):
// its list twin, a proposal's controls and effect, a refused group named while the others apply, the bounds and the permissions.
// Run: ./moq test framework --spec AdminAccessCapabilities
const S = "org.moqui.admin.AccessServices."
const adminUser = "spec.cap.admin"
const adminId = "SPEC_CAP_ADMIN"
const readerId = "SPEC_CAP_READER"
const adminsId = "SPEC_CAP_ADMINS"
const bundleId = "SPEC_CAP_BUNDLE"
const teamId = "SPEC_CAP_TEAM"
const looseId = "SPEC_CAP_LOOSE"
const scopedId = "SPEC_CAP_SCOPED"
const plainId = "SPEC_CAP_PLAIN"
const freshId = "SPEC_CAP_FRESH"
const appGroupId = "SPEC_CAP_APP"
const otherAppGroupId = "SPEC_CAP_OTHER_APP"
const setId = "SPEC_CAP_ORG"
const otherSetId = "SPEC_CAP_OTHER_SET"
const viewPermission = "SPEC_CAP_VIEW"
const editPermission = "SPEC_CAP_EDIT"
const loosePermission = "SPEC_CAP_LOOSE_RUN"
const day = 86400000

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const find = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const rows = (entityName, conditions) => Array.from(find(entityName, conditions).list())
function as(username, action) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
    }
}
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
const cellOf = (answer, capabilityKey, groupId) =>
    (answer.capabilityList || [])
        .find(row => row.capabilityKey === capabilityKey)
        ?.cells.find(cell => cell.userGroupId === groupId)
const boundSets = groupId =>
    rows("moqui.security.ArtifactAuthz", {userGroupId: groupId, artifactGroupId: appGroupId}).flatMap(authz =>
        rows("moqui.security.ArtifactAuthzFilter", {artifactAuthzId: authz.artifactAuthzId}).map(row => row.entityFilterSetId)
    )

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: adminId, username: adminUser, userFullName: "Spec capability administrator"})
        make("moqui.security.UserAccount", {
            userId: readerId,
            username: "spec.cap.reader",
            userFullName: "Spec capability reader"
        })
        make("moqui.security.UserGroup", {userGroupId: adminsId, description: "Spec capability administrators"})
        for (const permission of ["ADMIN_ACCESS_VIEW", "ADMIN_ACCESS_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: adminsId, userPermissionId: permission, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: adminsId, userId: adminId, fromDate: 0})
        // a registered bundle: a launcher with its filter set, a view and a change permission
        make("moqui.security.UserGroup", {userGroupId: bundleId, description: "Spec capability bundle"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: appGroupId, description: "Spec capability app"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: otherAppGroupId, description: "Spec other app"})
        make("moqui.security.EntityFilterSet", {entityFilterSetId: setId, description: "Spec organization rule"})
        make("moqui.security.EntityFilterSet", {entityFilterSetId: otherSetId, description: "Spec other rule"})
        make("moqui.security.CapabilityBundle", {
            userGroupId: bundleId,
            appName: "SpecCap",
            description: "Spec capability bundle",
            scopeDependency: "organization"
        })
        make("moqui.security.CapabilityBundlePermission", {
            userGroupId: bundleId,
            userPermissionId: viewPermission,
            capabilityKindEnumId: "CpkView"
        })
        make("moqui.security.CapabilityBundlePermission", {
            userGroupId: bundleId,
            userPermissionId: editPermission,
            capabilityKindEnumId: "CpkEdit"
        })
        make("moqui.security.CapabilityBundleArtifact", {
            userGroupId: bundleId,
            artifactGroupId: appGroupId,
            description: "Spec capability launcher",
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL",
            entityFilterSetIds: setId
        })
        // a custom group with nothing yet; and one as Tools left it: the launcher bound with no row rule, a rule and a
        // permission no bundle describes, a sealed grant, and a grant that ended ten days ago
        make("moqui.security.UserGroup", {
            userGroupId: teamId,
            description: "Spec capability team",
            groupTypeEnumId: "UgtCustomTeam"
        })
        make("moqui.security.UserGroup", {userGroupId: looseId, description: "Spec loose team", groupTypeEnumId: "UgtCustomTeam"})
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: "SPEC_CAP_LOOSE_APP",
            userGroupId: looseId,
            artifactGroupId: appGroupId,
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: "SPEC_CAP_LOOSE_OTHER",
            userGroupId: looseId,
            artifactGroupId: otherAppGroupId,
            authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"
        })
        // a group whose launcher binding carries a rule of its own, as an administrator group's does
        make("moqui.security.UserGroup", {
            userGroupId: scopedId,
            description: "Spec scoped team",
            groupTypeEnumId: "UgtCustomTeam"
        })
        make("moqui.security.ArtifactAuthz", {
            artifactAuthzId: "SPEC_CAP_SCOPED_APP",
            userGroupId: scopedId,
            artifactGroupId: appGroupId,
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        make("moqui.security.ArtifactAuthzFilter", {artifactAuthzId: "SPEC_CAP_SCOPED_APP", entityFilterSetId: otherSetId})
        make("moqui.security.UserGroupPermission", {userGroupId: looseId, userPermissionId: loosePermission, fromDate: 0})
        make("moqui.security.UserGroupPermission", {userGroupId: looseId, userPermissionId: "ADMIN_PASSWORD", fromDate: 0})
        make("moqui.security.UserGroupPermission", {
            userGroupId: looseId,
            userPermissionId: editPermission,
            fromDate: 0,
            thruDate: Date.now() - 10 * day
        })
        make("moqui.security.UserGroupMember", {userGroupId: looseId, userId: readerId, fromDate: 0})
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        const groups = [teamId, looseId, scopedId, bundleId, adminsId, plainId, freshId]
        for (const authz of ec.entity.find("moqui.security.ArtifactAuthz").condition("userGroupId", "in", groups).list())
            find("moqui.security.ArtifactAuthzFilter", {artifactAuthzId: authz.artifactAuthzId}).deleteAll()
        for (const entityName of [
            "moqui.security.ArtifactAuthz",
            "moqui.security.CapabilityBundleArtifact",
            "moqui.security.CapabilityBundlePermission",
            "moqui.security.CapabilityBundle",
            "moqui.security.UserGroupPermission",
            "moqui.security.UserGroupMember"
        ])
            ec.entity.find(entityName).condition("userGroupId", "in", groups).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", groups).deleteAll()
        ec.entity.find("moqui.security.EntityFilterSet").condition("entityFilterSetId", "in", [setId, otherSetId]).deleteAll()
        ec.entity
            .find("moqui.security.ArtifactGroup")
            .condition("artifactGroupId", "in", [appGroupId, otherAppGroupId])
            .deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", "in", [adminId, readerId]).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature(
    "the matrix keeps a launcher apart from a view and a change, its list holds the same cells, and an unregistered rule is read only",
    {
        when() {
            const launcher = [{action: "add", artifactGroupId: appGroupId}]
            const asAdmin = as(adminUser, () => {
                const preview = invoke(S + "preview#Grants", {userGroupId: teamId, grantList: launcher})
                const applied = invoke(S + "apply#Grants", {
                    userGroupId: teamId,
                    grantList: launcher,
                    expectedRevision: preview.revision
                })
                const matrix = invoke(S + "get#Capabilities", {
                    appName: "SpecCap",
                    userGroupIdList: [teamId, looseId],
                    proposedList: [{userGroupId: teamId, action: "add", userPermissionId: viewPermission}]
                })
                const tooMany = invoke(S + "get#Capabilities", {userGroupIdList: Array.from({length: 9}, (unused, n) => "G" + n)})
                return {applied, matrix, tooMany}
            })
            const matrix = asAdmin.matrix
            // every cell of the matrix is a row of the list with the same state and controls, and nothing more
            const listed = new Map((matrix.cellList || []).map(row => [row.capabilityKey + "|" + row.userGroupId, row]))
            const cells = (matrix.capabilityList || []).flatMap(row => row.cells.map(cell => ({row, cell})))
            const disagreeing = cells.filter(({row, cell}) => {
                const item = listed.get(row.capabilityKey + "|" + cell.userGroupId)
                return (
                    !item ||
                    ["state", "canAdd", "canEnd", "proposed", "kind"].some(
                        key => item[key] !== (key === "kind" ? row[key] : cell[key])
                    )
                )
            })
            const reader = as("spec.cap.reader", () => invoke(S + "get#Capabilities", {appName: "SpecCap"}))
            return Object.assign(asAdmin, {
                cellCount: cells.length,
                listCount: listed.size,
                disagreeing,
                reader,
                teamPermissions: rows("moqui.security.UserGroupPermission", {userGroupId: teamId})
            })
        },
        then({applied, matrix, tooMany, cellCount, listCount, disagreeing, reader, teamPermissions}) {
            applied.applied == true
            // the launcher alone: no view, no change, and no permission row written
            matrix.capabilityList.map(row => row.kind).join() == "launcher,view,edit"
            cellOf(matrix, "a:" + appGroupId, teamId).state == "granted"
            cellOf(matrix, "a:" + appGroupId, teamId).filterSetIds.join() == setId
            cellOf(matrix, "p:" + viewPermission, teamId).state == "none"
            cellOf(matrix, "p:" + editPermission, teamId).state == "none"
            teamPermissions.length == 0
            cellOf(matrix, "p:" + viewPermission, teamId).canAdd == true
            cellOf(matrix, "p:" + viewPermission, teamId).proposed == "add"
            cellOf(matrix, "a:" + appGroupId, teamId).canEnd == true
            // Tools' binding with no row rule is the registered type: granted, and marked as lacking its registered filter set
            cellOf(matrix, "a:" + appGroupId, looseId).unscoped == true
            // the ended change grant reads as none
            cellOf(matrix, "p:" + editPermission, looseId).state == "none"
            cellCount == 6 && listCount == 6
            disagreeing.length == 0
            // what no registered bundle describes: listed apart, with no control
            matrix.unsupportedList
                .map(row => row.id)
                .sort()
                .join() == ["ADMIN_PASSWORD", loosePermission, otherAppGroupId].sort().join()
            matrix.unsupportedList.find(row => row.id === otherAppGroupId).typeText == "Always"
            matrix.unsupportedList.every(row => row.userGroupId === looseId)
            tooMany.error.includes("at most 8 groups")
            reader.error.includes("ADMIN_ACCESS_VIEW")
        }
    }
)

feature("a row rule is bound only as a registered filter set on a registered binding, at the grant revision, and never removed", {
    when() {
        const scope = (filterSetId, artifactGroupId = appGroupId, action = "add") => [
            {action, artifactGroupId, entityFilterSetId: filterSetId}
        ]
        const asAdmin = as(adminUser, () => {
            const before = invoke(S + "get#ScopeRules", {userGroupId: looseId})
            const missing = before.ruleList.find(row => row.state === "missing")
            const unregistered = invoke(S + "preview#Grants", {userGroupId: looseId, grantList: scope(otherSetId)})
            const removal = invoke(S + "preview#Grants", {userGroupId: looseId, grantList: scope(setId, appGroupId, "end")})
            const unbound = invoke(S + "preview#Grants", {userGroupId: bundleId, grantList: scope(setId)})
            const advanced = invoke(S + "preview#Grants", {userGroupId: looseId, grantList: scope(setId, otherAppGroupId)})
            const stale = invoke(S + "apply#Grants", {userGroupId: looseId, grantList: scope(setId), expectedRevision: "stale"})
            const configured = invoke(S + "preview#Grants", {userGroupId: scopedId, grantList: scope(setId)})
            const scopedRules = invoke(S + "get#ScopeRules", {userGroupId: scopedId})
            const bound = invoke(S + "apply#Grants", {
                userGroupId: looseId,
                grantList: scope(setId),
                expectedRevision: missing?.grantRevision,
                reason: "Spec scope"
            })
            // the revision covers a binding's filter sets: the one read before the rule was bound is stale now
            const replay = invoke(S + "apply#Grants", {
                userGroupId: looseId,
                grantList: [{action: "add", userPermissionId: viewPermission}],
                expectedRevision: missing?.grantRevision
            })
            const again = invoke(S + "preview#Grants", {userGroupId: looseId, grantList: scope(setId)})
            const after = invoke(S + "get#ScopeRules", {userGroupId: looseId})
            const team = invoke(S + "get#ScopeRules", {userGroupId: teamId})
            return {
                missing,
                unregistered,
                removal,
                unbound,
                advanced,
                stale,
                configured,
                scopedRules,
                bound,
                replay,
                again,
                after,
                team
            }
        })
        // a second bundle registers the launcher with no row rule: a rule asked for beside the launcher taken from it is
        // bound all the same, whatever the order of the request
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserGroup", {userGroupId: plainId, description: "Spec plain bundle"})
        make("moqui.security.UserGroup", {userGroupId: freshId, description: "Spec fresh team", groupTypeEnumId: "UgtCustomTeam"})
        make("moqui.security.CapabilityBundle", {userGroupId: plainId, appName: "SpecCapPlain", description: "Spec plain bundle"})
        make("moqui.security.CapabilityBundleArtifact", {
            userGroupId: plainId,
            artifactGroupId: appGroupId,
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        ec.artifactExecution.enableAuthz()
        const together = as(adminUser, () => {
            const grantList = scope(setId).concat([{action: "add", artifactGroupId: appGroupId, bundleGroupId: plainId}])
            const preview = invoke(S + "preview#Grants", {userGroupId: freshId, grantList})
            return invoke(S + "apply#Grants", {userGroupId: freshId, grantList, expectedRevision: preview.revision})
        })
        for (const entityName of ["moqui.security.CapabilityBundleArtifact", "moqui.security.CapabilityBundle"])
            find(entityName, {userGroupId: plainId}).deleteAll()
        return Object.assign(asAdmin, {
            together,
            freshSets: boundSets(freshId),
            sets: boundSets(looseId),
            audit: rows("moqui.entity.EntityAuditLog", {
                changedEntityName: "moqui.security.ArtifactAuthzFilter",
                pkPrimaryValue: "SPEC_CAP_LOOSE_APP"
            })
        })
    },
    then({
        missing,
        unregistered,
        removal,
        unbound,
        advanced,
        stale,
        configured,
        scopedRules,
        bound,
        replay,
        again,
        after,
        team,
        together,
        freshSets,
        sets,
        audit
    }) {
        missing.entityFilterSetId == setId && missing.canBind == true && missing.artifactGroupId == appGroupId
        unregistered.allowed == false && unregistered.reasonCode == "UNREGISTERED_SCOPE"
        removal.allowed == false && removal.reasonCode == "SCOPE_REMOVAL"
        unbound.allowed == false && unbound.reasonCode == "SCOPE_UNBOUND"
        // a rule no bundle registers takes no row rule here
        advanced.allowed == false && advanced.reasonCode == "UNREGISTERED_SCOPE"
        stale.applied == false && stale.reasonCode == "REVISION_CHANGED"
        // a binding scoped on purpose with a rule of its own is not completed, and is not listed as lacking one
        configured.allowed == false && configured.reasonCode == "SCOPE_CONFIGURED"
        scopedRules.missingCount == 0 && scopedRules.ruleList[0].source == "configuration"
        bound.applied == true && bound.added == 1
        replay.applied == false && replay.reasonCode == "REVISION_CHANGED"
        sets.join() == setId
        audit.length == 1 && audit[0].newValueText == setId && audit[0].changeReason == "Spec scope"
        again.changeList[0].noChange == true
        together.applied == true && together.added == 2 && freshSets.join() == setId
        after.missingCount == 0
        after.ruleList.find(row => row.artifactGroupId === appGroupId).source == "bundle"
        // the launcher granted from the bundle came with its row rule
        team.ruleList.length == 1 && team.ruleList[0].state == "bound" && team.ruleList[0].entityFilterSetId == setId
        // an expression is never part of the answer
        !JSON.stringify(after).includes("applyCond") && !JSON.stringify(after).includes("filterMap")
    }
})

feature("the review lists grants, app gates and members as of its date and leaves sealed grants out, counted", {
    when() {
        return as(adminUser, () => ({
            now: invoke(S + "get#GrantReview", {userGroupIdList: [looseId]}),
            earlier: invoke(S + "get#GrantReview", {userGroupIdList: [looseId], asOfDate: Date.now() - 20 * day}),
            byApp: invoke(S + "get#GrantReview", {appName: "SpecCap"}),
            none: invoke(S + "get#GrantReview", {})
        }))
    },
    then({now, earlier, byApp, none}) {
        now.entryList
            .filter(row => row.entryType === "permission")
            .map(row => row.id)
            .join() == loosePermission
        !JSON.stringify(now).includes("ADMIN_PASSWORD")
        now.sealedOmittedCount == 1 && now.groupList[0].sealedOmitted == 1
        now.entryList.find(row => row.id === appGroupId).scope == setId
        now.entryList.find(row => row.id === otherAppGroupId).registered == false
        now.memberList.map(row => row.username).join() == "spec.cap.reader"
        // twenty days ago the change grant was still open
        earlier.entryList.some(row => row.id === editPermission && row.kindLabel == "Change")
        byApp.groupList.map(row => row.userGroupId).join() == bundleId
        none.error.includes("Name the groups or the app")
    }
})

// The matrix as the Capabilities screen shows and changes it (AccessMatrix.js; plans/Admin-Screens-Down.md A3, first IronStone
// Admin's): the proposal is the tokens the screen carries, nothing stored until it is applied
feature(
    "the matrix context marks a proposal with its controls and effect, and applies it group by group, naming a refused group",
    {
        when() {
            const token = (groupId, action, key) => groupId + "~" + action + "~" + key
            const viewToken = token(teamId, "a", "p:" + viewPermission)
            // a permission no registered bundle offers: refused whoever asks
            const refusedToken = token(looseId, "a", "p:" + loosePermission)
            const both = [viewToken, refusedToken].join(",")
            const asAdmin = as(adminUser, () => {
                const context = invoke(S + "get#MatrixContext", {
                    appName: "SpecCap",
                    groups: [teamId, looseId].join(","),
                    changes: viewToken
                })
                const refused = invoke(S + "get#MatrixContext", {changes: both})
                const applied = invoke(S + "apply#MatrixChanges", {
                    changes: both,
                    revisions: refused.proposal?.revisions,
                    reason: "Spec matrix"
                })
                // the team's grants changed with the first apply: the revision previewed before it is stale
                const stale = invoke(S + "apply#MatrixChanges", {
                    changes: token(teamId, "e", "p:" + viewPermission),
                    revisions: refused.proposal?.revisions
                })
                // over a bound: answered as the reason the screen shows, the proposal kept so it can be cleared
                const bounded = invoke(S + "get#MatrixContext", {
                    groups: Array.from({length: 9}, (unused, n) => "SPEC_G" + n).join(","),
                    changes: viewToken
                })
                const tooMany = invoke(S + "get#MatrixContext", {
                    changes: Array.from({length: 41}, (unused, n) => token(teamId, "a", "p:SPEC_P" + n)).join(",")
                })
                const none = invoke(S + "apply#MatrixChanges", {changes: "not a token"})
                return {context, refused, applied, stale, bounded, tooMany, none}
            })
            const context = asAdmin.context
            // every cell of the matrix is a row of the list with the same texts and controls
            const listed = new Map((context.listRows || []).map(row => [row.capabilityKey + "|" + row.userGroupId, row]))
            const cells = (context.matrixRows || []).flatMap(row => row.cells.map(cell => ({row, cell})))
            const differing = cells.filter(({row, cell}) => {
                const item = listed.get(row.capabilityKey + "|" + cell.userGroupId)
                return (
                    !item ||
                    ["state", "stateText", "proposedText", "actionText", "nextChanges", "scopeText"].some(
                        key => item[key] !== cell[key]
                    )
                )
            })
            const cell = key =>
                context.matrixRows?.find(row => row.capabilityKey === key)?.cells.find(c => c.userGroupId === teamId)
            return Object.assign(asAdmin, {
                cellCount: cells.length,
                listCount: listed.size,
                differing,
                launcherCell: cell("a:" + appGroupId),
                viewCell: cell("p:" + viewPermission),
                editCell: cell("p:" + editPermission),
                editToken: token(teamId, "a", "p:" + editPermission),
                viewToken,
                refusedToken,
                teamPermissions: rows("moqui.security.UserGroupPermission", {userGroupId: teamId}).map(
                    row => row.userPermissionId
                ),
                // a user with neither permission, with the parameters that answer the administrator
                reader: as("spec.cap.reader", () => ({
                    context: invoke(S + "get#MatrixContext", {appName: "SpecCap"}),
                    applied: invoke(S + "apply#MatrixChanges", {changes: viewToken})
                }))
            })
        },
        then({
            context,
            refused,
            applied,
            stale,
            bounded,
            tooMany,
            none,
            cellCount,
            listCount,
            differing,
            launcherCell,
            viewCell,
            editCell,
            editToken,
            viewToken,
            refusedToken,
            teamPermissions,
            reader
        }) {
            context.error == ""
            cellCount == 6 && listCount == 6
            differing.length == 0
            // what the team holds ends, what it lacks is granted, and a proposed cell withdraws its token
            launcherCell.stateText == "Granted" && launcherCell.actionText == "End" && launcherCell.scopeText == setId
            viewCell.proposed == "add" && viewCell.proposedText == "Grant proposed"
            viewCell.actionText == "Withdraw" && viewCell.nextChanges == ""
            editCell.actionText == "Grant" && editCell.nextChanges == [viewToken, editToken].join(",")
            // the proposal's effect on its group, from preview#Grants, with the revision to apply it at
            context.changeCount == 1 && context.proposal.canApply == true
            context.proposal.rows[0].actionText == "Grant" && context.proposal.rows[0].resultText.includes("granted")
            JSON.parse(context.proposal.revisions)[teamId] == context.proposal.groups[0].revision
            // one group of a proposal refused: nothing is offered to apply, and applying names it and applies the other
            refused.proposal.canApply == false
            refused.proposal.groups.find(row => row.userGroupId === looseId).allowed == false
            applied.appliedCount == 1 && applied.refusedCount == 1
            applied.resultList.find(row => row.userGroupId === looseId).reasonCode == "UNREGISTERED_GRANT"
            applied.resultList.find(row => row.userGroupId === teamId).added == 1
            applied.remainingChanges == refusedToken
            teamPermissions.join() == viewPermission
            stale.appliedCount == 0 && stale.resultList[0].reasonCode == "REVISION_CHANGED"
            bounded.failure.includes("at most 8 groups") && bounded.changes == viewToken && bounded.columns.length == 0
            tooMany.failure.includes("more than 40") && tooMany.changeCount == 41
            none.error.includes("no proposed change")
            reader.context.error.includes("ADMIN_ACCESS_VIEW")
            reader.applied.error.includes("ADMIN_ACCESS_MANAGE")
        }
    }
)
