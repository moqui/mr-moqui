// The notification topics part of org.moqui.admin.OperationServices (mr-boss plan/design-admin.md §10 Topics and TopicDetail;
// impl-admin P14): the registered notification topics (moqui.security.user.NotificationTopic) with their delivery defaults,
// and the subscriptions of a topic's registered audience (NotificationTopicUser).
//
// What is read. A topic answers its name and description, whether it is private, its inbox, whether it asks for a decision,
// its delivery defaults and how many users have a setting for it and receive every message of it. Never a title, error
// title or link template (each is expanded as a script when a notification is sent), an approval service or an email
// template. A subscription is answered by name only for a user of the topic's registered audience; the others are counted.
//
// What is changed. Nothing without a registration (moqui.server.TopicRegistration, its owner's seed row). store#TopicDefaults
// (ADMIN_SETTINGS_MANAGE) writes only the delivery settings the registration names; whether a topic is private, its
// templates, its approval services and its inbox are never written, and a topic that asks for a decision always keeps its
// notifications. store#TopicSubscription (ADMIN_OPERATION_MANAGE) sets the in-app, email, push and digest choices of one
// user, and only of a user who signs in and holds the permission the registration names as the topic's audience: so
// nobody is subscribed to a private topic, or to any topic, outside the audience its owner registered, the administrator
// neither. Receiving every message of a topic (allNotifications) is never set here, for any topic: such a user receives
// what was sent to others, a private topic's messages too; here it is only ended. Personal preferences stay each user's own.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const jobs = ec.resource.script("component://framework/service/org/moqui/admin/OperationJobs.js")
const {find, one, rows, map, ms, stamp, text, refused} = lib
const TOPIC = "moqui.security.user.NotificationTopic"
const TOPIC_USER = "moqui.security.user.NotificationTopicUser"
const REGISTRATION = "moqui.server.TopicRegistration"
const ACCOUNT = "moqui.security.UserAccount"
const SETTINGS_VIEW = "ADMIN_SETTINGS_VIEW"
const SETTINGS_MANAGE = "ADMIN_SETTINGS_MANAGE"
// the delivery settings a registration may name, with what each is called
const fieldTitles = {
    description: "Description",
    showAlert: "Shown as an alert",
    alertNoAutoHide: "The alert stays until dismissed",
    persistOnSend: "Kept in the notification list",
    receiveNotifications: "In app, by default",
    emailNotifications: "By email, by default",
    pushNotifications: "By push, by default",
    digestDefault: "In the daily digest, by default"
}
const flagFields = Object.keys(fieldTitles).filter(field => field !== "description")
const subscriptionFields = ["receiveNotifications", "emailNotifications", "pushNotifications", "digest"]
const listLimit = 200

function requireRead() {
    lib.signedIn()
    if (![lib.VIEW, lib.MANAGE, SETTINGS_VIEW, SETTINGS_MANAGE].some(permission => ec.user.hasPermission(permission)))
        throw new Error(lib.VIEW + " or " + SETTINGS_VIEW + " is required")
}
const now = () => ms(ec.user.nowTimestamp)
// A topic's registration, or null: none, disabled, or one naming an unknown field or permission (logged, left out).
function registrationOf(topic) {
    const row = one(REGISTRATION, {topic})
    if (!row || row.disabled === "Y") return null
    const registration = map(row)
    const fields = editableOf(registration)
    const permission = text(registration.subscriptionPermission)
    const problem = text(registration.editableFields)
        .split(",")
        .map(text)
        .filter(Boolean)
        .some(field => !Object.hasOwn(fieldTitles, field))
        ? "names a field that is not a delivery setting"
        : permission && !one("moqui.security.UserPermission", {userPermissionId: permission})
          ? "names the unknown permission " + permission
          : ""
    if (problem) {
        ec.logger.error("Topic registration " + topic + " " + problem + "; left out")
        return null
    }
    return Object.assign(registration, {fields})
}
const editableOf = registration =>
    text(registration?.editableFields)
        .split(",")
        .map(text)
        .filter(field => Object.hasOwn(fieldTitles, field))
