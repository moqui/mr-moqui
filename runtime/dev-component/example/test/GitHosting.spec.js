// Git hosting through Moqui (org.moqui.impl.GitServices and GitReviewServices, IronStone Projects P3a) on the real
// database: a branch made at a commit, the tree and a compare, a merge request held by its branch rule until a review
// and a CI status, merged and kept current, a trial merge listing a conflict, and an archived repository refusing a
// write. The rules in full are the Rust tests in crates/moqui-core/src/resource/git/server/hosting_tests.rs (pushes
// over the git command line, webhooks, the REST endpoint).

const GIT = "org.moqui.impl.GitServices"
const REVIEW = "org.moqui.impl.GitReviewServices"
// a suffix keeps each run's repository apart on a database kept between runs
const RUN = Date.now().toString(36).slice(-5).toUpperCase()
const REPO = "SPEC_GIT_" + RUN
const BASE = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n"

const git = (verb, params) => call(`${GIT}.${verb}`, params || {})
const review = (verb, params) => call(`${REVIEW}.${verb}`, params || {})
const user = (userId) => {
    if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
    const row = ec.entity.makeValue("moqui.security.UserAccount")
    row.setFields({ userId: userId, username: userId, userFullName: `${userId} Example` }, true, null, null)
    row.create()
}
// the errors a call left, cleared, so the next call runs
const errorsOf = (verb, service, params) => {
    ec.service.sync().name(`${service}.${verb}`).parameters(params).call()
    const text = ec.message.getErrorsString()
    ec.message.clearErrors()
    return text
}
const commit = (files, message) => git("commit#GitFiles", { repositoryId: REPO, fileMap: files, message: message }).commitId

spec({
    stepwise: true,
    requires: ["git"],
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        try {
            for (const id of ["gitAuthor", "gitReviewer"]) user(id)
            const repo = ec.entity.makeValue("moqui.resource.GitRepository")
            repo.setFields({ repositoryId: REPO, repositoryKind: "db", description: "Spec repository" }, true, null, null)
            repo.create()
            // the reviewer may push to the spec's repository: only such a user's approval counts toward a branch rule (F132 of
            // mr-boss plan/impl-projects.md), which the merge decides as the reviewer, with authorization on
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.create() }
            make("moqui.security.UserGroup", { userGroupId: "SPEC_GIT_" + RUN, description: "Spec git pushers" })
            make("moqui.security.UserGroupMember", { userGroupId: "SPEC_GIT_" + RUN, userId: "gitReviewer", fromDate: new Date(Date.now() - 60000) })
            make("moqui.security.ArtifactGroup", { artifactGroupId: "SPEC_GIT_" + RUN, description: "Spec repository" })
            make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_GIT_" + RUN, artifactName: REPO, artifactTypeEnumId: "AT_GIT_REPOSITORY",
                nameIsPattern: "N", inheritAuthz: "N" })
            make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_GIT_" + RUN, userGroupId: "SPEC_GIT_" + RUN, artifactGroupId: "SPEC_GIT_" + RUN,
                authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

let first
let mergeRequestId

feature("a branch at a commit, the tree with last commits, and a compare", {
    when() {
        ec.user.internalLoginUser("gitAuthor")
        first = commit({ "a.txt": BASE, "src/b.txt": "b\n" }, "First")
        git("create#GitRef", { repositoryId: REPO, refName: "release", revision: first })
        const second = commit({ "a.txt": BASE.replace("one", "ONE") }, "Second")
        const tree = git("get#GitTree", { repositoryId: REPO })
        const compared = git("get#GitCompare", { repositoryId: REPO, base: "release", head: "main", withDiffs: true, checkMergeable: true })
        return { second, tree, compared }
    },
    then({ second, tree, compared }) {
        tree.entryList.map((entry) => entry.name).join(",") == "src,a.txt"
        tree.entryList[1].lastCommit.commitId == second
        tree.entryList[0].lastCommit.commitId == first
        compared.aheadBy == 1
        compared.behindBy == 0
        compared.mergeable == true
        compared.fileList.length == 1
        compared.fileList[0].hunks[0].lines.some((line) => line.text.includes("ONE") && line.newNo == 1)
    },
})

