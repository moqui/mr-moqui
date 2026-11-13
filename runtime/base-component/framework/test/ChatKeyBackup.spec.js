// Chat key trust and backup (org.moqui.chat.ChatKeyServices and ChatKeys.js; framework/entity/ChatKeyEntities.xml;
// plans/Comms-Framework.md §4.5, C7; docs/interfaces/Chat-Keys.md): a device's store key, answered only to its own user,
// stable, absent from the device list and cleared by a revoke; cross-signing keys uploaded once, the same again a no-op,
// a different master key refused unless reset; signatures by the caller's keys over what those keys may sign, merged into
// key queries; key backup versions, the current version rule and the better-session rule; device_lists.left when the only
// shared room ends. Each refusal uses the very same parameters a positive case passes, by another user. No crypto runs
// here: keys and signatures are opaque strings, as the server treats them.
// Run: ./moq test framework --spec ChatKeyBackup
const A = "CHATKEYB_SPEC_A"
const B = "CHATKEYB_SPEC_B"
const C = "CHATKEYB_SPEC_C"
const USERS = [A, B, C]
const SERVICES = "org.moqui.chat.ChatKeyServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function username(userId) {
    return userId.toLowerCase().replace(/_/g, ".")
}
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out =
            sys(() =>
                ec.service
                    .sync()
                    .name(name.includes(".") ? name : SERVICES + name)
                    .parameters(parameters || {})
                    .call()
            ) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields(
                {userId, username: username(userId), userFullName: "Trust " + userId.slice(-1), disabled: "N"},
                true,
                null,
                null
            )
            account.create()
        }
        // a password, so a spec can sign in for real: a reset, a new or deleted backup and a revoke need a recent sign-in
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        account.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "",
            passwordBase64: "N", successiveFailedLogins: 0, requirePasswordChange: "N"}, true, null, null)
        account.update()
    })
}
const PASSWORD = "Trust-spec-9!"
/** as, signed in with the password just now (a recent sign-in, check#RecentAuth), not by an internal login. */
function asFresh(userId, name, parameters) {
    ec.user.loginUser(username(userId), PASSWORD)
    return as(null, name, parameters)
}
const mx = userId => as(null, "map#ChatKeyUserIds", {userIdList: [userId]}).matrixIds[userId]
function deviceKeys(userId, deviceId, tag) {
    return {
        user_id: mx(userId),
        device_id: deviceId,
        algorithms: ["m.olm.v1.curve25519-aes-sha2"],
        keys: {["curve25519:" + deviceId]: "curve-" + tag, ["ed25519:" + deviceId]: "ed-" + tag},
        signatures: {[mx(userId)]: {["ed25519:" + deviceId]: "sig-" + tag}}
    }
}
/** A new device of userId with its keys; its deviceId. */
function device(userId, tag) {
    const made = as(userId, "create#ChatDevice", {displayName: "Trust " + tag})
    as(userId, "upload#Keys", {deviceId: made.deviceId, device_keys: deviceKeys(userId, made.deviceId, tag)})
    return made.deviceId
}
/** A cross-signing key object of usage with public key pub, signed by signerPub (the master) when given. */
function crossKey(userId, usage, pub, signerPub) {
    const key = {user_id: mx(userId), usage: [usage], keys: {["ed25519:" + pub]: pub}, signatures: {}}
    if (signerPub) key.signatures[mx(userId)] = {["ed25519:" + signerPub]: "sig-by-" + signerPub}
    return key
}
function crossKeys(userId, tag) {
    const master = "mst" + tag
    return {
        master_key: crossKey(userId, "master", master),
        self_signing_key: crossKey(userId, "self_signing", "ssk" + tag, master),
        user_signing_key: crossKey(userId, "user_signing", "usk" + tag, master)
    }
}
/** The signatures one user's signer key made over target, as the signed object query#Keys answered. */
function sigs(object, userId) {
    return Object.keys(((object || {}).signatures || {})[mx(userId)] || {})
        .sort()
        .join()
}
const dev = {}

