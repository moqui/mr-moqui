// A hosted root (plans/design-dev-aide.md 4.10, src/hosted.rs): a db repository of this server's git hosting, added
// at its branch by a user who may update it (one who may not is refused, and is not shown the root; another who may
// does not relabel it), is a root of kind hosted with no working tree. Its tree, a file, Go to file and search read the
// branch; a Save is a commit by the user with its message (Edit <path> by default) and the task trailer; a Save over a
// file the branch moved past since it was opened is a conflict with the branch's text, which a Save at that blob then
// commits over; a new file, a new folder, a move and a delete are commits too, and the first Save of an empty
// repository makes its branch; a branch whose rules refuse a direct commit refuses the Save and the root says why; the
// git commands of a working tree and Replace across files are refused. Repositories' Add hosted root is checked
// through its transition and get#HostedRepositories (WorkspaceTui renders the screen).
// Run: ./moq test dev-aide --spec HostedRoot
const GIT = "org.moqui.git.GitServices."
const HOSTED = "org.moqui.devaide.HostedRootServices."
const WS = "org.moqui.devaide.WorkspaceServices."
// a suffix keeps each run's repository apart on a database kept between runs
const RUN = Date.now().toString(36).slice(-5).toUpperCase()
const REPO = "SPEC_HOSTED_" + RUN
const EMPTY = "SPEC_HOSTED_EMPTY_" + RUN
const README = "# Spec\n\nThe first line.\n"

const call = (name, parameters) => {
    try {
        const out =
            ec.service
                .sync()
                .name(name)
                .parameters(parameters || {})
                .call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        const errors = ec.message.getErrorsString() || String(error.message)
        ec.message.clearErrors()
        return {out: {}, errors}
    }
}
// a commit from elsewhere (another page, a push): the git services themselves, which only the Dev AIDE's frame grants here
const commitDirect = (path, text, message) => {
    const done = asSystem(() => call(GIT + "commit#GitFiles", {repositoryId: REPO, message, fileMap: {[path]: text}}))
    if (!done.out.commitId) throw new Error("The commit of " + path + " failed: " + done.errors)
    return done.out.commitId
}
const asSystem = body => {
    ec.artifactExecution.disableAuthz()
    try {
        return body()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
let rootId

spec({
    stepwise: true,
    requires: ["git"],
    setupSpec() {
        asSystem(() => {
            const make = (entity, fields) => {
                const row = ec.entity.makeValue(entity)
                row.setFields(fields, true, null, null)
                row.createOrUpdate()
            }
            make("moqui.resource.GitRepository", {
                repositoryId: REPO,
                repositoryKind: "db",
                branch: "main",
                description: "Spec hosted root"
            })
            make("moqui.resource.GitRepository", {
                repositoryId: EMPTY,
                repositoryKind: "db",
                branch: "main",
                description: "Spec empty"
            })
            // a second user who may update the repository: a member of a group with every right on it
            const since = new Date(Date.now() - 60000)
            make("moqui.security.UserAccount", {
                userId: "specHostedPeer",
                username: "spec.hosted.peer",
                userFullName: "Spec Hosted Peer"
            })
            make("moqui.security.UserGroupMember", {userGroupId: "DEV_AIDE_EXTENDERS", userId: "specHostedPeer", fromDate: since})
            make("moqui.security.UserGroup", {userGroupId: "SPEC_HOSTED_" + RUN, description: "Spec hosted peers"})
            make("moqui.security.UserGroupMember", {userGroupId: "SPEC_HOSTED_" + RUN, userId: "specHostedPeer", fromDate: since})
            make("moqui.security.ArtifactGroup", {artifactGroupId: "SPEC_HOSTED_" + RUN, description: "Spec hosted repository"})
            make("moqui.security.ArtifactGroupMember", {
                artifactGroupId: "SPEC_HOSTED_" + RUN,
                artifactName: REPO,
                artifactTypeEnumId: "AT_GIT_REPOSITORY",
                nameIsPattern: "N",
                inheritAuthz: "N"
            })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: "SPEC_HOSTED_" + RUN,
                userGroupId: "SPEC_HOSTED_" + RUN,
                artifactGroupId: "SPEC_HOSTED_" + RUN,
                authzTypeEnumId: "AUTHZT_ALWAYS",
                authzActionEnumId: "AUTHZA_ALL"
            })
            // an extend-tier user with no right in the repository
            make("moqui.security.UserAccount", {
                userId: "specHostedOutsider",
                username: "spec.hosted.outsider",
                userFullName: "Spec Hosted Outsider"
            })
            make("moqui.security.UserGroupMember", {
                userGroupId: "DEV_AIDE_EXTENDERS",
                userId: "specHostedOutsider",
                fromDate: new Date(Date.now() - 60000)
            })
        })
        login("john.doe")
        commitDirect("README.md", README, "Start")
        commitDirect("data/Seed.xml", '<entity-facade-xml type="seed">\n</entity-facade-xml>\n', "Seed")
    },
    cleanupSpec() {
        login("john.doe")
        ec.user.setPreference("DEV_AIDE_CURRENT_TASK." + rootId, "")
        asSystem(() => {
            ec.entity.find("moqui.dev.DevAideRoot").condition("gitRepositoryId", REPO).deleteAll()
            ec.entity.find("moqui.dev.DevAideRoot").condition("gitRepositoryId", EMPTY).deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", "specHostedPeer").deleteAll()
            ec.entity.find("moqui.resource.GitBranchRule").condition("repositoryId", REPO).deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", "specHostedOutsider").deleteAll()
        })
    }
})

