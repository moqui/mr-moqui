/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

/*
 * Git depth in the Workspace (plans/design-dev-aide.md 4.3 to 4.6, P8): the Branches, Tags and Stashes tabs under the
 * Advanced toggle, the Merge requests tab of a root a group server hosts (21.4: mine, waiting for my review, open; Check
 * out, New; Review with the server's threads), a commit's checks (21.5: statuses, runs, a job's log and its failed tests
 * as links, Re-run, Run locally: a job of the working tree's ci.xml in a terminal, not in its image), the Sessions
 * tab (21.8: agents' sessions in worktrees, which are not loaded), the Settings tab (remotes, the hosted repository with Find on the server and Fork, the root's components,
 * its publish branch, Open on the hosting site), a
 * compare of two refs, and the conflict view: ours, the result and theirs side by side, Accept ours, theirs or both on
 * each conflict, Mark resolved and Abort. Each component takes the Workspace (ws) for its transitions, its notes and its
 * refresh. Loaded on first use, as the visual views are; its pure parts are moqui.devAide.pure.
 */
(function (root) {
    "use strict";
    const moqui = root.moqui = root.moqui || {};
    const devAide = moqui.devAide = moqui.devAide || {};
    const pure = devAide.pure = devAide.pure || {};

    // ---- pure ----

    // The conflicts of a text with git's markers: each with its line range (from the <<<<<<< line to the >>>>>>> line,
    // from 0), our lines, the base's (diff3 style, when there) and theirs.
    pure.conflictRegions = (text) => {
        const lines = String(text || "").split("\n");
        const regions = [];
        let open = null;
        for (let i = 0; i < lines.length; i++) {
            const line = lines[i];
            if (!open && line.startsWith("<<<<<<< ")) { open = { start: i, ours: [], base: null, theirs: [], part: "ours", label: line.slice(8) }; continue; }
            if (!open) continue;
            if (line.startsWith("||||||| ") || line === "|||||||") { open.base = []; open.part = "base"; continue; }
            if (line === "=======") { open.part = "theirs"; continue; }
            if (line.startsWith(">>>>>>> ")) {
                open.end = i; open.theirsLabel = line.slice(8); delete open.part;
                regions.push(open); open = null; continue;
            }
            open[open.part].push(line);
        }
        return regions;
    };
    // The text with conflict `index` replaced by our side, theirs, or both (ours then theirs).
    pure.chooseRegion = (text, index, side) => {
        const region = pure.conflictRegions(text)[index];
        if (!region) return text;
        const lines = String(text).split("\n");
        const chosen = side === "ours" ? region.ours : side === "theirs" ? region.theirs : region.ours.concat(region.theirs);
        return lines.slice(0, region.start).concat(chosen, lines.slice(region.end + 1)).join("\n");
    };
    // A web page of a remote's repository on a known host, or null: https and scp-like SSH URLs.
    pure.hostPage = (url) => {
        const text = String(url || "").trim();
        let match = /^https?:\/\/(?:[^@/]+@)?([^/]+)\/(.+?)(?:\.git)?\/?$/.exec(text);
        if (!match) match = /^(?:ssh:\/\/)?git@([^:/]+)[:/](.+?)(?:\.git)?\/?$/.exec(text);
        return match ? "https://" + match[1] + "/" + match[2] : null;
    };
    // A hunk's line range on the side the diff shows, as a label.
    pure.hunkLabel = (hunk, staged) => {
        const start = hunk.newStart, count = hunk.newLines;
        const lines = count === 0 ? "removed lines near " + start : count === 1 ? "line " + start : "lines " + start + "–" + (start + count - 1);
        return (staged ? "staged " : "") + lines + " (−" + hunk.oldLines + " +" + hunk.newLines + ")";
    };

    // ---- components ----

    const date = (text) => String(text || "").slice(0, 10);
    // the color of a check's state chip (21.5)
    // The Assist conversation of this tab's drawer, whose session a worktree is for (21.8); null with none.
    pure.assistConversation = () => {
        const assist = root.moqui && root.moqui.assist;
        const id = assist && assist.state && assist.state.convo && assist.state.convo.conversationId;
        return id ? String(id) : null;
    };
    // Run locally (21.5): the combination of a job a local run takes (its matrix key, the first by default), as text
    // lines to show: its commands in order and its variables; a secret is named, never set.
    pure.localRunLines = (job, matrixKey) => {
        if (!job) return { steps: [], env: [], secrets: [], script: "", digest: "" };
        const combos = job.combinations || [job];
        const combo = combos.find((item) => item.matrixKey === matrixKey) || combos[0] || job;
        return {
            steps: (combo.steps || []).map((step, index) => ({ seq: index + 1, name: step.name || "", run: step.run || "", continueOnError: !!step.continueOnError })),
            env: (combo.env || []).map((pair) => pair[0] + "=" + pair[1]),
            secrets: (job.secrets || []).slice(),
            // the script that runs, whole, and its digest, which the run is held to
            script: combo.script || "", digest: combo.digest || ""
        };
    };
    pure.stateColor = (state) => (state === "failure" ? "negative" : state === "pending" ? "warning" : state === "success" ? "positive" : "grey");

    const branches = {
        name: "mDevGitBranches",
        props: { ws: { type: Object, required: true } },
        computed: { pure: () => pure },
        data() { return { local: [], remote: [], compare: null, loading: false, states: {} }; },
        mounted() { this.load(); },
        methods: {
            load() {
                this.loading = true;
                return this.ws.get("branches", { rootId: this.ws.rootId }).then((answer) => {
                    this.local = answer.local || []; this.remote = answer.remote || []; this.loading = false;
                    // the checks of each branch's head on a hosted repository (21.5)
                    this.ws.commitStates(this.local.concat(this.remote).map((branch) => branch.commit)).then((states) => { this.states = states; });
                }, (error) => { this.loading = false; this.ws.fail(error); });
            },
            done(message) { this.ws.note(message); this.load(); this.ws.refreshStatus(); },
            rename(branch) {
                this.ws.ask("Rename " + branch.name, branch.name).then((name) => {
                    if (name && name !== branch.name) this.ws.post("renameBranch", { rootId: this.ws.rootId, branch: branch.name, newName: name })
                        .then(() => this.done("Renamed to " + name), this.ws.fail);
                });
            },
            remove(branch) {
                this.ws.$q.dialog({ title: "Delete " + branch.name + "?", message: "A branch not merged into the current one is refused unless you delete it anyway.",
                    cancel: true, ok: { label: "Delete", color: "negative" } }).onOk(() => {
                    this.ws.post("deleteBranch", { rootId: this.ws.rootId, branch: branch.name }).then(() => this.done("Deleted " + branch.name), (error) => {
                        this.ws.$q.dialog({ title: "Delete anyway?", message: moqui.devAide.pure.errorText(error) + "\n\nIts commits are lost unless another branch has them.",
                            cancel: true, ok: { label: "Delete anyway", color: "negative" } }).onOk(() =>
                            this.ws.post("deleteBranch", { rootId: this.ws.rootId, branch: branch.name, force: true }).then(() => this.done("Deleted " + branch.name), this.ws.fail));
                    });
                });
            },
            merge(branch, mergeCommit) {
                this.ws.post("mergeBranch", { rootId: this.ws.rootId, branch: branch.name, mergeCommit: !!mergeCommit }).then((answer) => {
                    if (answer.conflict) { this.ws.note("The merge stopped at conflicts: resolve them in Changes", "warning"); this.ws.afterConflict(); return; }
                    this.done("Merged " + branch.name); this.ws.reloadTabs();
                }, (error) => {
                    if (mergeCommit) { this.ws.fail(error); return; }
                    this.ws.$q.dialog({ title: "Not a fast-forward", message: moqui.devAide.pure.errorText(error) + "\n\nMerge with a merge commit?", cancel: true,
                        ok: { label: "Merge commit" } }).onOk(() => this.merge(branch, true));
                });
            },
            openCompare(branch) {
                const base = (this.ws.status && this.ws.status.branch) || "HEAD";
                this.ws.get("compareRefs", { rootId: this.ws.rootId, base, head: branch.name }).then((answer) => {
                    this.compare = { base, head: branch.name, commits: answer.commits || [], files: answer.files || [], diff: null };
                }, this.ws.fail);
            },
            compareDiff(file) {
                const compare = this.compare;
                const side = (rev) => this.ws.get("fileAt", { rootId: this.ws.rootId, rev, path: file.path }).then((answer) => answer.text || "", () => "");
                Promise.all([side(compare.base), side(compare.head)]).then(([original, modified]) => { compare.diff = { path: file.path, original, modified }; });
            }
        },
        template: `
<div class="col row no-wrap m-dev-body" id="DevBranches">
  <div class="m-dev-side column no-wrap scroll q-pa-xs" style="width: 360px">
    <div class="row items-center no-wrap"><span class="text-caption text-weight-medium">Local</span><q-space></q-space>
      <q-btn dense flat no-caps size="sm" icon="add" label="New" @click="ws.newBranch().then(load)"></q-btn></div>
    <div v-for="branch in local" :key="branch.name" class="m-dev-change q-py-xs" :data-branch="branch.name">
      <div class="row items-center no-wrap"><q-icon v-if="branch.current" name="sym_o_check" size="xs" class="q-mr-xs"></q-icon>
        <span class="ellipsis text-weight-medium">{{branch.name}}</span>
        <q-badge v-if="states[branch.commit]" :color="pure.stateColor(states[branch.commit])" class="q-ml-xs cursor-pointer" :label="states[branch.commit]"
            @click="ws.openChecks(branch.commit)" :data-state="states[branch.commit]"></q-badge><q-space></q-space>
        <q-btn dense flat round size="xs" icon="more_vert" :aria-label="'Actions for ' + branch.name"><q-menu><q-list dense style="min-width: 12rem">
          <q-item v-if="!branch.current" clickable v-close-popup @click="ws.switchBranch(branch.name)"><q-item-section>Switch</q-item-section></q-item>
          <q-item v-if="!branch.current" clickable v-close-popup @click="merge(branch)"><q-item-section>Merge into {{ws.status && ws.status.branch}}</q-item-section></q-item>
          <q-item v-if="!branch.current" clickable v-close-popup @click="openCompare(branch)"><q-item-section>Compare with {{ws.status && ws.status.branch}}</q-item-section></q-item>
          <q-item clickable v-close-popup @click="rename(branch)"><q-item-section>Rename</q-item-section></q-item>
          <q-item v-if="!branch.current" clickable v-close-popup @click="remove(branch)"><q-item-section class="text-negative">Delete</q-item-section></q-item>
        </q-list></q-menu></q-btn></div>
      <div class="text-caption m-muted ellipsis">{{branch.upstream ? branch.upstream + (branch.gone ? ' (gone)' : ' ↑' + branch.ahead + ' ↓' + branch.behind) + ' · ' : ''}}{{date(branch.date)}} · {{branch.subject}}</div>
    </div>
    <div class="text-caption text-weight-medium q-mt-md">Remote</div>
    <div v-for="branch in remote" :key="branch.name" class="row items-center no-wrap m-dev-change" :data-branch="branch.name">
      <span class="ellipsis">{{branch.name}}</span><q-space></q-space>
      <q-btn dense flat no-caps size="sm" label="Compare" @click="openCompare(branch)"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Merge" :disable="ws.tier !== 'full'" @click="merge(branch)"></q-btn>
    </div>
  </div>
  <div class="col column no-wrap" v-if="compare" id="DevCompare">
    <div class="text-subtitle2 q-pa-sm">{{compare.head}} against {{compare.base}}: {{compare.commits.length}} commits, {{compare.files.length}} files</div>
    <q-list dense class="scroll" style="max-height: 30%"><q-item v-for="commit in compare.commits" :key="commit.id">
      <q-item-section><q-item-label class="ellipsis">{{commit.subject}}</q-item-label><q-item-label caption>{{commit.shortId}} · {{commit.authorName}} · {{date(commit.date)}}</q-item-label></q-item-section>
    </q-item></q-list>
    <q-separator></q-separator>
    <q-list dense class="scroll" style="max-height: 25%"><q-item v-for="file in compare.files" :key="file.path" clickable @click="compareDiff(file)">
      <q-item-section side class="m-file-status" :class="'m-file-status-' + file.status">{{file.status}}</q-item-section><q-item-section>{{file.path}}</q-item-section>
    </q-item></q-list>
    <m-diff-view v-if="compare.diff" class="col" :original="compare.diff.original" :modified="compare.diff.modified" :file-name="compare.diff.path"></m-diff-view>
  </div>
  <div v-else class="col flex flex-center m-muted">Compare a branch with the current one from its menu.</div>
</div>`
    };
    branches.methods.date = date;

    const tags = {
        name: "mDevGitTags",
        props: { ws: { type: Object, required: true } },
        data() { return { tags: [], form: { tag: "", message: "" } }; },
        mounted() { this.load(); },
        methods: {
            date,
            load() { return this.ws.get("tags", { rootId: this.ws.rootId }).then((answer) => { this.tags = answer.tags || []; }, this.ws.fail); },
            create() {
                this.ws.post("createTag", { rootId: this.ws.rootId, tag: this.form.tag, message: this.form.message }).then(() => {
                    this.ws.note("Tagged " + this.form.tag); this.form = { tag: "", message: "" }; this.load();
                }, this.ws.fail);
            },
            push(tag) { this.ws.network("pushTag", { tag: tag.name }).then(() => this.load()); },
            remove(tag) {
                this.ws.$q.dialog({ title: "Delete the tag " + tag.name + " here?", message: "A tag pushed already stays on the remote.", cancel: true,
                    ok: { label: "Delete", color: "negative" } }).onOk(() => this.ws.post("deleteTag", { rootId: this.ws.rootId, tag: tag.name }).then(() => this.load(), this.ws.fail));
            }
        },
        template: `
<div class="col column no-wrap scroll q-pa-sm m-dev-body" id="DevTags">
  <div class="row items-end q-gutter-sm">
    <q-input dense outlined v-model="form.tag" label="New tag" style="width: 12rem" id="DevTagName"></q-input>
    <q-input dense outlined v-model="form.message" label="Message (annotated when given)" class="col"></q-input>
    <q-btn dense unelevated no-caps color="primary" label="Tag HEAD" :disable="!form.tag || !ws.canWrite" @click="create"></q-btn>
  </div>
  <q-list dense class="q-mt-sm"><q-item v-for="tag in tags" :key="tag.name" :data-tag="tag.name">
    <q-item-section><q-item-label>{{tag.name}} <span class="m-muted text-caption">{{tag.annotated ? 'annotated' : 'lightweight'}}</span></q-item-label>
      <q-item-label caption>{{String(tag.commit).slice(0, 9)}} · {{date(tag.date)}} · {{tag.subject}}</q-item-label></q-item-section>
    <q-item-section side><div class="row no-wrap">
      <q-btn dense flat no-caps size="sm" label="Push" :disable="!ws.canWrite" @click="push(tag)"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Delete" :disable="!ws.canWrite" @click="remove(tag)"></q-btn></div></q-item-section>
  </q-item></q-list>
  <div v-if="!tags.length" class="m-muted q-pa-md">No tags.</div>
</div>`
    };

    const stashes = {
        name: "mDevGitStashes",
        props: { ws: { type: Object, required: true } },
        data() { return { stashes: [], form: { message: "", untracked: false } }; },
        mounted() { this.load(); },
        methods: {
            date,
            load() { return this.ws.get("stashes", { rootId: this.ws.rootId }).then((answer) => { this.stashes = answer.stashes || []; }, this.ws.fail); },
            after(answer, what) {
                if (answer && answer.conflict) { this.ws.note("The stash met conflicts: git kept it; resolve the files in Changes", "warning"); this.ws.afterConflict(); }
                else this.ws.note(what);
                this.load(); this.ws.refreshStatus(); this.ws.reloadTabs();
            },
            create() {
                this.ws.post("createStash", { rootId: this.ws.rootId, message: this.form.message, untracked: this.form.untracked })
                    .then((answer) => { this.form.message = ""; this.after(answer, "Stashed"); }, this.ws.fail);
            },
            restore(stash, pop) {
                this.ws.post(pop ? "popStash" : "applyStash", { rootId: this.ws.rootId, index: stash.index }).then((answer) => this.after(answer, pop ? "Popped" : "Applied"), this.ws.fail);
            },
            drop(stash) {
                this.ws.$q.dialog({ title: "Drop " + stash.ref + "?", message: stash.message, cancel: true, ok: { label: "Drop", color: "negative" } })
                    .onOk(() => this.ws.post("dropStash", { rootId: this.ws.rootId, index: stash.index }).then(() => this.load(), this.ws.fail));
            }
        },
        template: `
<div class="col column no-wrap scroll q-pa-sm m-dev-body" id="DevStashes">
  <div class="row items-center q-gutter-sm">
    <q-input dense outlined v-model="form.message" label="Message" class="col"></q-input>
    <q-checkbox dense v-model="form.untracked" label="Untracked files too"></q-checkbox>
    <q-btn dense unelevated no-caps color="primary" label="Stash changes" :disable="!ws.canWrite || !ws.changeCount" @click="create" id="DevStash"></q-btn>
  </div>
  <q-list dense class="q-mt-sm"><q-item v-for="stash in stashes" :key="stash.ref">
    <q-item-section><q-item-label>{{stash.message}}</q-item-label><q-item-label caption>{{stash.ref}} · {{date(stash.date)}}</q-item-label></q-item-section>
    <q-item-section side><div class="row no-wrap">
      <q-btn dense flat no-caps size="sm" label="Apply" :disable="!ws.canWrite" @click="restore(stash, false)"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Pop" :disable="!ws.canWrite" @click="restore(stash, true)"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Drop" :disable="!ws.canWrite" @click="drop(stash)"></q-btn></div></q-item-section>
  </q-item></q-list>
  <div v-if="!stashes.length" class="m-muted q-pa-md">No stashes.</div>
</div>`
    };

    const settings = {
        name: "mDevGitSettings",
        props: { ws: { type: Object, required: true } },
        data() { return { form: { name: "", url: "" } }; },
        computed: {
            remotes() { return (this.ws.status && this.ws.status.remotes) || []; },
            rootInfo() { return this.ws.root || {}; },
            hostedServer() {
                const url = (this.ws.hosted && this.ws.hosted.cloneUrls && this.ws.hosted.cloneUrls.httpsUrl) || "";
                return (url.split("://")[1] || url).split("/")[0];
            },
            rights() {
                const access = (this.ws.hosted && this.ws.hosted.access) || {};
                return access.canUpdate ? "push" : access.canCreate ? "push new branches" : "view and propose for review";
            }
        },
        methods: {
            hostPage: (url) => pure.hostPage(url),
            findHosted() {
                this.ws.post("hosting", { rootId: this.ws.rootId }).then((answer) => {
                    this.ws.note(answer.hosted ? "Hosted at " + answer.hosted.repository.repositoryPath : "No connected server hosts it", answer.hosted ? "positive" : "warning");
                    this.ws.loadRoots();
                }, this.ws.fail);
            },
            fork() {
                this.ws.post("fork", { rootId: this.ws.rootId }).then((answer) => {
                    this.ws.note("Forked as " + (answer.repositoryPath || answer.url) + ": the remote fork"); this.ws.refreshStatus();
                }, this.ws.fail);
            },
            add() {
                this.ws.post("addRemote", { rootId: this.ws.rootId, name: this.form.name, url: this.form.url })
                    .then(() => { this.form = { name: "", url: "" }; this.ws.refreshStatus(); }, this.ws.fail);
            },
            setUrl(remote) {
                this.ws.ask("URL of " + remote.name, remote.url).then((url) => {
                    if (url && url !== remote.url) this.ws.post("setRemoteUrl", { rootId: this.ws.rootId, name: remote.name, url }).then(() => this.ws.refreshStatus(), this.ws.fail);
                });
            },
            remove(remote) {
                this.ws.$q.dialog({ title: "Remove the remote " + remote.name + "?", cancel: true, ok: { label: "Remove", color: "negative" } })
                    .onOk(() => this.ws.post("removeRemote", { rootId: this.ws.rootId, name: remote.name }).then(() => this.ws.refreshStatus(), this.ws.fail));
            },
            prune(remote) { this.ws.network("pruneRemote", { name: remote.name }); }
        },
        template: `
<div class="col column no-wrap scroll q-pa-sm m-dev-body" id="DevRepoSettings">
  <div class="text-subtitle2">Remotes</div>
  <q-list dense><q-item v-for="remote in remotes" :key="remote.name" :data-remote="remote.name">
    <q-item-section><q-item-label>{{remote.name}}</q-item-label><q-item-label caption>{{remote.url}}</q-item-label></q-item-section>
    <q-item-section side><div class="row no-wrap">
      <q-btn v-if="hostPage(remote.url)" dense flat no-caps size="sm" label="Open on host" :href="hostPage(remote.url)" target="_blank" type="a"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Prune" :disable="!ws.canWrite" @click="prune(remote)"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Set URL" :disable="ws.tier !== 'full'" @click="setUrl(remote)"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Remove" :disable="ws.tier !== 'full'" @click="remove(remote)"></q-btn></div></q-item-section>
  </q-item></q-list>
  <div class="row items-center q-gutter-sm q-mt-xs" v-if="ws.tier === 'full'">
    <q-input dense outlined v-model="form.name" label="Name" style="width: 9rem"></q-input>
    <q-input dense outlined v-model="form.url" label="URL" class="col"></q-input>
    <q-btn dense flat no-caps label="Add remote" :disable="!form.name || !form.url" @click="add"></q-btn>
  </div>
  <div class="text-subtitle2 q-mt-md">On a group server</div>
  <div v-if="ws.hosted" class="text-caption" id="DevHosted">{{ws.hosted.repositoryPath}} on {{hostedServer}} by {{ws.root.hosted.remote}}:
    default branch {{ws.hosted.defaultBranch}}; you may {{rights}}{{ws.hosted.keptBy ? '; kept by ' + ws.hosted.keptBy.application : ''}}</div>
  <div v-else class="text-caption m-muted">No connected group server hosts this repository's remotes (Dev AIDE Settings: Group servers).</div>
  <div class="row q-gutter-sm">
    <q-btn dense flat no-caps label="Find on the server" @click="findHosted" id="DevFindHosted"></q-btn>
    <q-btn v-if="ws.hosted" dense flat no-caps label="Fork" :disable="!ws.canWrite" @click="fork" id="DevFork">
      <q-tooltip>Your own copy on the server, added as the remote fork, where the server makes forks</q-tooltip></q-btn>
  </div>
  <div class="text-subtitle2 q-mt-md">Components</div>
  <div class="text-caption" v-for="component in rootInfo.components || []" :key="component.name">{{component.name}}{{component.site ? ' (site)' : ''}} · {{component.path || ''}}</div>
  <div class="text-subtitle2 q-mt-md">Publish</div>
  <div class="text-caption">{{rootInfo.publishBranch ? 'Publish pushes to ' + rootInfo.publishBranch + (rootInfo.publishBranchRequired ? ', and Push is off for this repository' : '') : 'Publish pushes to the current branch.'}}
    <a href="/qapps/dev/Settings">Dev AIDE Settings</a> has the labels, read only flags and publish branches.</div>
  <div class="row q-gutter-sm q-mt-md">
    <q-btn dense flat no-caps icon="sym_o_description" label=".gitignore" @click="ws.openFile('.gitignore'); ws.tab = 'code'"></q-btn>
  </div>
</div>`
    };

    // The conflict view: our side and theirs read only beside the result, which is the file's text with its markers,
    // editable; each conflict's chips take a side into the result, and Mark resolved saves and stages it.
    const conflict = {
        name: "mDevGitConflict",
        props: { ws: { type: Object, required: true }, path: { type: String, required: true } },
        data() { return { sides: null, result: "", saving: false }; },
        computed: {
            regions() { return pure.conflictRegions(this.result); },
            narrow() { return !!this.ws.narrow; }
        },
        watch: { path() { this.load(); } },
        mounted() { this.load(); },
        methods: {
            load() {
                this.ws.get("conflictSides", { rootId: this.ws.rootId, path: this.path }).then((answer) => {
                    this.sides = answer; this.result = answer.result || "";
                }, this.ws.fail);
            },
            choose(index, side) { this.result = pure.chooseRegion(this.result, index, side); },
            resolve() {
                this.saving = true;
                this.ws.post("resolveConflict", { rootId: this.ws.rootId, path: this.path, text: this.result, baseHash: this.sides.hash }).then((answer) => {
                    this.saving = false;
                    if (answer.conflict) { this.ws.note(this.path + " changed on disk: look again", "warning"); this.load(); return; }
                    this.ws.note(this.path + " resolved and staged"); this.ws.refreshStatus(); this.ws.conflictPath = null; this.ws.reloadTabs();
                }, (error) => { this.saving = false; this.ws.fail(error); });
            },
            abort() {
                const operation = this.sides && this.sides.operation;
                this.ws.$q.dialog({ title: "Abort the " + (operation || "operation") + "?", message: "The files go back to how they were before it.", cancel: true,
                    ok: { label: "Abort", color: "negative" } }).onOk(() => this.ws.post("abortOperation", { rootId: this.ws.rootId }).then(() => {
                    this.ws.note("Aborted"); this.ws.conflictPath = null; this.ws.refreshStatus(); this.ws.reloadTabs(); this.ws.treeKey++;
                }, this.ws.fail));
            }
        },
        template: `
<div class="col column no-wrap" id="DevConflict" :data-path="path">
  <div class="row items-center no-wrap q-gutter-x-sm q-px-sm q-py-xs">
    <span class="text-subtitle2 ellipsis">{{path}}</span>
    <span class="text-caption m-muted" v-if="sides && sides.operation">{{sides.operation}} in progress</span>
    <q-space></q-space>
    <div v-for="(region, index) in regions" :key="index" class="row items-center no-wrap m-dev-conflict-chip q-px-xs" :data-region="index"
        style="border: 1px solid var(--m-border, rgba(0,0,0,.2)); border-radius: 4px">
      <span class="text-caption q-mr-xs">Conflict {{index + 1}}</span>
      <q-btn dense flat no-caps size="sm" label="Ours" @click="choose(index, 'ours')"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Theirs" @click="choose(index, 'theirs')"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Both" @click="choose(index, 'both')"></q-btn></div>
    <q-btn dense unelevated no-caps color="primary" label="Mark resolved" :disable="!!regions.length || !ws.canWrite" :loading="saving" @click="resolve" id="DevMarkResolved">
      <q-tooltip v-if="regions.length">Take a side of each conflict, or edit the result, first</q-tooltip></q-btn>
    <q-btn v-if="sides && sides.operation && sides.operation !== 'stash' && sides.operation !== 'rebase'" dense flat no-caps label="Abort" @click="abort" id="DevAbortOperation"></q-btn>
  </div>
  <div v-if="sides && sides.operation === 'stash'" class="text-caption m-muted q-px-sm">A stash applied has no abort: git kept the stash.</div>
  <div class="col row no-wrap" v-if="sides">
    <div class="col column no-wrap" v-if="!narrow"><div class="text-caption q-px-sm">Ours</div>
      <m-code-editor class="col" :model-value="sides.ours" :file-name="path" read-only height="100%"></m-code-editor></div>
    <div class="col column no-wrap"><div class="text-caption q-px-sm">Result</div>
      <m-code-editor class="col" v-model="result" :file-name="path" height="100%" id="DevConflictResult"></m-code-editor></div>
    <div class="col column no-wrap" v-if="!narrow"><div class="text-caption q-px-sm">Theirs</div>
      <m-code-editor class="col" :model-value="sides.theirs" :file-name="path" read-only height="100%"></m-code-editor></div>
  </div>
</div>`
    };

    // The merge requests of the hosted repository (21.4): mine, waiting for my review, all open; Check out fetches one into
    // a local branch; New opens one of the current branch. Merging is on the server's page (question 17).
    const mergeRequests = {
        name: "mDevGitMergeRequests",
        props: { ws: { type: Object, required: true } },
        data() { return { filter: "open", items: [], loading: false, error: "", review: null }; },
        mounted() { this.load(); },
        watch: { filter() { this.load(); } },
        methods: {
            date,
            // Review (21.4): the changes in the merge view with the server's threads on their lines; reply, resolve, a new
            // comment on a line and a review. Each write is made on the server as you.
            openReview(item) {
                this.review = { item, files: [], threads: [], file: null, loading: true, error: "", available: true, reply: {}, line: "", text: "",
                    decision: "comment", body: "", base: "", head: "" };
                this.ws.get("mergeRequestChanges", { rootId: this.ws.rootId, mergeRequestNum: item.mergeRequestNum }).then((answer) => {
                    Object.assign(this.review, { files: answer.files || [], threads: answer.threads || [], loading: false, available: answer.threadsAvailable !== false,
                        base: answer.baseCommitId || "", head: answer.headCommitId || "",
                        error: answer.diffRefused ? "The server shows no changes of this merge request" + (answer.reason ? " (" + answer.reason + ")" : "") : "" });
                }, (error) => { this.review.loading = false; this.review.error = pure.errorText ? pure.errorText(error) : String(error); });
            },
            openReviewFile(file) {
                const review = this.review;
                this.ws.get("mergeRequestFile", { rootId: this.ws.rootId, mergeRequestNum: review.item.mergeRequestNum, path: file.path,
                    baseCommitId: review.base, headCommitId: review.head }).then((answer) => {
                    review.file = { path: file.path, original: answer.original || "", modified: answer.modified || "" };
                }, this.ws.fail);
            },
            // a server's HTML is shown as its text: nothing a server sends runs on this instance's page
            plain(html) {
                try { return new DOMParser().parseFromString(String(html || ""), "text/html").body.textContent || ""; } catch (error) { return String(html || ""); }
            },
            fileThreads(path) { return ((this.review && this.review.threads) || []).filter((thread) => !path || thread.path === path); },
            reloadThreads() {
                const review = this.review;
                return this.ws.get("reviewThreads", { rootId: this.ws.rootId, mergeRequestNum: review.item.mergeRequestNum }).then((answer) => {
                    review.threads = answer.threads || [];
                }, this.ws.fail);
            },
            reply(thread) {
                const review = this.review, body = review.reply[thread.threadId];
                if (!body) return;
                this.ws.post("reviewComment", { rootId: this.ws.rootId, mergeRequestNum: review.item.mergeRequestNum, threadId: thread.threadId, body }).then((answer) => {
                    review.reply[thread.threadId] = ""; this.ws.note(answer.held ? "Sent: it waits for a moderator" : "Replied on the server"); this.reloadThreads();
                }, this.ws.fail);
            },
            comment() {
                const review = this.review;
                if (!review.text) return;
                const params = { rootId: this.ws.rootId, mergeRequestNum: review.item.mergeRequestNum, body: review.text, commitId: review.head };
                if (review.file) { params.path = review.file.path; if (Number(review.line) > 0) params.newLine = Number(review.line); }
                this.ws.post("reviewComment", params).then((answer) => {
                    review.text = ""; review.line = ""; this.ws.note(answer.held ? "Sent: it waits for a moderator" : "Commented on the server"); this.reloadThreads();
                }, this.ws.fail);
            },
            resolve(thread, resolved) {
                this.ws.post("resolveThread", { rootId: this.ws.rootId, mergeRequestNum: this.review.item.mergeRequestNum, threadId: thread.threadId, resolved })
                    .then(() => this.reloadThreads(), this.ws.fail);
            },
            submitReview() {
                const review = this.review;
                this.ws.post("submitReview", { rootId: this.ws.rootId, mergeRequestNum: review.item.mergeRequestNum, decision: review.decision, body: review.body }).then(() => {
                    review.body = ""; this.ws.note("Review sent: " + review.decision.replace("_", " ")); this.load();
                }, this.ws.fail);
            },
            load() {
                this.loading = true; this.error = "";
                return this.ws.get("mergeRequests", { rootId: this.ws.rootId, filter: this.filter }).then((answer) => {
                    this.loading = false; this.items = answer.mergeRequests || [];
                }, (error) => { this.loading = false; this.items = []; this.error = pure.errorText ? pure.errorText(error) : String(error); });
            },
            // a link from the server only when it is http(s); the server's answer is filtered too (review.rs page_url)
            pageUrl: (url) => (typeof url === "string" && /^https?:\/\//i.test(url) ? url : null),
            checkout(item) {
                // its code runs here once checked out (hot reload): said before, and again after
                if (!window.confirm("Check out !" + item.mergeRequestNum + "? This is someone else's code: the file watcher reloads it into this instance, where it runs with your rights. Read the changes first.")) return;
                this.ws.post("checkoutMergeRequest", { rootId: this.ws.rootId, mergeRequestNum: item.mergeRequestNum }).then((answer) => {
                    this.ws.note("On " + answer.branch + (answer.warning ? ": " + answer.warning : ""), "warning"); this.ws.refreshStatus(); this.ws.reloadTabs();
                }, this.ws.fail);
            },
            create() {
                const branch = this.ws.status && this.ws.status.branch;
                if (branch) this.ws.openMergeRequest(branch).then((made) => { if (made) this.load(); });
            }
        },
        template: `
<div class="col column no-wrap scroll q-pa-sm m-dev-body" id="DevMergeRequests">
  <div class="row items-center q-gutter-sm">
    <q-btn-toggle dense flat no-caps v-model="filter" :options="[{ label: 'Open', value: 'open' }, { label: 'Mine', value: 'mine' }, { label: 'For my review', value: 'review' }, { label: 'All', value: 'all' }]"></q-btn-toggle>
    <q-space></q-space>
    <q-btn dense flat no-caps label="Refresh" :loading="loading" @click="load"></q-btn>
    <q-btn dense unelevated no-caps color="primary" label="New merge request" :disable="!ws.canWrite || !ws.mayPush" @click="create" id="DevNewMergeRequest">
      <q-tooltip>Of the current branch, once pushed, into {{ws.hosted && ws.hosted.defaultBranch}}</q-tooltip></q-btn>
  </div>
  <div v-if="error" class="text-negative q-pa-sm">{{error}}</div>
  <q-list dense separator class="q-mt-sm"><q-item v-for="item in items" :key="item.mergeRequestNum" :data-merge-request="item.mergeRequestNum">
    <q-item-section><q-item-label>!{{item.mergeRequestNum}} {{item.title}}</q-item-label>
      <q-item-label caption>{{item.sourceRef}} → {{item.targetRef}} · {{item.author}} · {{item.state}}{{item.checkState ? ' · checks ' + item.checkState : ''}}</q-item-label></q-item-section>
    <q-item-section side><div class="row no-wrap">
      <q-btn v-if="pageUrl(item.url)" dense flat no-caps size="sm" label="Open on server" :href="pageUrl(item.url)" target="_blank" rel="noopener noreferrer" type="a"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Review" @click="openReview(item)" :data-review="item.mergeRequestNum"></q-btn>
      <q-btn dense flat no-caps size="sm" label="Check out" :disable="!ws.canWrite" @click="checkout(item)"></q-btn></div></q-item-section>
  </q-item></q-list>
  <div v-if="!items.length && !loading && !error" class="m-muted q-pa-md">No merge requests here.</div>
  <div v-if="review" class="column no-wrap q-mt-md" id="DevReview">
    <div class="row items-center"><span class="text-subtitle2">Review !{{review.item.mergeRequestNum}} {{review.item.title}}</span><q-space></q-space>
      <q-btn dense flat no-caps size="sm" label="Close" @click="review = null"></q-btn></div>
    <div v-if="review.error" class="text-negative">{{review.error}}</div>
    <div v-if="!review.available" class="text-caption m-muted">This server keeps no review threads.</div>
    <div class="row no-wrap">
      <q-list dense class="m-dev-side" style="width: 260px"><q-item v-for="file in review.files" :key="file.path" clickable @click="openReviewFile(file)"
          :active="review.file && review.file.path === file.path">
        <q-item-section>{{file.path}}</q-item-section>
        <q-item-section side v-if="fileThreads(file.path).length"><q-badge :label="fileThreads(file.path).length"></q-badge></q-item-section></q-item></q-list>
      <m-diff-view v-if="review.file" class="col" :original="review.file.original" :modified="review.file.modified" :file-name="review.file.path"></m-diff-view>
    </div>
    <div v-for="thread in fileThreads(review.file && review.file.path)" :key="thread.threadId" class="q-pa-xs m-dev-change" :data-thread="thread.threadId">
      <div class="text-caption text-weight-medium">{{thread.path || 'The merge request'}}{{thread.newLine ? ':' + thread.newLine : thread.oldLine ? ' (old) :' + thread.oldLine : ''}}
        {{thread.resolved ? '· resolved' : ''}}{{thread.outdated ? '· outdated' : ''}}</div>
      <div v-for="comment in thread.comments" :key="comment.commentId" class="text-caption"><b>{{comment.author}}</b>:
        <span style="white-space: pre-wrap">{{comment.bodyFormat === 'html' ? plain(comment.body) : comment.body}}</span></div>
      <div class="row items-center no-wrap q-gutter-x-xs">
        <q-input dense outlined class="col" v-model="review.reply[thread.threadId]" placeholder="Reply"></q-input>
        <q-btn dense flat no-caps size="sm" label="Reply" @click="reply(thread)"></q-btn>
        <q-btn dense flat no-caps size="sm" :label="thread.resolved ? 'Reopen' : 'Resolve'" @click="resolve(thread, !thread.resolved)"></q-btn></div>
    </div>
    <div class="row items-center no-wrap q-gutter-x-xs q-mt-xs">
      <q-input v-if="review.file" dense outlined style="width: 6rem" v-model="review.line" label="Line"></q-input>
      <q-input dense outlined class="col" v-model="review.text" :label="review.file ? 'A comment on ' + review.file.path : 'A comment on the merge request'"></q-input>
      <q-btn dense flat no-caps size="sm" label="Comment" :disable="!review.text" @click="comment" id="DevReviewComment"></q-btn></div>
    <div class="row items-center no-wrap q-gutter-x-xs q-mt-xs">
      <q-select dense outlined emit-value map-options style="width: 12rem" v-model="review.decision"
          :options="[{ label: 'Comment', value: 'comment' }, { label: 'Approve', value: 'approve' }, { label: 'Request changes', value: 'request_changes' }]"></q-select>
      <q-input dense outlined class="col" v-model="review.body" label="Review"></q-input>
      <q-btn dense unelevated no-caps color="primary" label="Send review" @click="submitReview" id="DevSubmitReview"></q-btn></div>
    <div class="text-caption m-muted">Merging is on the server's page.</div>
  </div>
</div>`
    };

    // A commit's checks (21.5): its statuses, its pipeline runs and jobs; a job's log, followed while it runs, and its
    // failed tests as links to the file and line; Re-run for a user who may.
    const checks = {
        name: "mDevGitChecks",
        props: { ws: { type: Object, required: true }, commitId: { type: String, required: true } },
        data() { return { loading: true, state: "", statuses: [], runs: [], job: null, error: "", local: null, localJobs: null, localNote: "", localError: "" }; },
        computed: { pure: () => pure, localLines() { return this.local ? pure.localRunLines(this.local.job, this.local.matrixKey) : null; } },
        mounted() { this.load(); },
        beforeUnmount() { this.stopFollow(); },
        methods: {
            load() {
                this.loading = true;
                return this.ws.get("checks", { rootId: this.ws.rootId, commitId: this.commitId }).then((answer) => {
                    Object.assign(this, { loading: false, state: answer.state || "", statuses: answer.statuses || [], runs: answer.runs || [] });
                }, (error) => { this.loading = false; this.error = pure.errorText ? pure.errorText(error) : String(error); });
            },
            pageUrl: (url) => (typeof url === "string" && /^https?:\/\//i.test(url) ? url : null),
            openJob(job) {
                this.stopFollow();
                this.job = { jobRunId: job.jobRunId, name: job.jobName, text: "", offset: 0, tests: [], finished: false };
                this.more();
            },
            more() {
                const job = this.job;
                if (!job) return;
                this.ws.get("job", { rootId: this.ws.rootId, jobRunId: job.jobRunId, offset: job.offset }).then((answer) => {
                    if (this.job !== job) return;
                    job.text += answer.text || ""; job.offset = answer.nextOffset || job.offset; job.tests = answer.failedTests || []; job.finished = answer.finished !== false;
                    // a running job is followed
                    if (!job.finished) this.timer = setTimeout(() => this.more(), 2000);
                }, this.ws.fail);
            },
            stopFollow() { if (this.timer) clearTimeout(this.timer); this.timer = null; },
            openTest(test) { if (test.link) { this.ws.openFile(test.link.path, test.link.line || 0); this.$emit("close"); } },
            // Run locally (21.5): the jobs of the working tree's ci.xml; one, as it would run, before it runs in a terminal
            openLocal(pipeline, jobName, matrixKey) {
                this.ws.get("localJobs", { rootId: this.ws.rootId }).then((answer) => {
                    this.localJobs = answer.jobs || []; this.localNote = answer.note || ""; this.localError = answer.error || "";
                    const job = pipeline ? this.localJobs.find((item) => item.pipeline === pipeline && item.job === jobName) : null;
                    if (pipeline && !job) this.localError = "The working tree's ci.xml has no job " + pipeline + "/" + jobName;
                    this.local = job ? { job, matrixKey: matrixKey && (job.matrixKeys || []).includes(matrixKey) ? matrixKey : ((job.matrixKeys || [])[0] || "") } : null;
                }, this.ws.fail);
            },
            pickLocal(job) { this.local = { job, matrixKey: (job.matrixKeys || [])[0] || "" }; },
            startLocal() {
                const local = this.local;
                if (!local) return;
                const lines = pure.localRunLines(local.job, local.matrixKey);
                this.ws.post("startLocalRun", { rootId: this.ws.rootId, pipeline: local.job.pipeline, job: local.job.job, matrixKey: local.matrixKey,
                    digest: lines.digest }).then((answer) => {
                    if (!answer.sessionId) return;
                    const terminal = moqui.terminal;
                    if (terminal && terminal.refresh && terminal.showSomewhere) terminal.refresh().then(() => terminal.showSomewhere(answer.sessionId));
                    else root.open("/qapps/Terminal", "_blank", "noopener");
                    this.ws.note("Running " + local.job.job + " in a terminal, on this machine: not in the job's image");
                    this.local = null;
                }, this.ws.fail);
            },
            rerun(run, job) {
                // the run's number with a job too: a repository an application keeps retries a job of a run
                const params = { rootId: this.ws.rootId, runNum: run.runNum };
                if (job) params.jobRunId = job.jobRunId;
                this.ws.post("rerunCheck", params).then(() => { this.ws.note(job ? "Job re-run" : "Run re-run"); this.load(); }, this.ws.fail);
            }
        },
        emits: ["close"],
        template: `
<div class="column no-wrap q-pa-sm" id="DevChecks">
  <div class="row items-center"><span class="text-subtitle2">Checks of {{commitId.slice(0, 9)}}</span>
    <q-badge v-if="state" :color="pure.stateColor(state)" :label="state" class="q-ml-sm"></q-badge><q-space></q-space>
    <q-btn dense flat no-caps size="sm" label="Run locally…" @click="openLocal()" id="DevRunLocallyAll"></q-btn>
    <q-btn dense flat no-caps size="sm" label="Refresh" :loading="loading" @click="load"></q-btn></div>
  <div v-if="error" class="text-negative">{{error}}</div>
  <div v-if="localJobs" class="q-pa-sm q-mb-sm bg-grey-2 text-black" id="DevLocalRun">
    <div class="text-caption text-weight-medium">Run locally</div>
    <div class="text-caption">{{localNote}}</div>
    <div v-if="localError" class="text-negative text-caption">{{localError}}</div>
    <div v-if="!local" class="row q-gutter-xs q-mt-xs">
      <q-btn v-for="item in localJobs" :key="item.pipeline + '/' + item.job" dense outline no-caps size="sm" :label="item.pipeline + '/' + item.job" @click="pickLocal(item)"></q-btn>
    </div>
    <template v-else>
      <div class="row items-center q-gutter-x-sm text-caption"><span class="text-weight-medium">{{local.job.pipeline}}/{{local.job.job}}</span>
        <span class="m-muted">image {{local.job.image}}, which this run does not use</span>
        <q-select v-if="(local.job.matrixKeys || []).length > 1" dense outlined options-dense v-model="local.matrixKey" :options="local.job.matrixKeys" label="Combination" style="min-width: 10rem"></q-select></div>
      <div v-for="step in localLines.steps" :key="step.seq" class="text-caption"><span class="m-muted">{{step.seq}}. {{step.name}}</span>
        <code class="m-dev-mono q-ml-xs">{{step.run}}</code><span v-if="step.continueOnError" class="m-muted"> (continues on error)</span></div>
      <div v-if="localLines.env.length" class="text-caption m-muted">Variables: <code class="m-dev-mono">{{localLines.env.join('  ')}}</code></div>
      <div v-if="localLines.secrets.length" class="text-caption">Not set (secrets are never passed): {{localLines.secrets.join(', ')}}</div>
      <q-expansion-item dense dense-toggle label="The script that runs" header-class="text-caption q-px-none" id="DevLocalScript">
        <pre class="m-dev-mono q-ma-none scroll text-caption" style="max-height: 30vh; white-space: pre-wrap">{{localLines.script}}</pre></q-expansion-item>
      <div class="row q-gutter-x-sm q-mt-xs">
        <q-btn dense unelevated no-caps color="primary" size="sm" label="Run in a terminal" @click="startLocal" id="DevRunLocallyStart"></q-btn>
        <q-btn dense flat no-caps size="sm" label="Cancel" @click="local = null; localJobs = null"></q-btn></div>
    </template>
  </div>
  <div v-for="status in statuses" :key="status.context" class="row items-center no-wrap text-caption">
    <q-badge :color="pure.stateColor(status.state)" :label="status.state" class="q-mr-xs"></q-badge>{{status.context}}
    <span class="m-muted q-ml-xs ellipsis">{{status.description}}</span>
    <q-btn v-if="pageUrl(status.url)" dense flat no-caps size="sm" label="Open" :href="pageUrl(status.url)" target="_blank" rel="noopener noreferrer" type="a"></q-btn></div>
  <div v-for="run in runs" :key="run.runNum" class="q-mt-sm">
    <div class="row items-center no-wrap text-caption text-weight-medium">#{{run.runNum}} {{run.pipelineName}}
      <q-badge :color="pure.stateColor(run.state)" :label="run.statusId" class="q-ml-xs"></q-badge><q-space></q-space>
      <q-btn v-if="run.canUpdate" dense flat no-caps size="sm" label="Re-run" @click="rerun(run)"></q-btn></div>
    <div v-for="job in run.jobs" :key="job.jobRunId" class="row items-center no-wrap text-caption q-pl-md">
      <q-badge :color="pure.stateColor(job.state)" :label="job.statusId" class="q-mr-xs"></q-badge>
      <a href="#" @click.prevent="openJob(job)">{{job.jobName}}{{job.matrixKey ? ' (' + job.matrixKey + ')' : ''}}</a>
      <span v-if="job.testFailedCount" class="text-negative q-ml-xs">{{job.testFailedCount}} failed</span><q-space></q-space>
      <q-btn dense flat no-caps size="sm" label="Run locally" @click="openLocal(run.pipelineName, job.jobName, job.matrixKey)"></q-btn>
      <q-btn v-if="run.canUpdate" dense flat no-caps size="sm" label="Retry" @click="rerun(run, job)"></q-btn></div>
  </div>
  <div v-if="job" class="q-mt-sm" id="DevJobLog">
    <div class="text-caption text-weight-medium">{{job.name}}{{job.finished ? '' : ' · running'}}</div>
    <div v-for="test in job.tests" :key="test.suiteName + test.testName" class="text-caption">
      <a v-if="test.link" href="#" @click.prevent="openTest(test)" :data-test-link="test.link.path + ':' + (test.link.line || '')">{{test.link.path}}{{test.link.line ? ':' + test.link.line : ''}}</a>
      <span v-else class="m-muted">{{test.filePath}}</span> {{test.testName}}: {{test.message}}</div>
    <pre class="m-dev-mono q-ma-none scroll" style="max-height: 40vh; white-space: pre-wrap">{{job.text}}</pre>
  </div>
  <div v-if="!loading && !statuses.length && !runs.length && !error" class="m-muted">No checks of this commit.</div>
</div>`
    };

    // The agents' sessions of a root (21.8): each worktree with its branch, its Assist conversation and its uncommitted
    // changes; a new one for this tab's Assist conversation; Open; Remove (its uncommitted changes discarded only
    // after a confirm; its branch stays). A worktree is not loaded (question 19), which the panel says.
    const sessions = {
        name: "mDevGitSessions",
        props: { ws: { type: Object, required: true } },
        data() { return { items: [], note: "", loading: false, branch: "", conversation: pure.assistConversation() }; },
        computed: {
            main() { return (this.ws.root && this.ws.root.worktreeOf) || this.ws.rootId; },
            mainLabel() { const found = this.ws.roots.find((item) => item.id === this.main); return found ? found.label : this.main; }
        },
        mounted() { this.load(); },
        methods: {
            load() {
                this.loading = true;
                this.conversation = pure.assistConversation();
                this.ws.get("worktrees", { rootId: this.main }).then((answer) => {
                    this.loading = false; this.items = answer.worktrees || []; this.note = answer.note || "";
                }, (error) => { this.loading = false; this.ws.fail(error); });
            },
            refreshRoots() { return this.ws.get("roots").then((answer) => { this.ws.roots = answer.roots || this.ws.roots; }); },
            create() {
                this.ws.post("createWorktree", { rootId: this.main, conversationId: this.conversation || "", branch: this.branch.trim() }).then((answer) => {
                    this.ws.note("Worktree on " + answer.branch + (this.conversation ? " for this Assist session" : ""));
                    this.branch = "";
                    this.refreshRoots().then(() => this.load());
                }, this.ws.fail);
            },
            open(item) { this.ws.selectRoot(item.rootId); },
            remove(item) {
                const go = (discard) => this.ws.post("removeWorktree", { rootId: item.rootId, discardChanges: discard ? "true" : "false" }).then((answer) => {
                    this.ws.note(answer.message || "Removed");
                    if (this.ws.rootId === item.rootId) this.ws.selectRoot(this.main);
                    this.refreshRoots().then(() => this.load());
                }, this.ws.fail);
                if (!item.changes) { go(false); return; }
                this.$q.dialog({ title: "Discard the uncommitted changes?", message: item.label + " has " + item.changes + " uncommitted changes, which removing it loses. Its branch " + item.branch + " stays with its commits.",
                    cancel: true, ok: { label: "Discard and remove", color: "negative", unelevated: true } }).onOk(() => go(true));
            }
        },
        template: `
<div class="column no-wrap q-pa-sm" id="DevSessions">
  <div class="text-caption">Agents' sessions of {{mainLabel}}: each in a worktree on a branch of its own, so two sessions do not share uncommitted changes.
    The session's file and git tools work in its worktree.</div>
  <div class="text-caption text-orange-9" id="DevSessionsNotLoaded">{{note}}</div>
  <div class="row items-center q-gutter-x-sm q-my-xs">
    <q-input dense outlined v-model="branch" label="Branch (agent/… by default)" style="min-width: 14rem"></q-input>
    <q-btn dense unelevated no-caps color="primary" size="sm" :label="conversation ? 'New worktree for this Assist session' : 'New worktree'" @click="create" id="DevNewWorktree"></q-btn>
    <span v-if="!conversation" class="text-caption m-muted">No Assist conversation in this tab yet: one bound to it starts from the drawer's session.</span>
    <q-space></q-space><q-btn dense flat no-caps size="sm" label="Refresh" :loading="loading" @click="load"></q-btn>
  </div>
  <div v-for="item in items" :key="item.rootId" class="row items-center no-wrap text-caption q-py-xs" :data-worktree="item.rootId">
    <span class="text-weight-medium ellipsis">{{item.label}}</span>
    <span class="m-muted q-ml-sm">{{item.branch}}</span>
    <q-badge v-if="item.conversationId" color="grey-6" class="q-ml-sm" :label="'session ' + item.conversationId"></q-badge>
    <span v-if="item.changes" class="text-orange-9 q-ml-sm">{{item.changes}} uncommitted</span>
    <q-space></q-space>
    <q-btn dense flat no-caps size="sm" label="Open" @click="open(item)"></q-btn>
    <q-btn dense flat no-caps size="sm" label="Remove" color="negative" @click="remove(item)"></q-btn>
  </div>
  <div v-if="!loading && !items.length" class="m-muted text-caption">No worktrees of {{mainLabel}}.</div>
</div>`
    };

    devAide.gitComponents = {
        "m-dev-git-branches": branches, "m-dev-git-tags": tags, "m-dev-git-stashes": stashes,
        "m-dev-git-settings": settings, "m-dev-git-conflict": conflict, "m-dev-git-merge-requests": mergeRequests,
        "m-dev-git-checks": checks, "m-dev-git-sessions": sessions
    };
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(devAide.gitComponents).forEach((name) => qvt.registerLate(name, devAide.gitComponents[name]));
    devAide.gitRegistered = true;
})(typeof window !== "undefined" ? window : globalThis);