spec({
    setupSpec() {
        for (const u of USERS) makeUser(u)
        sys(() => {
            for (const entity of [
                "moqui.chat.ChatToDevice",
                "moqui.chat.ChatDeviceKey",
                "moqui.chat.ChatDevice",
                "moqui.chat.ChatCrossSigningKey",
                "moqui.chat.ChatKeyBackupSession",
                "moqui.chat.ChatKeyBackup"
            ]) {
                ec.entity.find(entity).condition("userId", "in", USERS).deleteAll()
            }
            ec.entity.find("moqui.chat.ChatKeySignature").condition("targetUserId", "in", USERS).deleteAll()
            ec.entity.find("moqui.chat.ChatKeySignature").condition("signerUserId", "in", USERS).deleteAll()
            // these users share no room at the start, also after an earlier run that stopped half way
            for (const member of ec.entity
                .find("moqui.chat.ChatRoomMember")
                .condition("userId", "in", USERS)
                .condition("leftDate", null)
                .list()) {
                member.set("leftDate", ec.user.nowTimestamp)
                member.update()
            }
        })
        // A and B share a room; C shares nothing
        ec.user.internalLoginUser(username(A))
        sys(() =>
            call("org.moqui.chat.ChatServices.create#Room", {
                roomTypeEnumId: "CrtPrivate",
                roomName: "Trust spec",
                memberUserIdList: [B]
            })
        )
        dev.A1 = device(A, "a1")
        dev.B1 = device(B, "b1")
        dev.C1 = device(C, "c1")
    }
})

feature(
    "a device's store key is derived, not stored: the same at each ask, only for its own user, absent from the device list and every table, and another after a revoke",
    {
        when() {
            const first = as(A, "get#ChatDeviceStoreKey", {deviceId: dev.A1})
            const again = as(A, "get#ChatDeviceStoreKey", {deviceId: dev.A1})
            const other = as(B, "get#ChatDeviceStoreKey", {deviceId: dev.A1})
            const listed = JSON.stringify(as(A, "find#ChatDevices", {includeRevoked: true}))
            const row = sys(() => ec.entity.find("moqui.chat.ChatDevice").condition({userId: A, deviceId: dev.A1}).one())
            const rowText = JSON.stringify(row.getMap ? row.getMap() : row)
            const doomed = device(A, "a-doomed")
            const doomedKey = as(A, "get#ChatDeviceStoreKey", {deviceId: doomed})
            const revoked = asFresh(A, "revoke#ChatDevice", {deviceId: doomed})
            const afterRevoke = as(A, "get#ChatDeviceStoreKey", {deviceId: doomed})
            const doomedRow = sys(() => ec.entity.find("moqui.chat.ChatDevice").condition({userId: A, deviceId: doomed}).one())
            const rekeyed = as(null, "derive#ChatStoreKey", {userId: A, deviceId: doomed, revision: Number(doomedRow.storeRevision)})
            return {first, again, other, listed, row, rowText, doomedKey, revoked, afterRevoke, doomedRow, rekeyed}
        },
        then({first, again, other, listed, row, rowText, doomedKey, revoked, afterRevoke, doomedRow, rekeyed}) {
            first.errors == ""
            first.storeKey.length == 43
            again.storeKey == first.storeKey
            other.reasonCode == "notFound"
            other.storeKey == null
            listed.indexOf(first.storeKey) < 0
            listed.indexOf("storeKey") < 0
            rowText.indexOf(first.storeKey) < 0
            doomedKey.storeKey.length == 43
            doomedKey.storeKey != first.storeKey
            revoked.errors == ""
            afterRevoke.reasonCode == "revoked"
            Number(doomedRow.storeRevision) == 2
            rekeyed.storeKey != doomedKey.storeKey
        }
    }
)