feature("a user who may not update the repository is refused, and one who may adds it as a root with no working tree", {
    when() {
        ec.user.internalLoginUser("spec.hosted.outsider")
        const refused = call(HOSTED + "store#HostedRoot", {repositoryId: REPO, branch: "main"})
        login("john.doe")
        const offered = call(HOSTED + "get#HostedRepositories").out.repositories || []
        const added = call(HOSTED + "store#HostedRoot", {repositoryId: REPO, branch: "main"})
        rootId = added.out.rootId
        const root = (call(WS + "get#Roots").out.roots || []).find(item => item.id === rootId) || {}
        return {refused, offered: offered.find(item => item.repositoryId === REPO), added, root}
    },
    then({refused, offered, added, root}) {
        refused.errors.includes("You may not update " + REPO)
        offered.branches.includes("main")
        added.errors == ""
        root.kind == "hosted"
        root.hostedBranch.repositoryId == REPO
        root.hostedBranch.branch == "main"
        root.upstream == false
    }
})

feature("a user who may not view the repository is not shown the root, and one who did not add it does not relabel it", {
    when() {
        // Repositories' form names the repository and branch; the root takes its label
        const posted = ec.screen
            .makeTest()
            .rootScreen("component://dev-aide/screen/DevAide.xml")
            .render(
                "Repositories/addHostedRoot",
                {hostedBranch: JSON.stringify([REPO, "main"]), hostedLabel: "Spec hosted"},
                "post"
            )
        const mine = (call(WS + "get#Roots").out.roots || []).find(item => item.id === rootId) || {}
        ec.user.internalLoginUser("spec.hosted.outsider")
        const outsider = (call(WS + "get#Roots").out.roots || []).some(item => item.id === rootId)
        ec.user.internalLoginUser("spec.hosted.peer")
        const peer = (call(WS + "get#Roots").out.roots || []).some(item => item.id === rootId)
        const relabel = call(HOSTED + "store#HostedRoot", {repositoryId: REPO, branch: "main", label: "Taken"})
        login("john.doe")
        return {posted, label: mine.label, outsider, peer, relabel}
    },
    then({posted, label, outsider, peer, relabel}) {
        posted.status == 302
        posted.errorMessages.length == 0
        label == "Spec hosted"
        outsider == false
        peer == true
        relabel.errors.includes("added by another user")
    }
})

feature("the tree, a file, Go to file and search read the branch", {
    when() {
        const top = call(WS + "list#Directory", {rootId, path: ""}).out.entries || []
        const file = call(WS + "get#File", {rootId, path: "README.md"}).out
        const files = call(WS + "find#Files", {rootId}).out.files || []
        const found = call(WS + "search#Files", {rootId, query: "first line"}).out.results || []
        return {top, file, files, found}
    },
    then({top, file, files, found}) {
        top.map(entry => entry.name + (entry.dir ? "/" : "")).join(",") == "data/,README.md"
        file.text == README
        file.hash.length == 40
        file.readOnly == false
        JSON.stringify(files) == JSON.stringify(["README.md", "data/Seed.xml"])
        found.length == 1
        found[0].path == "README.md"
        found[0].line == 3
    }
})

feature("a Save is a commit by the user with its message (Edit <path> by default) and the task trailer", {
    when() {
        ec.user.setPreference("DEV_AIDE_CURRENT_TASK." + rootId, "SPEC-12")
        const opened = call(WS + "get#File", {rootId, path: "README.md"}).out
        const saved = call(WS + "save#File", {
            rootId,
            path: "README.md",
            text: README + "A second line.\n",
            baseHash: opened.hash
        })
        const log = asSystem(
            () => call(GIT + "get#GitLog", {repositoryId: REPO, revision: "main", limit: 1}).out.commitList || []
        )
        const now = call(WS + "get#File", {rootId, path: "README.md"}).out
        // the Save dialog's message
        const named = call(WS + "save#File", {
            rootId,
            path: "README.md",
            text: now.text + "Third.\n",
            baseHash: now.hash,
            message: "Say more"
        })
        const after = asSystem(
            () => call(GIT + "get#GitLog", {repositoryId: REPO, revision: "main", limit: 1}).out.commitList || []
        )
        return {opened, saved, top: log[0] || {}, now, named, last: after[0] || {}}
    },
    then({opened, saved, top, now, named, last}) {
        saved.errors == ""
        saved.out.conflict == false
        saved.out.commitId == top.commitId
        top.summary == "Edit README.md"
        top.message.includes("Task: SPEC-12")
        top.moquiUserId == ec.user.userId
        saved.out.hash == now.hash
        now.hash != opened.hash
        now.text.endsWith("A second line.\n")
        named.errors == ""
        last.summary == "Say more"
    }
})

