// Delegated access administration, org.moqui.admin.AccessServices and IdentityServices.check#AdministrationPath (mr-boss
// plan/impl-admin.md P3; design-admin §5, §1.1): a custom group is never privileged; grants only from a registered bundle and a
// sealed one only by its holder; memberships only in a registered bundle or a custom group and a privileged one only by a member,
// each dated and ended, never deleted, with a stale revision refused, and a grant proposal by the same rules; the last
// administration path refused with a reason; and a change seen at once by the session it changes. Each case with artifact
// authorization on, as a spec access administrator who holds ADMIN_ACCESS_VIEW and ADMIN_ACCESS_MANAGE through a group of its
// own (not ADMIN), and as john.doe (ADMIN) where a holder or a member is the point. The EECAs themselves are tested in cargo
// (service::eca_tests).
// Run: ./moq test framework --spec AdminAccessServices
const S = "org.moqui.admin.AccessServices."
const adminUser = "spec.access.admin"
const adminId = "SPEC_ACC_ADMIN"
const targetId = "SPEC_ACC_TARGET"
const bundleId = "SPEC_ACC_BUNDLE"
const teamId = "SPEC_ACC_TEAM"
const unregisteredId = "SPEC_ACC_UNREGISTERED"
const sealedGroupId = "SPEC_ACC_SEALED"
const appGroupId = "SPEC_ACC_APP"
const preferenceKey = "SPEC_ACC_PAGE_SIZE"
const viewPermission = "SPEC_ACC_VIEW"

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const find = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const rows = (entityName, conditions) => Array.from(find(entityName, conditions).list())
// Run action signed in as username with artifact authorization on, then as john.doe again.
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
// A service's result, or {error} with its message.
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
const audits = (entityName, fieldName, pk) =>
    rows("moqui.entity.EntityAuditLog", {changedEntityName: entityName, changedFieldName: fieldName, pkPrimaryValue: pk})
