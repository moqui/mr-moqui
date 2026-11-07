// The owner change of a shared drive and of a shared mail account (FileServices find#OwnedDrives and transfer#DriveOwner,
// EmailClientServices find#OwnedEmailAccounts and transfer#EmailAccountOwner; mr-boss plan/impl-admin.md P6, F31): what a user
// owns is theirs to see and another user's only with the owner-change permission; the new owner must be an enabled account
// that is not external, and only a shared drive or a shared mail account moves (a personal drive and a personal mail account
// stay). Each service implements the owned-resource contract of org.moqui.admin.IdentityServices, which an account's
// lifecycle relies on, calling it through call#OwnedResourceService (a service without the contract is never called).
// Run: ./moq test framework --spec ResourceOwners
const P = "SPEC_ROWN_"
const ownerId = P + "OWNER"
const heirId = P + "HEIR"
const operatorId = P + "OPERATOR"
const closedId = P + "CLOSED"
const externalId = P + "EXTERNAL"
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const one = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz().one()
function as(userId, action) {
    ec.user.internalLoginUser(one("moqui.security.UserAccount", {userId}).username)
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
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
// a service carries the contract's parameters (an implementing service gets them from the interface), which is what a
// coordinator checks before it calls a service a stored step names
const contracts = {
    OwnedResourceLookup: [["userId"], ["resourceList"]],
    OwnedResourceTransfer: [["fromUserId", "toUserId"], ["transferred"]]
}
function implementsContract(service, contract) {
    const definition = ec.service.getServiceDefinition(service)
    const [ins, outs] = contracts[contract]
    return (
        ins.every(name => Array.from(definition.inParameterNames || []).includes(name)) &&
        outs.every(name => Object.hasOwn(definition.outParameterInfoMap || {}, name))
    )
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [userId, extra] of [
            [ownerId, {}],
            [heirId, {}],
            [operatorId, {}],
            [closedId, {disabled: "Y"}],
            [externalId, {}]
        ])
            make(
                "moqui.security.UserAccount",
                Object.assign({userId, username: userId.toLowerCase(), userFullName: userId}, extra)
            )
        make("moqui.security.UserGroup", {userGroupId: P + "OPERATORS", description: "Spec owner changers"})
        for (const permission of ["DRIVE_OWNER_CHANGE", "EMAIL_ACCOUNT_OWNER_CHANGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: P + "OPERATORS", userPermissionId: permission, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: P + "OPERATORS", userId: operatorId, fromDate: 0})
        make("moqui.security.UserGroup", {userGroupId: P + "EXTERNAL", description: "Spec outside users"})
        make("moqui.security.UserGroupPermission", {userGroupId: P + "EXTERNAL", userPermissionId: "EXTERNAL_USER", fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: P + "EXTERNAL", userId: externalId, fromDate: 0})
        const drive = (driveId, driveKindEnumId) =>
            make("moqui.resource.ResourceDrive", {
                driveId,
                driveName: "Spec " + driveId,
                driveKindEnumId,
                rootLocation: "dbresource://drive/spec/" + driveId,
                ownerUserId: ownerId,
                createdDate: ec.user.nowTimestamp
            })
        drive(P + "SHARED", "RdkShared")
        drive(P + "PERSONAL", "RdkPersonal")
        make("moqui.resource.ResourceDriveMember", {
            driveMemberId: P + "HEIR_MEMBER",
            driveId: P + "SHARED",
            userId: heirId,
            permissionEnumId: "RdpEdit",
            fromDate: ec.user.nowTimestamp
        })
        const mail = (emailAccountId, accountKindEnumId) =>
            make("moqui.basic.email.EmailAccount", {
                emailAccountId,
                ownerUserId: ownerId,
                accountKindEnumId,
                accountName: "Spec " + emailAccountId,
                emailAddress: emailAccountId.toLowerCase() + "@example.com",
                statusId: "EasActive"
            })
        mail(P + "MAIL_SHARED", "EakShared")
        mail(P + "MAIL_OWN", "EakExternal")
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const [entityName, conditions] of [
            ["moqui.basic.email.EmailAccountUser", {emailAccountId: P + "MAIL_SHARED"}],
            ["moqui.basic.email.EmailAccount", {emailAccountId: P + "MAIL_SHARED"}],
            ["moqui.basic.email.EmailAccount", {emailAccountId: P + "MAIL_OWN"}],
            ["moqui.resource.ResourceDriveMember", {driveId: P + "SHARED"}],
            ["moqui.resource.ResourceDrive", {driveId: P + "SHARED"}],
            ["moqui.resource.ResourceDrive", {driveId: P + "PERSONAL"}]
        ])
            ec.entity.find(entityName).condition(conditions).deleteAll()
        for (const groupId of [P + "OPERATORS", P + "EXTERNAL"]) {
            ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", groupId).deleteAll()
            ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", groupId).deleteAll()
            ec.entity.find("moqui.security.UserGroup").condition("userGroupId", groupId).deleteAll()
        }
        for (const userId of [ownerId, heirId, operatorId, closedId, externalId])
            ec.entity.find("moqui.security.UserAccount").condition("userId", userId).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a shared drive is its owner's to see and moves only with DRIVE_OWNER_CHANGE, to an enabled internal account", {
    when() {
        const F = "org.moqui.impl.FileServices."
        const C = "org.moqui.admin.IdentityServices.call#OwnedResourceService"
        const move = toUserId => invoke(F + "transfer#DriveOwner", {driveId: P + "SHARED", fromUserId: ownerId, toUserId})
        return {
            own: as(ownerId, () => invoke(F + "find#OwnedDrives", {userId: ownerId})),
            peek: as(heirId, () => invoke(F + "find#OwnedDrives", {userId: ownerId})),
            take: as(heirId, () => move(heirId)),
            closed: as(operatorId, () => move(closedId)),
            external: as(operatorId, () => move(externalId)),
            personal: as(operatorId, () =>
                invoke(F + "transfer#DriveOwner", {driveId: P + "PERSONAL", fromUserId: ownerId, toUserId: heirId})
            ),
            // a coordinator's call (IdentityServices.call#OwnedResourceService): a service without the contract is never called
            notContract: as(operatorId, () =>
                invoke(C, {serviceName: F + "add#DriveMember", contract: "transfer", stepParameters: {driveId: P + "SHARED"}})
            ),
            moved: as(operatorId, () =>
                invoke(C, {
                    serviceName: F + "transfer#DriveOwner",
                    contract: "transfer",
                    stepParameters: {driveId: P + "SHARED", fromUserId: ownerId, toUserId: heirId}
                })
            ),
            drive: one("moqui.resource.ResourceDrive", {driveId: P + "SHARED"}),
            heirMember: one("moqui.resource.ResourceDriveMember", {driveMemberId: P + "HEIR_MEMBER"}),
            contract:
                implementsContract(F + "find#OwnedDrives", "OwnedResourceLookup") &&
                implementsContract(F + "transfer#DriveOwner", "OwnedResourceTransfer")
        }
    },
    then({own, peek, take, closed, external, personal, notContract, moved, drive, heirMember, contract}) {
        own.error == "" && own.resourceList.length == 1
        own.resourceList[0].resourceId == P + "SHARED" && own.resourceList[0].transferable == true
        peek.error.includes("DRIVE_OWNER_CHANGE") && take.error.includes("DRIVE_OWNER_CHANGE")
        closed.error.includes("not enabled") && external.error.includes("external user")
        personal.error.includes("not a shared drive")
        notContract.error.includes("does not carry")
        moved.error == "" && moved.result.transferred == true
        drive.ownerUserId == heirId
        // the new owner's member row ends: the owner manages
        heirMember.thruDate != null
        contract == true
    }
})