feature("a Save over a file the branch moved past is a conflict with the branch's text, then commits over it", {
    when() {
        const opened = call(WS + "get#File", {rootId, path: "README.md"}).out
        commitDirect("README.md", opened.text + "From elsewhere.\n", "Elsewhere")
        const stale = call(WS + "save#File", {
            rootId,
            path: "README.md",
            text: opened.text + "Mine.\n",
            baseHash: opened.hash
        }).out
        const over = call(WS + "save#File", {rootId, path: "README.md", text: opened.text + "Mine.\n", baseHash: stale.diskHash})
        return {stale, over, now: call(WS + "get#File", {rootId, path: "README.md"}).out}
    },
    then({stale, over, now}) {
        stale.conflict == true
        stale.diskText.endsWith("From elsewhere.\n")
        stale.diskHash.length == 40
        over.errors == ""
        over.out.conflict == false
        now.text.endsWith("Mine.\n")
    }
})

feature("a branch whose rules refuse a direct commit refuses the Save, and the root says why", {
    when() {
        asSystem(() => call(GIT + "store#GitBranchRule", {repositoryId: REPO, refPattern: "main", requireMergeRequest: "Y"}))
        const root = call(HOSTED + "get#HostedRoot", {rootId}).out
        const file = call(WS + "get#File", {rootId, path: "README.md"}).out
        const refused = call(WS + "save#File", {rootId, path: "README.md", text: file.text + "Refused.\n", baseHash: file.hash})
        asSystem(() => ec.entity.find("moqui.resource.GitBranchRule").condition("repositoryId", REPO).deleteAll())
        return {root, file, refused, after: call(WS + "get#File", {rootId, path: "README.md"}).out}
    },
    then({root, file, refused, after}) {
        root.canCommit == false
        root.refusal.includes("protected")
        file.readOnly == true
        refused.errors.includes("protected")
        !after.text.includes("Refused.")
    }
})

feature("a new file, a new folder, a move and a delete are commits; the git commands of a working tree and Replace are refused", {
    when() {
        const created = call(WS + "create#File", {rootId, path: "notes/new.txt", text: "new\n"})
        const again = call(WS + "create#File", {rootId, path: "notes/new.txt", text: "x"})
        const folder = call(WS + "create#Directory", {rootId, path: "empty"})
        const moved = call(WS + "rename#Path", {rootId, path: "notes/new.txt", to: "notes/moved.txt"})
        const deleted = call(WS + "delete#Path", {rootId, path: "notes"})
        const files = call(WS + "find#Files", {rootId}).out.files || []
        const log = asSystem(
            () => call(GIT + "get#GitLog", {repositoryId: REPO, revision: "main", limit: 4}).out.commitList || []
        )
        return {
            created,
            again,
            folder,
            moved,
            deleted,
            files,
            summaries: log.map(commit => commit.summary),
            status: call("org.moqui.devaide.VcsServices.get#Status", {rootId}).errors,
            replaced: call("org.moqui.devaide.WorkspaceServices.replace#Files", {rootId, query: "Spec", replacement: "x"}).errors
        }
    },
    then({created, again, folder, moved, deleted, files, summaries, status, replaced}) {
        created.errors == ""
        again.errors.includes("already there")
        folder.errors == ""
        moved.errors == ""
        deleted.errors == ""
        JSON.stringify(files) == JSON.stringify(["README.md", "data/Seed.xml", "empty/.gitkeep"])
        JSON.stringify(summaries) ==
            JSON.stringify(["Delete notes", "Move notes/new.txt to notes/moved.txt", "Add empty", "Add notes/new.txt"])
        status.includes("not a git repository")
        replaced.includes("Replace across files is not offered")
    }
})

feature("the first Save of an empty repository makes its branch", {
    when() {
        const added = call(HOSTED + "store#HostedRoot", {repositoryId: EMPTY})
        const empty = added.out.rootId
        const opened = call(WS + "get#File", {rootId: empty, path: "README.md", missingOk: "true"}).out
        const saved = call(WS + "save#File", {rootId: empty, path: "README.md", text: "First\n", baseHash: ""})
        return {added, opened, saved, now: call(WS + "get#File", {rootId: empty, path: "README.md"}).out}
    },
    then({added, opened, saved, now}) {
        added.errors == ""
        opened.missing == true
        saved.errors == ""
        saved.out.conflict == false
        now.text == "First\n"
    }
})
