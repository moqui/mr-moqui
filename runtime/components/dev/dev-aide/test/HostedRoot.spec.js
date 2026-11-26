// A hosted root (plans/design-dev-aide.md 4.10, src/hosted.rs): a db repository of this server's git hosting, added
// at its branch by a user who may update it (one who may not is refused), is a root of kind hosted with no working tree.
// Its tree, a file, Go to file and search read the branch; a Save is a commit by the user with "Edit <path>" and the
// task trailer; a Save over a file the branch moved past since it was opened is a conflict with the branch's text,
// which a Save at that blob then commits over; Repositories offers the repository and lists the root; a branch whose rules refuse a direct commit refuses the Save and the
// root says why; the git commands of a working tree and the file writes outside Save are refused.
// Run: ./moq test dev-aide --spec HostedRoot
const GIT = "org.moqui.git.GitServices."
const HOSTED = "org.moqui.devaide.HostedRootServices."
const WS = "org.moqui.devaide.WorkspaceServices."
// a suffix keeps each run's repository apart on a database kept between runs
const RUN = Date.now().toString(36).slice(-5).toUpperCase()
const REPO = "SPEC_HOSTED_" + RUN
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

// Repositories' one render (its own logic: the hosted repositories offered, a hosted root's Remove for its Terminal)
feature("Repositories offers each branch of a repository the user may update and lists the hosted root", {
    when() {
        const r = ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml").render("Repositories", {}, null)
        // the form's choice names the repository and branch: an added root takes the label
        const posted = ec.screen
            .makeTest()
            .rootScreen("component://dev-aide/screen/DevAide.xml")
            .render(
                "Repositories/addHostedRoot",
                {hostedBranch: JSON.stringify([REPO, "main"]), hostedLabel: "Spec hosted"},
                "post"
            )
        const root = (call(WS + "get#Roots").out.roots || []).find(item => item.id === rootId) || {}
        return {r, out: String(r.output), posted, label: root.label}
    },
    then({r, out, posted, label}) {
        posted.status == 302
        posted.errorMessages.length == 0
        label == "Spec hosted"
        r.status == 200
        r.errorMessages.length == 0
        !out.includes("Error rendering")
        out.includes("Add hosted root")
        out.includes(REPO + " · main")
        out.includes("removeHostedRoot")
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

feature("a Save is a commit by the user with the default message and the task trailer", {
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
        return {opened, saved, top: log[0] || {}, now}
    },
    then({opened, saved, top, now}) {
        saved.errors == ""
        saved.out.conflict == false
        saved.out.commitId == top.commitId
        top.summary == "Edit README.md"
        top.message.includes("Task: SPEC-12")
        top.moquiUserId == ec.user.userId
        saved.out.hash == now.hash
        now.hash != opened.hash
        now.text.endsWith("A second line.\n")
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

feature("the git commands of a working tree and the writes outside Save are refused", {
    when() {
        return {
            status: call("org.moqui.devaide.VcsServices.get#Status", {rootId}).errors,
            created: call(WS + "create#File", {rootId, path: "new.txt", text: "x"}).errors
        }
    },
    then({status, created}) {
        status.includes("not a git repository")
        created.includes("no working tree")
    }
})