feature("cross-signing keys upload once, the same again is a no-op, and a different master key needs reset", {
    when() {
        const params = crossKeys(A, "A1")
        const revision = () =>
            sys(() => Number(ec.entity.find("moqui.chat.ChatDevice").condition({userId: A, deviceId: dev.A1}).one().listRevision))
        const revisionBefore = revision()
        const first = as(A, "upload#CrossSigningKeys", params)
        const revisionFirst = revision()
        const other = as(B, "upload#CrossSigningKeys", params)
        const same = as(A, "upload#CrossSigningKeys", params)
        const revisionSame = revision()
        const changed = as(A, "upload#CrossSigningKeys", crossKeys(A, "A2"))
        const noMaster = as(C, "upload#CrossSigningKeys", {self_signing_key: crossKey(C, "self_signing", "sskC", "mstC")})
        const unsigned = as(C, "upload#CrossSigningKeys", {
            master_key: crossKey(C, "master", "mstC"),
            self_signing_key: crossKey(C, "self_signing", "sskC")
        })
        const wrongUsage = as(C, "upload#CrossSigningKeys", {master_key: crossKey(C, "self_signing", "mstC")})
        const kept = sys(
            () => ec.entity.find("moqui.chat.ChatCrossSigningKey").condition({userId: A, keyUsage: "master"}).one().publicKey
        )
        return {first, other, same, revisionBefore, revisionFirst, revisionSame, changed, noMaster, unsigned, wrongUsage, kept}
    },
    then({first, other, same, revisionBefore, revisionFirst, revisionSame, changed, noMaster, unsigned, wrongUsage, kept}) {
        first.errors == ""
        first.changed == true
        other.reasonCode == "badKeys"
        same.errors == ""
        same.changed == false
        // a change bumps the user's device list (syncs of users in reach name them); the same keys again do not
        revisionFirst == revisionBefore + 1
        revisionSame == revisionFirst
        changed.reasonCode == "keysChanged"
        noMaster.reasonCode == "noMasterKey"
        unsigned.reasonCode == "badKeys"
        wrongUsage.reasonCode == "badKeys"
        kept == "mstA1"
    }
})

