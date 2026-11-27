// A real room signal, through /notws and the chat store to the desktop's notify callback (COM-UI-015).
// A receiver hears a direct reply, a reply in their thread and a reply in a thread they wrote in.
const base = "/apps/BasicComms/Chat/"
const password = "CallsClient1!!"
function post(user, action, form) {
    const result = user.post(base + action, {form}).json()
    if (result.errors) fail(result.errors)
    return result
}

feature("a mentions-only desktop gets its own reply and thread notifications once from the room socket", {
    when() {
        needsFixtures("the ordinary chat users of CallClientFixtures")
        const viewer = browser.newPage({user: "calls.client.b", password})
        viewer.allowProblems(/userPhoto\?userId=/)
        viewer.goto("/qapps/BasicComms/Home")
        const viewerId = viewer.waitFor(() => moqui.chat.state.connected && moqui.chat.me)
        const john = session({user: "john.doe"})
        const other = session({user: "calls.client.c", password})
        const otherId = other.get(base + "chatData").json().me.userId
        const room = post(john, "createRoom", {roomTypeEnumId: "private", roomName: "Reply notifications", encrypted: "false",
            memberUserIds: viewerId + "," + otherId}).chatRoomId
        viewer.evaluate(async (id) => {
            const chat = window.moqui.chat
            await moqui.request("/apps/BasicComms/Chat/setNotification", {method: "POST", params: {chatRoomId: id, notificationLevelEnumId: "CnlMentions"}})
            await chat.readCounts()
            window.__replies = {seen: [], notices: []}
            window.moquiDesktop = {notify: (notice) => window.__replies.notices.push(notice.link)}
            chat.on("event", ({chatRoomId, event}) => {
                if (chatRoomId === id && event.type === "message") window.__replies.seen.push({
                    seq: event.eventSeq, qualified: event.replyForViewer, recipients: event.replyUserIdList
                })
            })
        }, room)
        const send = (who, fields) => post(who, "send", Object.assign({chatRoomId: room, body: "A message"}, fields)).eventSeq
        const fromViewer = (fields) => viewer.evaluate(async ([id, extra]) => {
            const sent = await moqui.request("/apps/BasicComms/Chat/send", {method: "POST", params: Object.assign({chatRoomId: id, body: "My message"}, extra)})
            return sent.json.eventSeq
        }, [room, fields])
        const mine = fromViewer({})
        const direct = send(john, {replyToEventSeq: mine})
        // Both reasons apply to this one event: it reaches the viewer once.
        const inMine = send(john, {threadRootEventSeq: mine, replyToEventSeq: mine})
        const otherRoot = send(other, {})
        const unrelated = send(john, {threadRootEventSeq: otherRoot})
        fromViewer({threadRootEventSeq: otherRoot})
        const participated = send(john, {threadRootEventSeq: otherRoot})
        const seen = viewer.waitFor((seq) => window.__replies.seen.some((item) => item.seq === seq) && window.__replies, {arg: participated})
        return {seen, direct, inMine, unrelated, participated}
    },
    then({seen, direct, inMine, unrelated, participated}) {
        seen.seen.filter((item) => item.qualified).map((item) => item.seq).join(",") == [direct, inMine, participated].join(",")
        seen.seen.filter((item) => item.seq === unrelated && item.qualified === false).length == 1
        seen.seen.every((item) => item.recipients == null)
        seen.notices.map((link) => Number(link.split("eventSeq=")[1])).join(",") == [direct, inMine, participated].join(",")
    }
})