feature("a shared mail account moves to an enabled account with EMAIL_ACCOUNT_OWNER_CHANGE; a personal account stays", {
    when() {
        const E = "org.moqui.impl.EmailClientServices."
        return {
            listed: as(operatorId, () => invoke(E + "find#OwnedEmailAccounts", {userId: ownerId})),
            peek: as(heirId, () => invoke(E + "find#OwnedEmailAccounts", {userId: ownerId})),
            personal: as(operatorId, () =>
                invoke(E + "transfer#EmailAccountOwner", {emailAccountId: P + "MAIL_OWN", fromUserId: ownerId, toUserId: heirId})
            ),
            closed: as(operatorId, () =>
                invoke(E + "transfer#EmailAccountOwner", {
                    emailAccountId: P + "MAIL_SHARED",
                    fromUserId: ownerId,
                    toUserId: closedId
                })
            ),
            moved: as(operatorId, () =>
                invoke(E + "transfer#EmailAccountOwner", {
                    emailAccountId: P + "MAIL_SHARED",
                    fromUserId: ownerId,
                    toUserId: heirId
                })
            ),
            account: one("moqui.basic.email.EmailAccount", {emailAccountId: P + "MAIL_SHARED"}),
            own: one("moqui.basic.email.EmailAccount", {emailAccountId: P + "MAIL_OWN"})
        }
    },
    then({listed, peek, personal, closed, moved, account, own}) {
        listed.error == "" && listed.resourceList.map(row => row.resourceId).join() == P + "MAIL_SHARED"
        peek.error.includes("EMAIL_ACCOUNT_OWNER_CHANGE")
        personal.error.includes("not a shared account") && closed.error.includes("not enabled")
        moved.error == "" && moved.transferred == true
        account.ownerUserId == heirId && own.ownerUserId == ownerId
    }
})