feature(
    "signatures by the caller's keys are stored where they may sign and merged into key queries; others are refused per key",
    {
        when() {
            as(B, "upload#CrossSigningKeys", crossKeys(B, "B1"))
            as(C, "upload#CrossSigningKeys", crossKeys(C, "C1"))
            const aDevice = Object.assign(deviceKeys(A, dev.A1, "a1"), {signatures: {[mx(A)]: {"ed25519:sskA1": "ssk-over-a1"}}})
            const bMaster = Object.assign(crossKey(B, "master", "mstB1"), {
                signatures: {[mx(A)]: {"ed25519:uskA1": "usk-over-b"}}
            })
            const aMaster = Object.assign(crossKey(A, "master", "mstA1"), {
                signatures: {[mx(A)]: {["ed25519:" + dev.A1]: "a1-over-master"}}
            })
            const signed = {[mx(A)]: {[dev.A1]: aDevice, mstA1: aMaster}, [mx(B)]: {mstB1: bMaster}}
            const own = as(A, "upload#KeySignatures", {signed})
            // the very same body by B: none of it is signed by B's keys, and A's keys are not B's to sign
            const other = as(B, "upload#KeySignatures", {signed})
            const outOfReach = as(A, "upload#KeySignatures", {
                signed: {
                    [mx(C)]: {
                        mstC1: Object.assign(crossKey(C, "master", "mstC1"), {
                            signatures: {[mx(A)]: {"ed25519:uskA1": "usk-over-c"}}
                        })
                    }
                }
            })
            const wrongSigner = as(A, "upload#KeySignatures", {
                signed: {
                    [mx(A)]: {
                        [dev.A1]: Object.assign(deviceKeys(A, dev.A1, "a1"), {
                            signatures: {[mx(A)]: {"ed25519:uskA1": "usk-over-own-device"}}
                        })
                    }
                }
            })
            const aSeesB = as(A, "query#Keys", {device_keys: {[mx(B)]: []}})
            const bSeesA = as(B, "query#Keys", {device_keys: {[mx(A)]: []}})
            const bSeesB = as(B, "query#Keys", {device_keys: {[mx(B)]: []}})
            const aSeesA = as(A, "query#Keys", {device_keys: {[mx(A)]: []}})
            const aSeesC = as(A, "query#Keys", {device_keys: {[mx(C)]: []}})
            // a reset of A's identity deletes every signature by or over its old keys
            // a reset needs a sign-in in the last five minutes: after a logout and an internal login it is refused, with the
            // same parameters, and taken once the user signed in with the password
            ec.user.logoutUser()
            const resetRefused = as(A, "upload#CrossSigningKeys", Object.assign(crossKeys(A, "A3"), {reset: true}))
            const reset = asFresh(A, "upload#CrossSigningKeys", Object.assign(crossKeys(A, "A3"), {reset: true}))
            const bSeesAAfter = as(B, "query#Keys", {device_keys: {[mx(A)]: []}})
            const leftOver = sys(() => ec.entity.find("moqui.chat.ChatKeySignature").condition("signerUserId", A).count())
            return {own, other, outOfReach, wrongSigner, aSeesB, bSeesA, bSeesB, aSeesA, aSeesC, resetRefused, reset, bSeesAAfter, leftOver}
        },
        then({own, other, outOfReach, wrongSigner, aSeesB, bSeesA, bSeesB, aSeesA, aSeesC, resetRefused, reset, bSeesAAfter, leftOver}) {
            own.errors == ""
            Object.keys(own.failures).length == 0
            other.errors == ""
            other.failures[mx(A)][dev.A1].errcode == "M_FORBIDDEN"
            other.failures[mx(A)].mstA1.errcode == "M_INVALID_SIGNATURE"
            other.failures[mx(B)].mstB1.errcode == "M_INVALID_SIGNATURE"
            outOfReach.failures[mx(C)].mstC1.errcode == "M_FORBIDDEN"
            wrongSigner.failures[mx(A)][dev.A1].errcode == "M_INVALID_SIGNATURE"
            // A sees its own signature over B's master; B's self-signing key is there, B's user-signing key is not
            sigs(aSeesB.master_keys[mx(B)], A) == "ed25519:uskA1"
            aSeesB.self_signing_keys[mx(B)].keys["ed25519:sskB1"] == "sskB1"
            aSeesB.user_signing_keys[mx(B)] == null
            // B sees A's device signed by A's self-signing key, and A's master by A's device, but not A's user-signing key
            sigs(bSeesA.device_keys[mx(A)][dev.A1], A) == ["ed25519:" + dev.A1, "ed25519:sskA1"].sort().join()
            sigs(bSeesA.master_keys[mx(A)], A) == "ed25519:" + dev.A1
            bSeesA.user_signing_keys[mx(A)] == null
            // A's signature over B's master is A's own business
            sigs(bSeesB.master_keys[mx(B)], A) == ""
            bSeesB.user_signing_keys[mx(B)].keys["ed25519:uskB1"] == "uskB1"
            aSeesA.user_signing_keys[mx(A)].keys["ed25519:uskA1"] == "uskA1"
            aSeesC.master_keys[mx(C)] == null
            resetRefused.reasonCode == "reauthRequired"
            reset.changed == true
            bSeesAAfter.master_keys[mx(A)].keys["ed25519:mstA3"] == "mstA3"
            sigs(bSeesAAfter.device_keys[mx(A)][dev.A1], A) == "ed25519:" + dev.A1
            leftOver == 0
        }
    }
)

