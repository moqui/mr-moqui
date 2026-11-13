// The drive picker of BasicComms (org.moqui.comms.FilePickerServices, template/FilePickerTransitions.xml; plans/archive/Comms-Framework.md
// §7.4 DP): Compose's Attach from Files and the Inbox's Save to Files over the framework drives alone. A user with no party
// and a mail account of their own sees their drives, recent files, a folder and a search; attaches a drive file to a draft;
// saves a mail attachment into a folder (a second time renamed); another user with the same parameters is refused, and so
// is the user's save into another user's drive; a user with no drive is told so, which leaves the buttons out; and a user
// whose personal drive is not made yet has one, without the check making it.
// Run: ./moq test basic-comms --spec BasicCommsFiles
const USER = "BASIC_COMMS_FILES"
const OTHER = "BASIC_COMMS_FILES_OTHER"
const NO_DRIVE = "BASIC_COMMS_FILES_NODRIVE"
const FRESH = "BASIC_COMMS_FILES_FRESH"
const ACCOUNT = "BASIC_COMMS_FILES_ACCT"
const FOLDER = "BASIC_COMMS_FILES_INBOX"
const MESSAGE = "BASIC_COMMS_FILES_M1"
const ATTACHMENT = "BASIC_COMMS_FILES_M1_A1"
const NO_FILES = "BASIC_COMMS_FILES_M2"
const ROOT = "component://basic-comms/screen/BasicComms.xml"
// a group IronStone keeps out of its Comms, so run#Operation answers from the framework when IronStone is installed too
const FRAMEWORK_GROUP = "BOSS_PROJECT_CLIENT"
let personalDriveId
let otherDriveId

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// as the screens call them, authorized by the app's screen
function attempt(name, parameters) {
    let out = {}
    let thrown = ""
    try {
        out = sys(() => call(name, parameters || {})) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const picker = (name, parameters) => attempt("org.moqui.comms.FilePickerServices." + name, parameters)
function render(path, parameters, method) {
    const out = ec.screen
        .makeTest()
        .rootScreen(ROOT)
        .render(path, parameters || {}, method || null)
    let json = null
    try {
        json = JSON.parse(out.output)
    } catch (err) {
        json = null
    }
    ec.message.clearErrors()
    return {status: out.status, errors: (json && json.errors) || out.errorMessages.join(" | "), json: json || {}}
}
function store(entityName, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entityName)
        value.setFields(fields, true, null, null)
        value.createOrUpdate()
    })
}
function makeUser(userId, username) {
    store("moqui.security.UserAccount", {
        userId,
        username,
        userFullName: "Files " + username,
        emailAddress: username + "@example.com",
        disabled: "N"
    })
    // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS
    store("moqui.security.UserGroupMember", {userGroupId: "BASIC_COMMS_USERS", userId, fromDate: "2026-01-01 00:00:00"})
    sys(() => {
        if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", FRAMEWORK_GROUP).count() == 0) return
        if (
            ec.entity
                .find("moqui.security.UserGroupMember")
                .condition("userGroupId", FRAMEWORK_GROUP)
                .condition("userId", userId)
                .count() > 0
        )
            return
        const member = ec.entity.makeValue("moqui.security.UserGroupMember")
        member.setFields({userGroupId: FRAMEWORK_GROUP, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
        member.create()
    })
}
const names = list => (list || []).map(row => row.name)

spec({
    stepwise: true,
    setupSpec() {
        makeUser(USER, "basic.comms.files")
        makeUser(OTHER, "basic.comms.files.other")
        makeUser(NO_DRIVE, "basic.comms.files.nodrive")
        makeUser(FRESH, "basic.comms.files.fresh")
        // a mail account delivered here, one message with a file
        store("moqui.basic.email.EmailAccount", {
            emailAccountId: ACCOUNT,
            ownerUserId: USER,
            accountKindEnumId: "EakHosted",
            emailAddress: "files.spec@example.com",
            accountName: "Files mail",
            statusId: "EasActive"
        })
        store("moqui.basic.email.EmailFolder", {
            emailFolderId: FOLDER,
            emailAccountId: ACCOUNT,
            folderName: "Inbox",
            folderPath: "INBOX",
            folderKindEnumId: "EfkInbox",
            subscribed: "Y",
            sequenceNum: 1
        })
        store("moqui.basic.email.EmailMessage", {
            emailMessageId: MESSAGE,
            rootEmailMessageId: MESSAGE,
            emailAccountId: ACCOUNT,
            emailFolderId: FOLDER,
            statusId: "ES_RECEIVED",
            fromAddress: "pat@example.org",
            fromName: "Pat Sender",
            toAddresses: "files.spec@example.com",
            subject: "The figures",
            body: "<p>The figures are attached.</p>",
            contentType: "text/html",
            messageDate: ec.user.nowTimestamp,
            receivedDate: ec.user.nowTimestamp,
            isRead: "N",
            messageId: "<basic-comms-files-1@example.org>"
        })
        store("moqui.basic.email.EmailMessage", {
            emailMessageId: NO_FILES,
            rootEmailMessageId: NO_FILES,
            emailAccountId: ACCOUNT,
            emailFolderId: FOLDER,
            statusId: "ES_RECEIVED",
            fromAddress: "pat@example.org",
            toAddresses: "files.spec@example.com",
            subject: "No files",
            body: "<p>Nothing attached.</p>",
            contentType: "text/html",
            messageDate: ec.user.nowTimestamp,
            receivedDate: ec.user.nowTimestamp,
            isRead: "N",
            messageId: "<basic-comms-files-2@example.org>"
        })
        const location = `dbresource://email/${ACCOUNT}/${MESSAGE}/figures.txt`
        sys(() => ec.resource.getLocationReference(location).putText("Figures for the quarter"))
        store("moqui.basic.email.EmailMessageAttachment", {
            emailMessageAttachmentId: ATTACHMENT,
            emailMessageId: MESSAGE,
            attachmentLocation: location,
            fileName: "figures.txt",
            contentType: "text/plain",
            fileSize: 23
        })
        // the user's own drive with a folder and two files, uploaded as the user (so they are recent)
        ec.user.internalLoginUser("basic.comms.files")
        personalDriveId = sys(() => call("org.moqui.resource.FileServices.ensure#PersonalDrive", {})).driveId
        sys(() =>
            call("org.moqui.resource.FileServices.upload#Resource", {
                driveId: personalDriveId,
                path: "Reports",
                fileList: ["Summary for the quarter", "a,b\n1,2\n"],
                relativePathList: ["Quarterly summary.txt", "Quarterly numbers.csv"],
                conflictPolicy: "replace"
            })
        )
        ec.user.internalLoginUser("basic.comms.files.other")
        otherDriveId = sys(() => call("org.moqui.resource.FileServices.ensure#PersonalDrive", {})).driveId
        // a user whose only drive is archived (left out of their drives): no drive to pick from
        ec.user.internalLoginUser("basic.comms.files.nodrive")
        const archived = sys(() => call("org.moqui.resource.FileServices.ensure#PersonalDrive", {})).driveId
        store("moqui.resource.ResourceDrive", {driveId: archived, archivedDate: ec.user.nowTimestamp})
    },
    cleanupSpec() {
        login("john.doe")
    }
})

feature("the picker opens with the user's drives and recent files, through the Compose and Mail transitions too", {
    when() {
        ec.user.internalLoginUser("basic.comms.files")
        const data = picker("get#PickerData")
        const compose = render("Inbox/Compose/pickerData")
        const check = render("Inbox/Mail/pickerData", {checkOnly: "true"})
        ec.user.internalLoginUser("basic.comms.files.other")
        const other = picker("get#PickerData")
        return {data, compose, check, other}
    },
    then({data, compose, check, other}) {
        data.errors == ""
        data.hasDrive == true
        data.personalDriveId == personalDriveId
        data.driveList[0].driveId == personalDriveId
        data.driveList[0].driveName == "My files"
        data.driveList[0].isPersonal == true
        names(data.recentList).includes("Quarterly summary.txt")
        data.recentList.find(row => row.name === "Quarterly summary.txt").folderPath == "Reports"
        data.recentList.find(row => row.name === "Quarterly numbers.csv").icon == "sym_o_table_chart"
        compose.status == 200
        compose.json.personalDriveId == personalDriveId
        names(compose.json.recentList).includes("Quarterly summary.txt")
        check.json.hasDrive == true
        check.json.recentList.length == 0
        other.personalDriveId == otherDriveId
        !other.driveList.some(drive => drive.driveId === personalDriveId)
        other.recentList.length == 0
    }
})

feature("a folder lists its files with their kinds and the breadcrumbs; a search finds files by name", {
    when() {
        ec.user.internalLoginUser("basic.comms.files")
        const top = picker("list#PickerFolder", {driveId: personalDriveId, path: ""})
        const reports = picker("list#PickerFolder", {driveId: personalDriveId, path: "Reports"})
        const found = picker("find#PickerFiles", {text: "quarterly"})
        const blank = picker("find#PickerFiles", {text: "  "})
        const searched = render("Inbox/Compose/pickerSearch", {text: "numbers"})
        const folderScreen = render("Inbox/Mail/pickerFolder", {driveId: personalDriveId, path: "Reports"})
        ec.user.internalLoginUser("basic.comms.files.other")
        const otherFound = picker("find#PickerFiles", {text: "quarterly"})
        return {top, reports, found, blank, searched, folderScreen, otherFound}
    },
    then({top, reports, found, blank, searched, folderScreen, otherFound}) {
        top.errors == ""
        names(top.fileList).includes("Reports")
        top.fileList.find(row => row.name === "Reports").isDirectory == true
        top.fileList.find(row => row.name === "Reports").icon == "sym_o_folder"
        reports.breadcrumbList.map(crumb => crumb.name) == ["My files", "Reports"]
        reports.canWrite == true
        names(reports.fileList).sort() == ["Quarterly numbers.csv", "Quarterly summary.txt"]
        reports.fileList.find(row => row.name === "Quarterly summary.txt").fileType == "text"
        reports.fileList.find(row => row.name === "Quarterly numbers.csv").fileType == "spreadsheet"
        found.errors == ""
        names(found.fileList).sort() == ["Quarterly numbers.csv", "Quarterly summary.txt"]
        found.fileList[0].driveName == "My files"
        found.fileList[0].folderPath == "Reports"
        blank.fileList.length == 0
        searched.status == 200
        names(searched.json.fileList) == ["Quarterly numbers.csv"]
        folderScreen.status == 200
        names(folderScreen.json.fileList).sort() == ["Quarterly numbers.csv", "Quarterly summary.txt"]
        otherFound.errors == ""
        otherFound.fileList.length == 0
    }
})

feature("Attach from Files copies a drive file onto a draft; another user is refused the same file", {
    when() {
        ec.user.internalLoginUser("basic.comms.files")
        const started = attempt("org.moqui.comms.ComposeServices.get#ComposeData", {})
        const draft = attempt("org.moqui.comms.ComposeServices.save#ComposeDraft", {
            fromValue: started.compose.fromValue,
            toJson: JSON.stringify([{value: "bob@example.com", label: "bob@example.com"}]),
            subject: "The summary",
            body: "<p>Here it is.</p>"
        })
        const items = [{driveId: personalDriveId, path: "Reports/Quarterly summary.txt"}]
        const attached = attempt("org.moqui.comms.InboxServices.run#Operation", {
            operation: "attach#DriveFiles",
            parameters: {emailMessageId: draft.emailMessageId, itemList: items}
        })
        const rows = sys(() =>
            Array.from(
                ec.entity
                    .find("moqui.basic.email.EmailMessageAttachment")
                    .condition("emailMessageId", draft.emailMessageId)
                    .list()
            )
        )
        const text = rows.length ? sys(() => ec.resource.getLocationReference(rows[0].attachmentLocation).getText()) : null
        const upload = attempt("org.moqui.comms.ComposeServices.attach#DriveFiles", {itemList: items})
        ec.user.internalLoginUser("basic.comms.files.other")
        const refused = attempt("org.moqui.comms.ComposeServices.attach#DriveFiles", {itemList: items})
        const folder = picker("list#PickerFolder", {driveId: personalDriveId, path: "Reports"})
        return {draft, attached, rows, text, upload, refused, folder}
    },
    then({draft, attached, rows, text, upload, refused, folder}) {
        draft.errors == ""
        attached.errors == ""
        attached.result.attachmentList.map(file => file.name) == ["Quarterly summary.txt"]
        rows[0].attachmentLocation.startsWith(`dbresource://email/${ACCOUNT}/${draft.emailMessageId}/`)
        text == "Summary for the quarter"
        upload.uploadList.map(file => file.name) == ["Quarterly summary.txt"]
        refused.uploadList.length == 0
        refused.failedList.map(file => file.name) == ["Quarterly summary.txt"]
        folder.errors.includes("Folder not found")
        !folder.fileList
    }
})

feature("Save to Files copies a mail attachment into a folder, renamed the second time; another user is refused", {
    when() {
        ec.user.internalLoginUser("basic.comms.files")
        const parameters = {store: "email", id: ATTACHMENT, driveId: personalDriveId, path: "Reports"}
        const saved = attempt("org.moqui.comms.InboxServices.run#Operation", {operation: "save#MessageFiles", parameters})
        const again = attempt("org.moqui.comms.InboxServices.save#MessageFiles", parameters)
        const whole = attempt("org.moqui.comms.InboxServices.save#MessageFiles", {
            emailMessageId: MESSAGE,
            driveId: personalDriveId,
            path: "Reports"
        })
        const none = attempt("org.moqui.comms.InboxServices.save#MessageFiles", {
            emailMessageId: NO_FILES,
            driveId: personalDriveId,
            path: "Reports"
        })
        const text = sys(() =>
            ec.resource.getLocationReference(`dbresource://drive/personal/${USER}/Reports/figures.txt`).getText()
        )
        const kept = sys(() =>
            ec.entity.find("moqui.basic.email.EmailMessageAttachment").condition("emailMessageAttachmentId", ATTACHMENT).one()
        )
        const screen = render("Inbox/Mail/saveFiles", parameters, "post")
        // the same attachment into a drive the user may not write
        const intoOther = Object.assign({}, parameters, {driveId: otherDriveId, path: ""})
        const refusedDrive = attempt("org.moqui.comms.InboxServices.save#MessageFiles", intoOther)
        const screenRefusedDrive = render("Inbox/Mail/saveFiles", intoOther, "post")
        ec.user.internalLoginUser("basic.comms.files.other")
        const otherFiles = picker("list#PickerFolder", {driveId: otherDriveId, path: ""}).fileList
        const refused = attempt("org.moqui.comms.InboxServices.save#MessageFiles", parameters)
        const screenRefused = render("Inbox/Mail/saveFiles", parameters, "post")
        const refusedWhole = attempt("org.moqui.comms.InboxServices.save#MessageFiles", {
            emailMessageId: MESSAGE,
            driveId: personalDriveId,
            path: "Reports"
        })
        return {saved, again, whole, none, text, kept, screen, refusedDrive, screenRefusedDrive, otherFiles, refused, refusedWhole, screenRefused}
    },
    then({saved, again, whole, none, text, kept, screen, refusedDrive, screenRefusedDrive, otherFiles, refused, refusedWhole, screenRefused}) {
        saved.errors == ""
        saved.result.savedList == [{name: "figures.txt", path: "Reports/figures.txt", outcome: "created"}]
        saved.result.failedList.length == 0
        again.savedList == [{name: "figures (2).txt", path: "Reports/figures (2).txt", outcome: "renamed"}]
        whole.savedList.map(row => row.name) == ["figures (3).txt"]
        none.errors.includes("This message has no files")
        text == "Figures for the quarter"
        kept.attachmentLocation == `dbresource://email/${ACCOUNT}/${MESSAGE}/figures.txt`
        refused.errors.includes("Attachment not found: " + ATTACHMENT)
        !refused.errors.includes("rollback")
        refusedWhole.errors.includes("Attachment not found: " + MESSAGE)
        screen.status == 200
        screen.json.savedList.map(row => row.name) == ["figures (4).txt"]
        refusedDrive.errors.includes("Folder not found")
        !refusedDrive.errors.includes("rollback")
        names(refusedDrive.savedList).length == 0
        screenRefusedDrive.errors.includes("Folder not found")
        !screenRefusedDrive.errors.includes("rollback")
        !screenRefusedDrive.json.savedList
        names(otherFiles).length == 0
        screenRefused.errors.includes("Attachment not found: " + ATTACHMENT)
        !screenRefused.errors.includes("rollback")
        !screenRefused.json.savedList
    }
})

feature("a user with no drive is told so, and the picker has nothing to open", {
    when() {
        ec.user.internalLoginUser("basic.comms.files.nodrive")
        const out = {data: picker("get#PickerData"), check: render("Inbox/Compose/pickerData", {checkOnly: "true"})}
        // a user whose personal drive is not made yet: the check says there is one and makes none
        ec.user.internalLoginUser("basic.comms.files.fresh")
        out.fresh = render("Inbox/Mail/pickerData", {checkOnly: "true"})
        out.freshDrives = sys(() => ec.entity.find("moqui.resource.ResourceDrive").condition("ownerUserId", FRESH).count())
        return out
    },
    then({data, check, fresh, freshDrives}) {
        data.errors == ""
        data.hasDrive == false
        data.driveList.length == 0
        data.personalDriveId == null
        data.recentList.length == 0
        check.status == 200
        check.json.hasDrive == false
        fresh.json.hasDrive == true
        freshDrives == 0
    }
})