// The users who hold a permission now: {everyone} when ALL_USERS holds it, else their ids.
function holdersOf(permission) {
    const nowMs = now()
    const within = (from, thru) => (ms(from) == null || ms(from) <= nowMs) && (ms(thru) == null || ms(thru) > nowMs)
    const ids = new Set()
    let everyone = false
    for (const row of rows("moqui.security.UserPermissionCheck", {userPermissionId: permission})) {
        if (!within(row.groupFromDate, row.groupThruDate) || !within(row.permissionFromDate, row.permissionThruDate)) continue
        if (row.userGroupId === "ALL_USERS") everyone = true
        else ids.add(row.userId)
    }
    if (
        rows("moqui.security.UserGroupPermission", {userGroupId: "ALL_USERS", userPermissionId: permission}).some(row =>
            within(row.fromDate, row.thruDate)
        )
    )
        everyone = true
    return {everyone, has: userId => everyone || ids.has(userId)}
}
const flagText = value => (value === "Y" ? "Yes" : value === "N" ? "No" : "")
// A topic as the list and the detail show it.
function described(topic, registration, subscriptions) {
    const updates = topic.inboxEnumId === "NinUpdates"
    const actionable = !!text(topic.approveServiceName)
    return {
        topic: topic.topic,
        description: text(topic.description) || topic.topic,
        registered: !!registration,
        ownerApp: text(registration?.ownerApp) || null,
        ownerText: registration ? text(registration.ownerApp) || "Registered" : "Not registered",
        purpose: text(registration?.purpose) || null,
        isPrivate: topic.isPrivate === "Y",
        inboxText: updates && !actionable ? "Updates" : "Notifications",
        actionable,
        // as a notification is resolved: with no value a topic's notifications are received
        receiveText: flagText(topic.receiveNotifications) || "Yes (not set)",
        emailText: text(topic.emailTemplateId) ? flagText(topic.emailNotifications) || "No (not set)" : "No email template",
        pushText: flagText(topic.pushNotifications) || (updates ? "No (by its inbox)" : "Yes (by its inbox)"),
        digestText: flagText(topic.digestDefault) || "No (not set)",
        showAlert: topic.showAlert === "Y",
        persistOnSend: topic.persistOnSend === "Y",
        hasEmailTemplate: !!text(topic.emailTemplateId),
        subscriptionCount: subscriptions.length,
        allNotificationsCount: subscriptions.filter(row => row.allNotifications === "Y").length,
        revision: stamp(topic)
    }
}
const subscriptionsOf = topic => rows(TOPIC_USER, {topic}).map(map)

// find#Topics: the notification topics, by description.
function findTopics(p) {
    requireRead()
    const term = text(p.term).toLowerCase()
    const registrations = new Map(
        rows(REGISTRATION)
            .filter(row => row.disabled !== "Y")
            .map(row => [row.topic, map(row)])
    )
    const byTopic = new Map()
    for (const row of Array.from(find(TOPIC_USER).selectField("topic,userId,allNotifications").limit(20000).list())) {
        if (!byTopic.has(row.topic)) byTopic.set(row.topic, [])
        byTopic.get(row.topic).push({allNotifications: row.allNotifications})
    }
    let topics = rows(TOPIC).map(map)
    if (term)
        topics = topics.filter(topic => [topic.topic, topic.description].some(value => text(value).toLowerCase().includes(term)))
    const found = topics
        .map(topic => described(topic, registrations.get(topic.topic), byTopic.get(topic.topic) || []))
        .sort((a, b) => a.description.localeCompare(b.description))
    const paged = lib.page(found, p)
    return {
        topicList: paged.rows,
        topicCount: paged.count,
        pageIndex: paged.pageIndex,
        pageSize: paged.pageSize,
        canManageDefaults: ec.user.hasPermission(SETTINGS_MANAGE),
        canManageSubscriptions: ec.user.hasPermission(lib.MANAGE)
    }
}