const groupRevision = groupId => invoke(S + "get#Group", {userGroupId: groupId}).group.revision

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: adminId, username: adminUser, userFullName: "Spec access administrator"})
        make("moqui.security.UserAccount", {userId: targetId, username: "spec.access.target", userFullName: "Spec access target"})
        make("moqui.security.UserGroup", {userGroupId: "SPEC_ACC_ADMINS", description: "Spec access administrators"})
        for (const permission of ["ADMIN_ACCESS_VIEW", "ADMIN_ACCESS_MANAGE"])
            make("moqui.security.UserGroupPermission", {
                userGroupId: "SPEC_ACC_ADMINS",
                userPermissionId: permission,
                fromDate: 0
            })
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_ACC_ADMINS", userId: adminId, fromDate: 0})
        // a registered bundle carrying a plain view permission and the sealed ADMIN_PASSWORD, with an app binding; and the
        // privileged ADMIN_ADV registered too, so only the privileged-group rule refuses its membership
        make("moqui.security.UserGroup", {userGroupId: bundleId, description: "Spec bundle"})
        make("moqui.security.UserGroup", {userGroupId: unregisteredId, description: "Spec unregistered group"})
        // a custom group an instance administrator gave the sealed ADMIN_PASSWORD
        make("moqui.security.UserGroup", {
            userGroupId: sealedGroupId,
            description: "Spec sealed team",
            groupTypeEnumId: "UgtCustomTeam"
        })
        make("moqui.security.UserGroupPermission", {userGroupId: sealedGroupId, userPermissionId: "ADMIN_PASSWORD", fromDate: 0})
        make("moqui.security.ArtifactGroup", {artifactGroupId: appGroupId, description: "Spec app"})
        make("moqui.security.CapabilityBundle", {userGroupId: bundleId, appName: "Spec", description: "Spec bundle"})
        make("moqui.security.CapabilityBundle", {userGroupId: "ADMIN_ADV", appName: "Spec", description: "Spec privileged"})
        make("moqui.security.CapabilityBundlePermission", {
            userGroupId: bundleId,
            userPermissionId: viewPermission,
            capabilityKindEnumId: "CpkView"
        })
        make("moqui.security.CapabilityBundlePermission", {
            userGroupId: bundleId,
            userPermissionId: "ADMIN_PASSWORD",
            capabilityKindEnumId: "CpkSensitive"
        })
        // a permission only the privileged ADMIN_ADV's bundle offers: granted from it only by a member of ADMIN_ADV
        make("moqui.security.CapabilityBundlePermission", {
            userGroupId: "ADMIN_ADV",
            userPermissionId: "SPEC_ACC_ADV_RUN",
            capabilityKindEnumId: "CpkEdit"
        })
        make("moqui.security.CapabilityBundleArtifact", {
            userGroupId: bundleId,
            artifactGroupId: appGroupId,
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        make("moqui.basic.Enumeration", {
            enumId: preferenceKey,
            enumTypeId: "UserPreferenceKey",
            description: "Spec page size",
            optionValue: "number"
        })
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        const groups = [teamId, bundleId, unregisteredId, sealedGroupId, "SPEC_ACC_ADMINS"]
        for (const authz of rows("moqui.security.ArtifactAuthz", {artifactGroupId: appGroupId}))
            find("moqui.security.ArtifactAuthzFilter", {artifactAuthzId: authz.artifactAuthzId}).deleteAll()
        find("moqui.security.ArtifactAuthz", {artifactGroupId: appGroupId}).deleteAll()
        for (const entityName of [
            "moqui.security.CapabilityBundleArtifact",
            "moqui.security.CapabilityBundlePermission",
            "moqui.security.CapabilityBundle"
        ])
            ec.entity
                .find(entityName)
                .condition("userGroupId", "in", groups.concat(["ADMIN_ADV"]))
                .deleteAll()
        for (const entityName of [
            "moqui.security.UserGroupPreference",
            "moqui.security.UserGroupPermission",
            "moqui.security.UserGroupMember"
        ])
            ec.entity.find(entityName).condition("userGroupId", "in", groups).deleteAll()
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", [adminId, targetId]).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", groups).deleteAll()
        find("moqui.security.ArtifactGroup", {artifactGroupId: appGroupId}).deleteAll()
        find("moqui.basic.Enumeration", {enumId: preferenceKey}).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", "in", [adminId, targetId]).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a custom group is never privileged, and the list marks privileged groups and sealed grants", {
    when() {
        return as(adminUser, () => ({
            created: invoke(S + "create#Group", {
                userGroupId: teamId,
                description: "Spec team",
                groupTypeEnumId: "UgtCustomTeam"
            }),
            privileged: invoke(S + "create#Group", {
                userGroupId: "ADMIN_ADV",
                description: "Taken",
                groupTypeEnumId: "UgtCustomTeam"
            }),
            again: invoke(S + "create#Group", {userGroupId: teamId, description: "Again", groupTypeEnumId: "UgtCustomTeam"}),
            groups: invoke(S + "find#Groups", {}).groupList
        }))
    },
    then({created, privileged, again, groups}) {
        created.applied == true
        privileged.reasonCode == "PRIVILEGED_GROUP"
        again.reasonCode == "GROUP_EXISTS"
        groups.find(g => g.userGroupId === teamId).kind == "custom"
        groups.find(g => g.userGroupId === bundleId).registered == true
        groups.find(g => g.userGroupId === "ADMIN").privileged == true
        groups.find(g => g.userGroupId === "ADMIN").sealedPermissionIds.includes("ADMIN_PASSWORD")
        groups.find(g => g.userGroupId === unregisteredId).kind == "other"
    }
})

feature("grants come only from a registered bundle, a sealed one only from its holder, and never from a stale preview", {
    when() {
        const bundleGrant = [{action: "add", bundleGroupId: bundleId}]
        const plainGrant = [
            {action: "add", userPermissionId: viewPermission},
            {action: "add", artifactGroupId: appGroupId}
        ]
        const asAdmin = as(adminUser, () => {
            const preview = invoke(S + "preview#Grants", {userGroupId: teamId, grantList: bundleGrant})
            const sealed = invoke(S + "apply#Grants", {
                userGroupId: teamId,
                grantList: bundleGrant,
                expectedRevision: preview.revision
            })
            const unregistered = invoke(S + "preview#Grants", {
                userGroupId: teamId,
                grantList: [{action: "add", userPermissionId: "SPEC_ACC_OTHER"}]
            })
            const privileged = invoke(S + "preview#Grants", {
                userGroupId: teamId,
                grantList: [{action: "add", userPermissionId: "SPEC_ACC_ADV_RUN"}]
            })
            const plain = invoke(S + "preview#Grants", {userGroupId: teamId, grantList: plainGrant})
            const stale = invoke(S + "apply#Grants", {userGroupId: teamId, grantList: plainGrant, expectedRevision: "stale"})
            const applied = invoke(S + "apply#Grants", {
                userGroupId: teamId,
                grantList: plainGrant,
                expectedRevision: plain.revision,
                reason: "Spec team reads"
            })
            return {preview, sealed, unregistered, privileged, stale, applied}
        })
        // john.doe holds ADMIN_PASSWORD (ADMIN): the sealed grant is his to give, and to end
        const holder = as("john.doe", () => {
            const grant = [{action: "add", userPermissionId: "ADMIN_PASSWORD"}]
            const preview = invoke(S + "preview#Grants", {userGroupId: teamId, grantList: grant})
            const added = invoke(S + "apply#Grants", {
                userGroupId: teamId,
                grantList: grant,
                expectedRevision: preview.revision
            })
            const end = [{action: "end", userPermissionId: "ADMIN_PASSWORD"}]
            const ended = invoke(S + "apply#Grants", {userGroupId: teamId, grantList: end, expectedRevision: added.revision})
            return {added, ended}
        })
        const grants = rows("moqui.security.UserGroupPermission", {userGroupId: teamId})
        return Object.assign(asAdmin, holder, {
            grants,
            authz: rows("moqui.security.ArtifactAuthz", {userGroupId: teamId, artifactGroupId: appGroupId}),
            audit: audits("moqui.security.UserGroupPermission", "fromDate", teamId)
        })
    },
    then({preview, sealed, unregistered, privileged, stale, applied, added, ended, grants, authz, audit}) {
        preview.allowed == false
        preview.changeList.find(c => c.id === "ADMIN_PASSWORD").reasonCode == "SEALED_PERMISSION"
        preview.changeList.find(c => c.id === viewPermission).allowed == true
        sealed.applied == false && sealed.reasonCode == "SEALED_PERMISSION"
        unregistered.reasonCode == "UNREGISTERED_GRANT"
        // a privileged group's bundle is not granted from by a non-member
        privileged.reasonCode == "UNREGISTERED_GRANT"
        stale.reasonCode == "REVISION_CHANGED"
        applied.applied == true && applied.added == 2
        authz.length == 1
        added.applied == true && ended.applied == true
        // the sealed grant is kept as an ended row, not deleted
        grants.find(r => r.userPermissionId === "ADMIN_PASSWORD").thruDate != null
        grants.find(r => r.userPermissionId === viewPermission).thruDate == null
        audit.some(r => r.changeReason === "Spec team reads")
    }
})