feature(
    "key backup versions: create, get the current, update, put only to the current version, the better session wins, get and delete",
    {
        when() {
            const session = (fmi, fc, verified, tag) => ({
                first_message_index: fmi,
                forwarded_count: fc,
                is_verified: verified,
                session_data: {ephemeral: "eph-" + tag, ciphertext: "ct-" + tag, mac: "mac-" + tag}
            })
            const put = (version, roomId, sessionId, data) =>
                as(A, "store#BackupKeys", {version, rooms: {[roomId]: {sessions: {[sessionId]: data}}}})
            const none = as(A, "get#KeyBackupVersion", {})
            // a new backup needs a sign-in in the last five minutes: refused after an internal login, with the same parameters
            ec.user.logoutUser()
            const v1Refused = as(A, "create#KeyBackupVersion", {algorithm: "m.megolm_backup.v1.curve25519-aes-sha2", auth_data: {public_key: "pk1", signatures: {}}})
            const v1 = asFresh(A, "create#KeyBackupVersion", {
                algorithm: "m.megolm_backup.v1.curve25519-aes-sha2",
                auth_data: {public_key: "pk1", signatures: {}}
            })
            const v2 = as(A, "create#KeyBackupVersion", {
                algorithm: "m.megolm_backup.v1.curve25519-aes-sha2",
                auth_data: {public_key: "pk2", signatures: {}}
            })
            const current = as(A, "get#KeyBackupVersion", {})
            const firstPut = {
                version: v2.version,
                rooms: {
                    "!r1:localhost": {sessions: {s1: session(5, 1, false, "first"), s2: session(0, 0, false, "two")}},
                    "!r2:localhost": {sessions: {s3: session(0, 0, true, "three")}}
                }
            }
            const stored = as(A, "store#BackupKeys", firstPut)
            const otherUser = as(B, "store#BackupKeys", firstPut)
            const wrong = put(v1.version, "!r1:localhost", "s1", session(0, 0, true, "wrong"))
            const worse = put(v2.version, "!r1:localhost", "s1", session(7, 0, false, "worse"))
            const lower = put(v2.version, "!r1:localhost", "s1", session(3, 2, false, "lower"))
            const verified = put(v2.version, "!r1:localhost", "s1", session(9, 9, true, "verified"))
            const unverified = put(v2.version, "!r1:localhost", "s1", session(0, 0, false, "unverified"))
            const fewerForwards = put(v2.version, "!r1:localhost", "s1", session(9, 4, true, "fewer"))
            const tooLarge = put(
                v2.version,
                "!r1:localhost",
                "s9",
                Object.assign(session(0, 0, false, "big"), {session_data: {ciphertext: "x".repeat(17000)}})
            )
            const all = as(A, "get#BackupKeys", {version: v2.version})
            const room = as(A, "get#BackupKeys", {version: v2.version, roomId: "!r1:localhost"})
            const one = as(A, "get#BackupKeys", {version: v2.version, roomId: "!r1:localhost", sessionId: "s1"})
            const otherGet = as(B, "get#BackupKeys", {version: v2.version, roomId: "!r1:localhost", sessionId: "s1"})
            const counted = as(A, "get#KeyBackupVersion", {version: v2.version})
            const updated = as(A, "update#KeyBackupVersion", {
                version: v2.version,
                algorithm: "m.megolm_backup.v1.curve25519-aes-sha2",
                auth_data: {public_key: "pk2", signatures: {x: {}}}
            })
            const otherUpdate = as(B, "update#KeyBackupVersion", {
                version: v2.version,
                algorithm: "m.megolm_backup.v1.curve25519-aes-sha2",
                auth_data: {public_key: "pk2", signatures: {x: {}}}
            })
            const algorithm = as(A, "update#KeyBackupVersion", {
                version: v2.version,
                algorithm: "other",
                auth_data: {public_key: "pk2"}
            })
            const afterUpdate = as(A, "get#KeyBackupVersion", {})
            // another public key sends the backup to another key, as a new version does: it needs a recent sign-in (A's
            // went with the account switch to B above); new signatures do not
            const newKey = {version: v2.version, algorithm: "m.megolm_backup.v1.curve25519-aes-sha2", auth_data: {public_key: "pk2b"}}
            const keyChangeRefused = as(A, "update#KeyBackupVersion", newKey)
            const keyChanged = asFresh(A, "update#KeyBackupVersion", newKey)
            const keyBack = as(A, "update#KeyBackupVersion", {
                version: v2.version,
                algorithm: "m.megolm_backup.v1.curve25519-aes-sha2",
                auth_data: {public_key: "pk2", signatures: {x: {}}}
            })
            // deleting backed-up keys needs a recent sign-in too, refused with the same parameters after a switch
            as(B, "get#KeyBackupVersion", {})
            const deleteKeysRefused = as(A, "delete#BackupKeys", {version: v2.version, roomId: "!r1:localhost", sessionId: "s2"})
            const deletedOne = asFresh(A, "delete#BackupKeys", {version: v2.version, roomId: "!r1:localhost", sessionId: "s2"})
            const deletedNone = as(A, "delete#BackupKeys", {version: v2.version, roomId: "!r1:localhost", sessionId: "s2"})
            const otherDelete = as(B, "delete#KeyBackupVersion", {version: v2.version})
            const deletedVersion = asFresh(A, "delete#KeyBackupVersion", {version: v2.version})
            const afterDelete = as(A, "get#KeyBackupVersion", {})
            const gone = as(A, "get#KeyBackupVersion", {version: v2.version})
            const goneSessions = sys(() =>
                ec.entity.find("moqui.chat.ChatKeyBackupSession").condition({userId: A, version: v2.version}).count()
            )
            const v3 = as(A, "create#KeyBackupVersion", {
                algorithm: "m.megolm_backup.v1.curve25519-aes-sha2",
                auth_data: {public_key: "pk3"}
            })
            // the versions left, newest first, with the public key a recovery key is matched to (an older one restores too)
            const versions = as(A, "find#KeyBackupVersions", {}).versionList.map((v) => v.version + ":" + v.publicKey + ":" + v.current).join(",")
            return {
                none,
                v1,
                v2,
                current,
                stored,
                otherUser,
                wrong,
                worse,
                lower,
                verified,
                unverified,
                fewerForwards,
                tooLarge,
                all,
                room,
                one,
                otherGet,
                counted,
                updated,
                otherUpdate,
                algorithm,
                afterUpdate,
                keyChangeRefused,
                keyChanged,
                keyBack,
                deleteKeysRefused,
                deletedOne,
                deletedNone,
                otherDelete,
                deletedVersion,
                afterDelete,
                gone,
                goneSessions,
                v3,
                v1Refused,
                versions
            }
        },
        then({
            none,
            v1,
            v2,
            current,
            stored,
            otherUser,
            wrong,
            worse,
            lower,
            verified,
            unverified,
            fewerForwards,
            tooLarge,
            all,
            room,
            one,
            otherGet,
            counted,
            updated,
            otherUpdate,
            algorithm,
            afterUpdate,
            keyChangeRefused,
            keyChanged,
            keyBack,
            deleteKeysRefused,
            deletedOne,
            deletedNone,
            otherDelete,
            deletedVersion,
            afterDelete,
            gone,
            goneSessions,
            v3,
            v1Refused,
            versions
        }) {
            none.reasonCode == "notFound"
            v1Refused.reasonCode == "reauthRequired"
            versions == "3:pk3:true,1:pk1:false"
            v1.version == "1"
            v2.version == "2"
            current.version == "2"
            current.count == 0
            current.etag == "0"
            current.auth_data.public_key == "pk2"
            current.algorithm == "m.megolm_backup.v1.curve25519-aes-sha2"
            stored.errors == ""
            stored.count == 3
            stored.etag == "1"
            otherUser.reasonCode == "notFound"
            wrong.reasonCode == "wrongVersion"
            wrong.current_version == "2"
            worse.etag == "1"
            lower.etag == "2"
            verified.etag == "3"
            unverified.etag == "3"
            fewerForwards.etag == "4"
            fewerForwards.count == 3
            tooLarge.reasonCode == "tooLarge"
            Object.keys(all.rooms).sort().join() == "!r1:localhost,!r2:localhost"
            all.rooms["!r2:localhost"].sessions.s3.is_verified == true
            all.has_more == false
            Object.keys(room.rooms).join() == "!r1:localhost"
            Object.keys(room.rooms["!r1:localhost"].sessions).sort().join() == "s1,s2"
            one.rooms["!r1:localhost"].sessions.s1.session_data.ciphertext == "ct-fewer"
            one.rooms["!r1:localhost"].sessions.s1.forwarded_count == 4
            one.rooms["!r1:localhost"].sessions.s1.first_message_index == 9
            Object.keys(one.rooms["!r1:localhost"].sessions).join() == "s1"
            otherGet.reasonCode == "notFound"
            counted.count == 3
            counted.etag == "4"
            updated.errors == ""
            otherUpdate.reasonCode == "notFound"
            algorithm.reasonCode == "algorithmChanged"
            afterUpdate.auth_data.signatures.x != null
            afterUpdate.etag == "4"
            keyChangeRefused.reasonCode == "reauthRequired"
            keyChanged.errors == ""
            keyBack.errors == ""
            deleteKeysRefused.reasonCode == "reauthRequired"
            deletedOne.count == 2
            deletedOne.etag == "5"
            deletedNone.etag == "5"
            otherDelete.reasonCode == "notFound"
            deletedVersion.errors == ""
            afterDelete.version == "1"
            gone.reasonCode == "notFound"
            goneSessions == 0
            v3.version == "3"
        }
    }
)