// get#Topic: one topic's delivery defaults as they may be changed here, and the subscriptions of its audience.
function getTopic(p) {
    requireRead()
    const name = text(p.topic)
    const row = name ? one(TOPIC, {topic: name}) : null
    if (!row) throw new Error("No notification topic " + name)
    const topic = map(row)
    const registration = registrationOf(name)
    const subscriptions = subscriptionsOf(name)
    const canManageDefaults = ec.user.hasPermission(SETTINGS_MANAGE)
    const canManageSubscriptions = ec.user.hasPermission(lib.MANAGE)
    const audience = text(registration?.subscriptionPermission)
    const holders = audience ? holdersOf(audience) : null
    // the names of subscribers are an operator's to read, and only within the audience
    const namesShown = !!holders && (ec.user.hasPermission(lib.VIEW) || canManageSubscriptions)
    const inAudience = namesShown ? subscriptions.filter(found => holders.has(found.userId)) : []
    const accounts = new Map(
        inAudience.length
            ? Array.from(
                  find(ACCOUNT)
                      .condition(
                          "userId",
                          "in",
                          inAudience.slice(0, listLimit).map(found => found.userId)
                      )
                      .list()
              ).map(found => [found.userId, map(found)])
            : []
    )
    const listed = inAudience
        .slice(0, listLimit)
        .filter(found => accounts.has(found.userId))
        .map(found => {
            const account = accounts.get(found.userId)
            return {
                userId: found.userId,
                username: text(account.username),
                name: jobs.accountName(account),
                open: jobs.accountOpen(account),
                receiveText: flagText(found.receiveNotifications) || "Default",
                emailText: flagText(found.emailNotifications) || "Default",
                pushText: flagText(found.pushNotifications) || "Default",
                digestText: flagText(found.digest) || "Default",
                receiveNotifications: found.receiveNotifications || "default",
                emailNotifications: found.emailNotifications || "default",
                pushNotifications: found.pushNotifications || "default",
                digest: found.digest || "default",
                allNotifications: found.allNotifications === "Y",
                revision: stamp(found)
            }
        })
        .sort((a, b) => a.name.localeCompare(b.name))
    const everyOutside = subscriptions.filter(
        found => found.allNotifications === "Y" && !(holders && holders.has(found.userId))
    ).length
    const editable = registration ? registration.fields : []
    return {
        topic: Object.assign(described(topic, registration, subscriptions), {
            audiencePermission: audience || null,
            defaultsReadOnlyReason: !registration
                ? "Its owner registered nothing an administrator changes here"
                : !editable.length
                  ? "Its owner registered no delivery setting as changed here"
                  : !canManageDefaults
                    ? SETTINGS_MANAGE + " is required to change its defaults"
                    : null,
            subscriptionsReadOnlyReason: !audience
                ? "Its owner registered no audience an administrator subscribes here: each user sets their own in their notification settings"
                : !canManageSubscriptions
                  ? lib.MANAGE + " is required to set a subscription"
                  : null
        }),
        fieldList: Object.keys(fieldTitles).map(field => ({
            field,
            title: fieldTitles[field],
            value: field === "description" ? text(topic.description) : topic[field] || "",
            valueText: field === "description" ? text(topic.description) : flagText(topic[field]) || "Not set",
            editable: canManageDefaults && editable.includes(field)
        })),
        subscriptionList: listed,
        // settings of users outside the audience (each user's own preference), counted and never named
        otherSubscriptionCount: subscriptions.length - listed.length,
        // users who receive every message of the topic and are outside its audience: an instance administrator's to review
        allNotificationsOutsideCount: everyOutside,
        canManageDefaults,
        canManageSubscriptions
    }
}

// store#TopicDefaults: the delivery settings a registration names, each checked before any is written.
function storeTopicDefaults(p) {
    lib.signedIn()
    if (!ec.user.hasPermission(SETTINGS_MANAGE)) throw new Error(SETTINGS_MANAGE + " is required")
    const name = text(p.topic)
    const row = name ? find(TOPIC, {topic: name}).forUpdate(true).one() : null
    if (!row) throw new Error("No notification topic " + name)
    const topic = map(row)
    const reason = lib.reasonText(p)
    if (reason) lib.plainText(reason, "The reason")
    const revision = stamp(topic)
    const no = (reasonCode, problem) => refused(reasonCode, problem, {revision})
    const registration = registrationOf(name)
    if (!registration) return no("NOT_REGISTERED", "Its owner registered nothing an administrator changes here")
    if (text(p.expectedRevision) !== revision)
        return no("STALE_REVISION", "It changed since it was read; read it again and repeat")
    const fields = {}
    const audits = []
    for (const field of Object.keys(fieldTitles)) {
        if (p[field] === undefined || p[field] === null) continue
        let wanted
        if (field === "description") {
            wanted = lib.plainText(p.description, "The description")
            if (!wanted) continue
        } else {
            const value = text(p[field])
            if (!value) continue
            // push alone has a third value: by the topic's inbox
            if (!(["Y", "N"].includes(value) || (field === "pushNotifications" && value === "default")))
                throw new Error(fieldTitles[field] + " is Y or N")
            wanted = value === "default" ? null : value
        }
        if ((topic[field] ?? null) === wanted) continue
        if (!registration.fields.includes(field))
            return no("NOT_EDITABLE", fieldTitles[field] + " is its owner's and is not changed here")
        if (field === "persistOnSend" && wanted !== "Y" && text(topic.approveServiceName))
            return no("APPROVAL_KEPT", "This topic asks for a decision: its notifications are always kept")
        if (field === "emailNotifications" && wanted === "Y" && !text(topic.emailTemplateId))
            return no("NO_EMAIL_TEMPLATE", "Its owner gave it no email template, so nothing is sent by email")
        fields[field] = wanted
        audits.push([field, topic[field] ?? null, wanted])
    }
    if (!audits.length) return {applied: true, changedList: [], revision}
    row.setFields(fields, true, null, null)
    row.update()
    for (const [field, before, after] of audits) lib.auditRow(TOPIC, field, name, null, before, after, reason)
    return {applied: true, changedList: audits.map(found => found[0]), revision: stamp(map(one(TOPIC, {topic: name})))}
}

