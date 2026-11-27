// A location share's guest link (plans/Location-Sharing.md §3.7, LL6; org.moqui.maps.LocationServices check#LinkKey,
// get#LinkShare; tools Maps/SharedLocation.xml), read with no sign-in and authorization on: the link's token is its
// selector and its secret, the page's link /location/<selector>#<secret> with the secret in the fragment only, so the
// request line a log records holds none; a read answers the share's label, purpose and latest position (what was received
// after the link was added, an approximate share's cell) with the public basemap or the outline, and nothing else; a
// wrong secret is counted on the link and refused, and so is the link after location@link-max-failed-tries of them;
// another share's secret never reads this share; reads past location@link-max-views-per-minute a minute are refused;
// an ended share's link is refused; and the page's read transition takes the secret from its body.
// Run: ./moq test framework --spec LocationShareLinks
const S = "org.moqui.maps.LocationServices."
const A = "LOC_LINK_A"
const B = "LOC_LINK_B"
const MINUTE = 60000
const HOUR = 3600000
const shared = {}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = userId => userId.toLowerCase().replace(/_/g, ".")
// a call with authorization on, as the user or, with no user, as an anonymous visitor: its results and its errors as text
function as(userId, verbNoun, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    else ec.user.logoutUser()
    ec.artifactExecution.enableAuthz()
    let out = {}
    let thrown = ""
    try {
        out = ec.service.sync().name(S + verbNoun).parameters(parameters || {}).call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const at = millis => ec.user.setEffectiveTime(millis)
const later = millis => {
    shared.now += millis
    at(shared.now)
    return shared.now
}
const toLink = () => ({audienceTypeEnumId: "LsaLink"})
// the link's selector and secret as the page has them: its path's last segment and its fragment
function partsOf(linkPath) {
    const [path, secret] = String(linkPath || "").split("#")
    return {selector: path.split("/").pop(), secret, path}
}
const guestRead = (selector, secret) => as(null, "get#LinkShare", {linkKey: selector, linkSecret: secret})
const linkRow = shareId =>
    sys(() => ec.entity.find("moqui.maps.LocationShareAudience").condition({shareId, audienceTypeEnumId: "LsaLink"}).one())
const report = (shareId, latitude, longitude) =>
    as(A, "report#Position", {shareId, latitude, longitude, accuracy: "15", observedDate: new Date(shared.now)})

spec({
    stepwise: true,
    setupSpec() {
        shared.now = Math.floor(Date.now() / 1000) * 1000
        at(shared.now)
        sys(() => {
            for (const userId of [A, B])
                if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0)
                    ec.entity
                        .makeValue("moqui.security.UserAccount")
                        .setAll({userId, username: username(userId), userFullName: "Link " + userId.slice(9), disabled: "N"})
                        .create()
        })
    },
    cleanupSpec() {
        for (const share of Array.from(as(A, "find#MyShares").shareList || [])) as(A, "stop#Share", {shareId: share.shareId})
        ec.user.logoutUser()
    }
})

feature("a guest reads the link's share with its secret, only what the server received after the link was added, and nothing else", {
    when() {
        // a share with B first, reported to, then the link added: the earlier position is not the guest's
        const made = as(A, "create#Share", {
            audienceList: [{audienceTypeEnumId: "LsaUser", userId: B}],
            thruDate: new Date(shared.now + 2 * HOUR),
            label: "Van 7",
            purposeText: "About 15:30"
        })
        shared.shareId = made.shareId
        const early = report(made.shareId, "40.7452", "-111.8750")
        later(10 * 1000)
        const linked = as(A, "update#Share", {shareId: made.shareId, addAudienceList: [toLink()]})
        shared.link = partsOf(linked.linkPath)
        const beforeAny = guestRead(shared.link.selector, shared.link.secret)
        later(10 * 1000)
        const reported = report(made.shareId, "40.7460", "-111.8760")
        later(5 * 1000)
        const read = guestRead(shared.link.selector, shared.link.secret)
        const row = linkRow(made.shareId)
        // the route's check: the selector alone, answering nothing of the share; another selector is none
        const routed = as(null, "check#LinkKey", {token: shared.link.selector, routeName: "location"})
        const unknown = as(null, "check#LinkKey", {token: "noSuchSelector00", routeName: "location"})
        return {early, linked, beforeAny, reported, read, row, routed, unknown}
    },
    then({early, linked, beforeAny, reported, read, row, routed, unknown}) {
        early.statusCode == 200 && reported.statusCode == 200
        // the page's link: its path holds the selector, the secret is in the fragment alone, and the token is both
        /^\/location\/[A-Za-z0-9_-]{16}#[A-Za-z0-9_-]{32}$/.test(linked.linkPath)
        !partsOf(linked.linkPath).path.includes(partsOf(linked.linkPath).secret)
        linked.linkToken == shared.link.selector + "." + shared.link.secret
        row.tokenSelector == shared.link.selector
        row.tokenDigest == ec.user.singleUseCodeHash(linked.linkToken)
        // nothing received before the link was added
        beforeAny.statusCode == 200 && beforeAny.latest == null
        read.statusCode == 200
        read.share.label == "Van 7" && read.share.purposeText == "About 15:30" && read.share.approximate == false
        Math.abs(read.latest.latitude - 40.746) < 1e-9 && Math.abs(read.latest.longitude + 111.876) < 1e-9
        read.latest.ageSeconds == 5
        // only the label, the purpose, the position and the map
        Object.keys(read.share).sort().join() == "approximate,label,purposeText"
        Object.keys(read.latest).sort().join() == "accuracy,ageSeconds,latitude,longitude"
        // no basemap is marked public here: the country outline
        read.map.basemap == "OUTLINE_NE110" && read.map.basemaps.length == 1
        !JSON.stringify(read).includes(A) && !JSON.stringify(read).includes(B) && !JSON.stringify(read).includes(shared.shareId)
        row.lastViewedDate != null && row.viewMinuteCount == 2
        routed.valid == true && Object.keys(routed.routeContext || {}).length == 0
        unknown.valid == false
        read.errors == ""
    }
})

feature("a wrong secret is counted on the link and refused, another share's secret reads nothing of it, and the link is refused after too many", {
    when() {
        const other = as(A, "create#Share", {audienceList: [toLink()], thruDate: new Date(shared.now + HOUR), label: "Other van"})
        const otherLink = partsOf(other.linkPath)
        report(other.shareId, "10.5", "20.5")
        shared.otherShareId = other.shareId
        shared.otherLink = otherLink
        const wrong = guestRead(shared.link.selector, "x".repeat(32))
        // the other share's secret with this link's selector, and this link's secret with the other's selector
        const crossed = guestRead(shared.link.selector, otherLink.secret)
        const crossedBack = guestRead(otherLink.selector, shared.link.secret)
        const counted = Number(linkRow(shared.shareId).failedTries)
        const stillRead = guestRead(shared.link.selector, shared.link.secret)
        // the other share's own read still works and answers its own share only
        const otherRead = guestRead(otherLink.selector, otherLink.secret)
        // up to location@link-max-failed-tries (3 here): the link refused then, its right secret too
        const locked = withConf("location@link-max-failed-tries", "3", () => {
            const third = guestRead(shared.link.selector, "y".repeat(32))
            const right = guestRead(shared.link.selector, shared.link.secret)
            return {third, right}
        })
        return {wrong, crossed, crossedBack, counted, stillRead, otherRead, locked}
    },
    then({wrong, crossed, crossedBack, counted, stillRead, otherRead, locked}) {
        wrong.statusCode == 404 && wrong.reasonCode == "LINK_NOT_FOUND" && wrong.share == null && wrong.latest == null
        crossed.statusCode == 404 && crossed.share == null
        crossedBack.statusCode == 404 && crossedBack.share == null
        counted == 2
        stillRead.statusCode == 200 && stillRead.share.label == "Van 7"
        otherRead.statusCode == 200 && otherRead.share.label == "Other van"
        !JSON.stringify(otherRead).includes("Van 7") && Math.abs(otherRead.latest.latitude - 10.5) < 1e-9
        locked.third.statusCode == 403 && locked.third.reasonCode == "LINK_LOCKED"
        locked.right.statusCode == 403 && locked.right.share == null
        // a refusal is a status and a reason: no error message, so the count it wrote is kept, and no value named
        wrong.errors == "" && locked.right.errors == ""
    }
})

feature("reads past location@link-max-views-per-minute in a minute are refused until the minute is over", {
    when() {
        const link = shared.otherLink
        // a new minute for the link
        later(MINUTE)
        return withConf("location@link-max-views-per-minute", "2", () => {
            const answers = []
            for (let i = 0; i < 3; i++) answers.push(guestRead(link.selector, link.secret).statusCode)
            const limited = guestRead(link.selector, link.secret)
            later(MINUTE)
            const after = guestRead(link.selector, link.secret)
            return {answers, limited, after}
        })
    },
    then({answers, limited, after}) {
        answers.join() == "200,200,429"
        limited.statusCode == 429 && limited.reasonCode == "RATE_LIMITED" && limited.retryAfterSeconds >= 1 && limited.share == null
        after.statusCode == 200
    }
})

feature("the guest page's read takes the secret in its body only, and an ended share's link is refused", {
    when() {
        const made = as(A, "create#Share", {audienceList: [toLink()], thruDate: new Date(shared.now + HOUR), label: "Truck 2"})
        const link = partsOf(made.linkPath)
        report(made.shareId, "45.1", "-122.6")
        later(5 * 1000)
        // the one render of the page's read transition (the qvue page itself has no logic the server runs)
        ec.user.logoutUser()
        const page = screenTest().rootScreen("component://tools/screen/Maps/SharedLocation.xml")
        const fromBody = page.render("read", {token: link.selector, linkSecret: link.secret}, "post").jsonObject
        as(A, "stop#Share", {shareId: made.shareId})
        const ended = guestRead(link.selector, link.secret)
        const routed = as(null, "check#LinkKey", {token: link.selector})
        return {fromBody, link, ended, routed}
    },
    then({fromBody, link, ended, routed}) {
        fromBody.statusCode == 200 && fromBody.share.label == "Truck 2" && Math.abs(fromBody.latest.latitude - 45.1) < 1e-9
        // the answer names neither part of the token
        !JSON.stringify(fromBody).includes(link.secret) && !JSON.stringify(fromBody).includes(link.selector)
        ended.statusCode == 410 && ended.reasonCode == "SHARE_ENDED" && ended.share == null && ended.latest == null
        // the route still takes the selector, so the page can say the share has ended
        routed.valid == true
    }
})