feature(
    "memberships are given only in registered bundles and custom groups, a privileged one only by a member and a sealed grant only by its holder, and are ended, not deleted",
    {
        when() {
            return as(adminUser, () => {
                const unregistered = invoke(S + "store#Membership", {userGroupId: unregisteredId, userId: targetId})
                const privileged = invoke(S + "store#Membership", {userGroupId: "ADMIN_ADV", userId: targetId})
                const sealed = invoke(S + "store#Membership", {userGroupId: sealedGroupId, userId: targetId})
                const added = invoke(S + "store#Membership", {
                    userGroupId: bundleId,
                    userId: targetId,
                    reason: "Spec bundle member"
                })
                const overlapping = invoke(S + "store#Membership", {userGroupId: bundleId, userId: targetId})
                const stale = invoke(S + "end#Membership", {
                    userGroupId: bundleId,
                    userId: targetId,
                    fromDate: added.fromDate,
                    expectedRevision: "stale"
                })
                const ended = invoke(S + "end#Membership", {
                    userGroupId: bundleId,
                    userId: targetId,
                    fromDate: added.fromDate,
                    expectedRevision: added.revision
                })
                const privilegedEnd = invoke(S + "end#Membership", {
                    userGroupId: "ADMIN",
                    userId: "EX_JOHN_DOE",
                    fromDate: "1265184000000",
                    expectedRevision: "x"
                })
                // a grant proposal (an account an app provisions, an invitation) by the same rules, nothing written
                const proposal = invoke(S + "check#GrantProposal", {
                    bundleGroupIdList: [bundleId, "ADMIN_ADV", unregisteredId]
                })
                return {unregistered, privileged, sealed, added, overlapping, stale, ended, privilegedEnd, proposal}
            })
        },
        then({unregistered, privileged, sealed, added, overlapping, stale, ended, privilegedEnd, proposal}) {
            const proposed = Object.fromEntries(Array.from(proposal.bundleList).map(row => [row.bundleGroupId, row]))
            proposal.allowed == false && proposed[bundleId].allowed == true
            proposed.ADMIN_ADV.allowed == false && proposed.ADMIN_ADV.reasonCode == "PRIVILEGED_GROUP"
            proposed[unregisteredId].reasonCode == "UNREGISTERED_BUNDLE"
            unregistered.reasonCode == "UNREGISTERED_BUNDLE"
            privileged.reasonCode == "PRIVILEGED_GROUP"
            sealed.reasonCode == "SEALED_PERMISSION"
            added.applied == true
            overlapping.reasonCode == "MEMBERSHIP_EXISTS"
            stale.reasonCode == "REVISION_CHANGED"
            ended.applied == true
            privilegedEnd.reasonCode == "PRIVILEGED_GROUP"
            rows("moqui.security.UserGroupMember", {userGroupId: bundleId, userId: targetId}).length == 1
            rows("moqui.security.UserGroupMember", {userGroupId: bundleId, userId: targetId})[0].thruDate != null
            audits("moqui.security.UserGroupMember", "thruDate", bundleId).length >= 1
            audits("moqui.security.UserGroupMember", "fromDate", bundleId).some(r => r.changeReason === "Spec bundle member")
        }
    }
)

