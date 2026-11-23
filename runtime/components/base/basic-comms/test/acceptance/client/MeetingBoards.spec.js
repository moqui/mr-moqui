// One meeting from the room to the stage and back. The two pages have different sessions: the visitor
// has only the call credential. Read the host's actual canvas after the guest draws with its toolbar.
let host, guest
const button = (page,name) => page.getByRole("button",{name,exact:true}).first()

spec({
    requires:["rtc"],
    setupSpec() {
        host=browser.newPage({user:"john.doe",timeout:30})
        guest=browser.newPage({timeout:30})
        host.allowProblems(/userPhoto\?userId=/)
    }
})

feature("a room meeting presents a share, then a board which an admitted guest draws on", {
    when() {
        host.goto("/qapps/BasicComms/Calls")
        const meeting=host.evaluate(async () => {
            const chat=await moqui.request("/apps/BasicComms/Chat/createRoom",{method:"POST",
                params:{roomTypeEnumId:"private",encrypted:false,roomName:"Board meeting "+Date.now()}})
            const room=chat.json.chatRoomId
            if (!room) throw Error(JSON.stringify(chat.json))
            const link=await moqui.call.request("createCallLink",{name:"Drawing session",chatRoomId:room,policy:"link"})
            const opened=await moqui.call.request("openCallLink",{callLinkId:link.callLinkId})
            moqui.call.open(opened.callSessionId)
            await moqui.call.join(opened.callSessionId,opened.callLegId,{video:true})
            return {path:link.path, callLinkId:link.callLinkId, room}
        })
        guest.goto(meeting.path)
        guest.getByLabel("Your name").fill("Board visitor")
        button(guest,"Join").click()
        guest.waitFor(()=>moqui.call.state.phase==="connected",{timeout:25})
        button(host,"Share the screen").click()
        button(host,"A window or screen").click()
        guest.waitFor(()=>Object.values(moqui.call.state.remotes).some(r=>!!r.screen),{timeout:20})
        button(host,"Whiteboard").click()
        host.getByLabel("Board name").fill("Meeting sketch")
        button(host,"New board").click()
        guest.locator(".m-meeting-board label[title^=Rectangle]").click()
        guest.evaluate(async () => {
            const canvas=document.querySelector(".m-meeting-board canvas.interactive")
            const rect=canvas.getBoundingClientRect(), x=rect.x+rect.width/2, y=rect.y+rect.height/2
            const send=(type,dx,dy,buttons)=>canvas.dispatchEvent(new PointerEvent(type,{bubbles:true,cancelable:true,
                clientX:x+dx,clientY:y+dy,pointerType:"mouse",pointerId:1,isPrimary:true,button:0,buttons}))
            send("pointermove",0,0,0);send("pointerdown",0,0,1)
            await new Promise(requestAnimationFrame);send("pointermove",62,38,1)
            await new Promise(requestAnimationFrame);send("pointerup",62,38,0)
            return true
        })
        const drawn=host.waitFor(()=>{
            function hasShape(v) {
                if (!v) return false
                const component=v.component
                if(component?.type.name==="mWhiteboard") return !!component.proxy.api?.getSceneElements().some(e=>e.type==="rectangle")
                if(component && hasShape(component.subTree)) return true
                return Array.isArray(v.children) && v.children.some(hasShape)
            }
            return hasShape(moqui.webrootVue.$.subTree)
        },{timeout:20})
        const board=host.evaluate(()=>moqui.call.state.stage.wikiPageId)
        host.getByText("Who can draw",{exact:true}).first().click()
        host.getByRole("switch",{name:"Guests"}).click()
        guest.waitFor(()=>moqui.call.state.stage.guestCanEdit===false)
        host.keyboard.press("Escape")
        const listed=host.evaluate(async room=>(await moqui.request("/apps/BasicComms/Chat/roomBoards",{params:{chatRoomId:room}})).json.boards,meeting.room)
        guest.evaluate(id => {
            // Removal may close the admitted guest's socket before its board's revocation event arrives.
            window.drawingSocket = moqui.call.guest.socket
            moqui.call.guest.registerListener("collab:WikiPage:" + id, frame => {
                if (frame.type === "error" && frame.code === 403 && frame.message === "you may no longer view this page")
                    window.boardGrantRevoked = true
            })
            return true
        }, board)
        host.evaluate(async callLinkId => {
            await moqui.request("/apps/BasicComms/Calls/History/revokeLink", {method:"POST", params:{callLinkId}})
            return true
        }, meeting.callLinkId)
        guest.waitFor(() => window.boardGrantRevoked === true || window.drawingSocket.readyState === WebSocket.CLOSED, {timeout:35})
        const saved = host.evaluate(async wikiPageId => {
            await moqui.call.request("endCall", {callSessionId:moqui.call.state.current})
            const {json} = await moqui.request("/apps/BasicComms/Boards/Board/board", {params:{wikiPageId}})
            const version = json.versionList.find(v => v.note?.includes("Meeting Drawing session ended"))
            return {writer:version?.userId, guest:version?.note?.includes("Board visitor"),
                drawn:JSON.parse(json.scene).elements.some(e => e.type === "rectangle")}
        }, board)
        return {drawn,saved,listed:listed.some(b=>b.wikiPageId===board && b.title==="Meeting sketch")}
    },
    then({drawn,saved,listed}) {
        drawn == true
        listed == true
        saved.writer === "EX_JOHN_DOE" && saved.guest === true && saved.drawn === true
    }
})