feature("a merge request waits for its review and check, then merges", {
    when() {
        ec.user.internalLoginUser("gitAuthor")
        git("store#GitBranchRule", { repositoryId: REPO, refPattern: "release", requireMergeRequest: "Y",
            requiredApprovals: 1, requiredStatusContexts: "ci/build" })
        const created = review("create#MergeRequest", { repositoryId: REPO, sourceRef: "main", targetRef: "release",
            title: "Release the second commit", reviewerUserIdList: ["gitReviewer"] })
        mergeRequestId = created.mergeRequestId
        const waitingApproval = errorsOf("merge#MergeRequest", REVIEW, { mergeRequestId })
        const direct = errorsOf("merge#GitRefs", GIT, { repositoryId: REPO, source: "main", target: "release" })
        ec.user.internalLoginUser("gitReviewer")
        const waiting = review("find#MergeRequests", { waitingForUserId: "gitReviewer" }).mergeRequestList
            .filter((row) => row.repositoryId === REPO)
        review("submit#MergeRequestReview", { mergeRequestId, decision: "approve", note: "Fine" })
        ec.user.internalLoginUser("gitAuthor")
        const waitingCheck = errorsOf("merge#MergeRequest", REVIEW, { mergeRequestId })
        const head = ec.entity.find("moqui.resource.GitMergeRequest").condition("mergeRequestId", mergeRequestId).one().headCommitId
        git("create#GitCommitStatus", { repositoryId: REPO, commitId: head, state: "success", context: "ci/build",
            target_url: "https://ci.example.com/run/1" })
        const detail = review("get#MergeRequest", { mergeRequestId })
        const merged = review("merge#MergeRequest", { mergeRequestId })
        const row = ec.entity.find("moqui.resource.GitMergeRequest").condition("mergeRequestId", mergeRequestId).one()
        const after = git("get#GitCompare", { repositoryId: REPO, base: "release", head: "main" })
        return { created, waitingApproval, direct, waiting, waitingCheck, detail, merged, row, after }
    },
    then({ created, waitingApproval, direct, waiting, waitingCheck, detail, merged, row, after }) {
        created.mergeRequestNum == 1
        waitingApproval.includes("0 of 1 approvals")
        direct.includes("protected branch")
        waiting.length == 1
        waitingCheck.includes("checks not passed: ci/build")
        detail.canMerge == true
        merged.commitId.length == 40
        row.statusId == "GmrMerged"
        row.mergeCommitId == merged.commitId
        row.mergedByUserId == "gitAuthor"
        after.aheadBy == 0
    },
})

feature("a trial merge lists a conflict; an archived repository refuses a write", {
    when() {
        ec.user.internalLoginUser("gitAuthor")
        git("create#GitRef", { repositoryId: REPO, refName: "clash", revision: "main" })
        commit({ "a.txt": BASE.replace("one", "ONE").replace("four", "four, on main") }, "Main four")
        // commit#GitFiles writes the default branch: make clash the default for one commit
        git("update#GitRepository", { repositoryId: REPO, branch: "clash" })
        commit({ "a.txt": BASE.replace("one", "ONE").replace("four", "four, on clash") }, "Clash four")
        git("update#GitRepository", { repositoryId: REPO, branch: "main", defaultMergeMethodEnumId: "GmmSquash" })
        const trial = git("merge#GitRefs", { repositoryId: REPO, source: "clash", target: "main", checkOnly: true })
        const refusedMerge = errorsOf("merge#GitRefs", GIT, { repositoryId: REPO, source: "clash", target: "main" })
        const archived = git("archive#GitRepository", { repositoryId: REPO })
        const refused = errorsOf("create#GitRef", GIT, { repositoryId: REPO, refName: "late" })
        const listed = git("find#GitRepositories", { repositoryIdList: [REPO] }).repositoryList.length
        git("unarchive#GitRepository", { repositoryId: REPO })
        const repo = ec.entity.find("moqui.resource.GitRepository").condition("repositoryId", REPO).one()
        const updates = git("find#GitUpdates", { repositoryId: REPO, source: "write" }).updateList
        return { trial, refusedMerge, archived, refused, listed, repo, updates }
    },
    then({ trial, refusedMerge, archived, refused, listed, repo, updates }) {
        trial.mergeable == false
        trial.conflictPaths.join(",") == "a.txt"
        refusedMerge.includes("conflicts in a.txt")
        archived.closedMergeRequestCount == 0
        refused.includes("archived")
        listed == 0
        repo.archivedDate == null
        repo.defaultMergeMethodEnumId == "GmmSquash"
        updates.length >= 4
    },
})
