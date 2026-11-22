// Board creation must preserve an existing wiki page at the requested name, including another page type.
const boardCall = (service, parameters) => {
    const result = ec.service.sync().name(service).parameters(parameters).disableAuthz().call() || {}
    const error = ec.message.hasError() ? ec.message.errorsString : ""
    ec.message.clearErrors()
    return {...result, error}
}

feature("a new personal board opens as editable and creating the same name preserves the scene", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const service = "org.moqui.comms.BoardServices."
        const title = "Planning sketch"
        const created = boardCall(service + "create#Board", {pageName: title})
        const duplicate = boardCall(service + "create#Board", {pageName: title})
        const board = boardCall(service + "get#Board", {wikiPageId: created.wikiPageId})
        return {created, duplicate, board}
    },
    then({created, duplicate, board}) {
        created.error === ""
        duplicate.error.includes("already exists")
        board.error === ""
        board.canEdit === true
        board.title === "Planning sketch"
        JSON.parse(board.scene).type === "excalidraw"
    }
})
