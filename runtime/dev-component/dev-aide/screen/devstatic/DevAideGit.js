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
 * Advanced toggle, the Settings tab (remotes, the root's components, its publish branch, Open on the hosting site), a
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

    const branches = {
        name: "mDevGitBranches",
        props: { ws: { type: Object, required: true } },
        data() { return { local: [], remote: [], compare: null, loading: false }; },
        mounted() { this.load(); },
        methods: {
            load() {
                this.loading = true;
                return this.ws.get("branches", { rootId: this.ws.rootId }).then((answer) => {
                    this.local = answer.local || []; this.remote = answer.remote || []; this.loading = false;
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
        <span class="ellipsis text-weight-medium">{{branch.name}}</span><q-space></q-space>
        <q-btn dense flat round size="xs" icon="more_vert"><q-menu><q-list dense style="min-width: 12rem">
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
            rootInfo() { return this.ws.root || {}; }
        },
        methods: {
            hostPage: (url) => pure.hostPage(url),
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

    devAide.gitComponents = {
        "m-dev-git-branches": branches, "m-dev-git-tags": tags, "m-dev-git-stashes": stashes,
        "m-dev-git-settings": settings, "m-dev-git-conflict": conflict
    };
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(devAide.gitComponents).forEach((name) => qvt.registerLate(name, devAide.gitComponents[name]));
    devAide.gitRegistered = true;
})(typeof window !== "undefined" ? window : globalThis);