feature("the last administration path is refused with a reason", {
    when() {
        const ending = rows("moqui.security.UserGroupMember", {userGroupId: "ADMIN"}).map(row => ({
            userGroupId: "ADMIN",
            userId: row.userId,
            fromDate: row.fromDate,
            thruDate: ec.user.nowTimestamp
        }))
        return as("john.doe", () => ({
            none: invoke("org.moqui.admin.IdentityServices.check#AdministrationPath", {proposed: {}}),
            allEnded: invoke("org.moqui.admin.IdentityServices.check#AdministrationPath", {proposed: {memberships: ending}}),
            factor: invoke(S + "store#Group", {
                userGroupId: "ADMIN",
                expectedRevision: groupRevision("ADMIN"),
                requireAuthcFactor: "Y"
            }),
            nonMember: as(adminUser, () =>
                invoke(S + "store#Group", {userGroupId: "ADMIN", expectedRevision: "x", requireAuthcFactor: "N"})
            )
        }))
    },
    then({none, allEnded, factor, nonMember}) {
        none.allowed == true && none.pathsBefore > 0
        allEnded.allowed == false && allEnded.reasonCode == "ADMINISTRATION_PATH" && allEnded.pathsAfter == 0
        factor.applied == false && factor.reasonCode == "ADMINISTRATION_PATH" && factor.reason.includes("administrator")
        nonMember.reasonCode == "PRIVILEGED_GROUP"
        rows("moqui.security.UserGroup", {userGroupId: "ADMIN"})[0].requireAuthcFactor != "Y"
    }
})

feature("a membership change is seen at once by the session it changes", {
    when() {
        return as(adminUser, () => {
            const before = ec.user.isInGroup(teamId)
            const added = invoke(S + "store#Membership", {userGroupId: teamId, userId: adminId})
            const during = {member: ec.user.isInGroup(teamId), permission: ec.user.hasPermission(viewPermission)}
            const ended = invoke(S + "end#Membership", {
                userGroupId: teamId,
                userId: adminId,
                fromDate: added.fromDate,
                expectedRevision: added.revision
            })
            return {
                before,
                added,
                during,
                ended,
                after: ec.user.isInGroup(teamId),
                afterPermission: ec.user.hasPermission(viewPermission)
            }
        })
    },
    then({before, added, during, ended, after, afterPermission}) {
        before == false
        added.applied == true && ended.applied == true
        during.member == true && during.permission == true
        after == false && afterPermission == false
    }
})

feature("a group's sign-in policy and preferences are typed, revisioned and audit-logged", {
    when() {
        return as(adminUser, () => {
            const revision = groupRevision(teamId)
            const stored = invoke(S + "store#Group", {
                userGroupId: teamId,
                expectedRevision: revision,
                ipAllowed: "10.0.*.*",
                requireAuthcFactor: "Y",
                reason: "Spec team policy"
            })
            const stale = invoke(S + "store#Group", {userGroupId: teamId, expectedRevision: revision, description: "Stale"})
            const badIp = invoke(S + "store#Group", {
                userGroupId: teamId,
                expectedRevision: stored.revision,
                ipAllowed: "not an address"
            })
            const unregistered = invoke(S + "store#GroupPreference", {
                userGroupId: teamId,
                preferenceKey: "SPEC_ACC_NONE",
                preferenceValue: "1"
            })
            const badValue = invoke(S + "store#GroupPreference", {userGroupId: teamId, preferenceKey, preferenceValue: "many"})
            const preference = invoke(S + "store#GroupPreference", {
                userGroupId: teamId,
                preferenceKey,
                preferenceValue: "25",
                reason: "Spec page size"
            })
            return {stored, stale, badIp, unregistered, badValue, preference}
        })
    },
    then({stored, stale, badIp, unregistered, badValue, preference}) {
        stored.applied == true
        stale.reasonCode == "REVISION_CHANGED"
        badIp.error.includes("Allowed IPs")
        unregistered.reasonCode == "UNREGISTERED_PREFERENCE"
        badValue.error.includes("number")
        preference.applied == true
        audits("moqui.security.UserGroup", "ipAllowed", teamId).some(
            r => r.changeReason === "Spec team policy" && r.newValueText === "10.0.*.*"
        )
        audits("moqui.security.UserGroup", "requireAuthcFactor", teamId).length == 1
        audits("moqui.security.UserGroupPreference", "preferenceValue", teamId).some(r => r.newValueText === "25")
    }
})