feature("device_lists.left names a co-member once the only shared room ends, on both sides, and not one still in another room", {
    when() {
        ec.user.internalLoginUser(username(A))
        const shared = sys(() =>
            call("org.moqui.chat.ChatServices.create#Room", {
                roomTypeEnumId: "CrtPrivate",
                roomName: "Trust spec left",
                memberUserIdList: [B, C]
            })
        ).chatRoomId
        const before = Date.now() - 1000
        const aSeesC = as(A, "query#Keys", {device_keys: {[mx(C)]: []}})
        sys(() => {
            for (const userId of [C, B]) {
                const member = ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: shared, userId}).one()
                member.set("leftDate", ec.user.nowTimestamp)
                member.update()
            }
        })
        const aSync = as(A, "sync#ToDevice", {deviceId: dev.A1, since: 0, deviceListSince: before})
        const cSync = as(C, "sync#ToDevice", {deviceId: dev.C1, since: 0, deviceListSince: before})
        const bSync = as(B, "sync#ToDevice", {deviceId: dev.B1, since: 0, deviceListSince: before})
        const aLater = as(A, "sync#ToDevice", {deviceId: dev.A1, since: 0, deviceListSince: Date.now() + 600000})
        return {aSeesC, aSync, cSync, bSync, aLater}
    },
    then({aSeesC, aSync, cSync, bSync, aLater}) {
        Object.keys(aSeesC.device_keys[mx(C)]).join() == dev.C1
        aSync.device_lists.left.includes(mx(C))
        // B left this room but still shares the first one with A
        !aSync.device_lists.left.includes(mx(B))
        cSync.device_lists.left.includes(mx(A))
        cSync.device_lists.left.includes(mx(B))
        !bSync.device_lists.left.includes(mx(A))
        bSync.device_lists.left.includes(mx(C))
        aLater.device_lists.left.length == 0
    }
})