// store#TopicSubscription: one user's delivery choices for a topic, within the topic's registered audience.
function storeTopicSubscription(p) {
    lib.requireManage()
    const name = text(p.topic)
    const topicRow = name ? one(TOPIC, {topic: name}) : null
    if (!topicRow) throw new Error("No notification topic " + name)
    const reason = lib.reasonText(p)
    if (reason) lib.plainText(reason, "The reason")
    const registration = registrationOf(name)
    const audience = text(registration?.subscriptionPermission)
    if (!audience)
        return refused(
            "NOT_REGISTERED",
            "Its owner registered no audience an administrator subscribes here: each user sets their own in their notification settings"
        )
    const username = text(p.username)
    if (!username) throw new Error("Name the user")
    const account = map(one(ACCOUNT, {username: username.toLowerCase()}) || one(ACCOUNT, {username}) || null)
    // one answer for an unknown name and for a user outside the audience, so names are not found out here
    if (!jobs.accountOpen(account) || !jobs.userHolds(account.userId, audience))
        return refused(
            "NOT_ELIGIBLE",
            "No user of that name is in this topic's audience: a subscriber is an account that signs in and holds " + audience
        )
    const userId = account.userId
    const row = find(TOPIC_USER, {topic: name, userId}).forUpdate(true).one()
    const current = row ? map(row) : null
    const revision = current ? stamp(current) : ""
    if (text(p.expectedRevision) !== revision)
        return refused("STALE_REVISION", "It changed since it was read; read it again and repeat", {revision})
    const remove = p.remove === true || p.remove === "true"
    if (remove) {
        if (!row) return refused("NOT_SUBSCRIBED", "That user has no setting for this topic", {revision})
        row.delete()
        lib.auditRow(TOPIC_USER, "subscription", name, userId, "set", null, reason)
        return {applied: true, changedList: ["removed"], revision: ""}
    }
    const fields = {}
    const audits = []
    for (const field of subscriptionFields) {
        const value = text(p[field])
        if (!value) continue
        if (!["Y", "N", "default"].includes(value)) throw new Error("A choice is Y, N or default")
        const wanted = value === "default" ? null : value
        if ((current?.[field] ?? null) === wanted) continue
        fields[field] = wanted
        audits.push([field, current?.[field] ?? null, wanted])
    }
    // receiving every message of the topic is only ever ended here
    if ((p.endAllNotifications === true || p.endAllNotifications === "true") && current?.allNotifications === "Y") {
        fields.allNotifications = "N"
        audits.push(["allNotifications", "Y", "N"])
    }
    if (!audits.length) return {applied: true, changedList: [], revision}
    if (row) {
        row.setFields(fields, true, null, null)
        row.update()
    } else lib.write(TOPIC_USER, Object.assign({topic: name, userId}, fields), true)
    for (const [field, before, after] of audits) lib.auditRow(TOPIC_USER, field, name, userId, before, after, reason)
    return {
        applied: true,
        changedList: audits.map(found => found[0]),
        revision: stamp(map(one(TOPIC_USER, {topic: name, userId})))
    }
}

const operations = {findTopics, getTopic, storeTopicDefaults, storeTopicSubscription}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown topic operation " + op)
        return operations[op](parameters)
    }
})
