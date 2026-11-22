/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// A single-user file or one WikiPage collaboration room. Only attachment names cross the room.
(function (root) {
    "use strict";
    const moqui = root.moqui;
    const board = moqui.whiteboard = {};
    const base = "/libs/excalidraw/";
    const appKeys = ["viewBackgroundColor", "gridSize", "gridStep", "gridModeEnabled"];
    const imageTypes = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/svg+xml"];
    const order = (a, b) => a < b ? -1 : a > b ? 1 : 0;
    const sortElements = (a, b) => order(a.index || "", b.index || "") || order(a.id, b.id);
    const newer = (a,b) => a.version > b.version || (a.version === b.version && a.versionNonce < b.versionNonce);
    const revision = e => JSON.stringify([e.id, e.version, e.versionNonce]);
    const clean = value => JSON.parse(JSON.stringify(value));
    const settings = app => Object.fromEntries(appKeys.filter(k => app[k] !== undefined).map(k => [k, app[k]]));
    function winners(map) {
        const found = new Map();
        for (const e of map.values()) if (!found.has(e.id) || newer(e, found.get(e.id))) found.set(e.id, e);
        return found;
    }
    board.read = doc => [...winners(doc.getMap("elements")).values()].sort(sortElements);
    board.write = (doc, elements, appState, origin) => {
        const map = doc.getMap("elements"), app = doc.getMap("app"), latest = winners(map);
        const changed = elements.filter(e => !latest.has(e.id) || newer(e, latest.get(e.id)));
        doc.transact(() => {
            const replace = new Set(changed.map(e => e.id));
            if (replace.size) {
                for (const [key, e] of map.entries()) if (replace.has(e.id)) map.delete(key);
                for (const e of changed) map.set(revision(e), clean(e));
            }
            for (const [key, value] of Object.entries(settings(appState))) if (app.get(key) !== value) app.set(key, value);
        }, origin);
    };
    function localUrl(value) {
        const url = new URL(value, root.location.href);
        if (url.origin !== root.location.origin || !["http:", "https:"].includes(url.protocol)) throw Error("A local URL is required");
        return url.href;
    }
    function dataUrl(blob) {
        return new Promise((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result);
            reader.onerror = () => reject(reader.error);
            reader.readAsDataURL(blob);
        });
    }
    async function fetchJson(url, options) {
        const reply = await fetch(localUrl(url), Object.assign({ credentials: "same-origin", redirect: "error" }, options));
        if (!reply.ok) throw Error("Request failed (" + reply.status + ")");
        const data = await reply.json();
        if (data.error || (data.errors && data.errors.length)) throw Error(String(data.error || data.errors));
        return data;
    }
    board.converter = async () => {
        await moqui.mermaid.load();
        await moqui.loadScript(base + "converter.js", null, () => !!root.MoquiBoardMermaid);
        return root.MoquiBoardMermaid;
    };
    const defaultLibraries = ["wireframe", "flowchart"].map(name => base + "library/" + name + ".excalidrawlib");
    function serialize(elements, state, files) {
        const rows = elements.filter(e => !e.isDeleted).slice().sort(sortElements).map(e => JSON.stringify(e));
        return '{"type":"excalidraw","version":2,"elements":[\n' + rows.join(",\n") + '\n],"appState":' +
            JSON.stringify(settings(state)) + ',"files":' + JSON.stringify(files) + '}\n';
    }
    board.component = {
        name: "mWhiteboard",
        props: {
            wikiPageId: String, scene: [Object, String], readOnly: Boolean, filesUrl: String, uploadUrl: String,
            saveUrl: String, previewUrl: String, linkSchemes: Array, libraries: Array, skeleton: Array,
            stage: Boolean, follow: String, socketClient: Object, legId: String
        },
        emits: ["editors", "saved", "open-link", "unfollow", "error"],
        data() {
            return { ready: false, canEdit: false, error: "", failed: false, source: null, editingFrame: "",
                selectedMermaid: null, selectedLink: "", previewText: "", converting: false };
        },
        mounted() { this.open(); },
        beforeUnmount() {
            this.closed = true;
            cancelAnimationFrame(this.queuedFrame);
            cancelAnimationFrame(this.writeFrame);
            this.flushChanges();
            clearTimeout(this.previewTimer);
            clearTimeout(this.connectTimer);
            if (this.provider) this.provider.destroy();
            if (this.awareness) this.awareness.destroy();
            if (this.doc) this.doc.destroy();
            if (this.reactRoot) this.reactRoot.unmount();
        },
        watch: {
            readOnly() { this.draw(); },
            dark() { this.draw(); },
            follow() { this.presence(); }
        },
        computed: { dark() { return this.$q && this.$q.dark.isActive; } },
        methods: {
            t(text) { return moqui.l10n ? moqui.l10n.localize(text) : text; },
            report(err) { this.error = String(err.message || err); this.$emit("error", this.error); },
            initial() {
                return (typeof this.scene === "string" ? JSON.parse(this.scene) : this.scene) || { elements: [], appState: {}, files: {} };
            },
            async open() {
                try {
                    root.EXCALIDRAW_ASSET_PATH = new URL(base, root.location.href).href;
                    moqui.loadStylesheet(base + "editor.css");
                    const lib = Promise.all([...defaultLibraries, ...(this.libraries || [])].map(async path =>
                        (await fetchJson(path)).libraryItems));
                    const loads = [lib, moqui.loadScript(base + "editor.js", null, () => !!root.MoquiBoardEditor)];
                    if (this.wikiPageId) loads.push(moqui.loadScript("/libs/tiptap/tiptap.min.js", null, () => !!root.Tiptap));
                    const [groups] = await Promise.all(loads);
                    if (this.closed) return;
                    this.libItems = groups.flat();
                    this.metaFiles = Object.assign({}, this.initial().files);
                    this.fileJobs = new Map();
                    this.canEdit = !this.readOnly && !this.wikiPageId;
                    this.reactRoot = root.Vue.markRaw(root.MoquiBoardEditor.createRoot(this.$refs.canvas));
                    if (this.wikiPageId) this.connect();
                    else this.draw();
                } catch (err) { this.failed = true; this.report(err); }
            },
            connect() {
                const client = this.socketClient || (this.$root && this.$root.notificationClient);
                if (!client) throw Error(this.t("The collaboration connection is unavailable"));
                const T = root.Tiptap;
                this.doc = root.Vue.markRaw(new T.Y.Doc());
                this.awareness = root.Vue.markRaw(new T.Awareness(this.doc));
                const name = this.$root.userFullName || this.$root.username || this.$root.userId || this.t("Guest");
                this.awareness.setLocalStateField("user", { name, color: moqui.coedit.userColor(this.$root.userId || name), legId: this.legId });
                this.provider = root.Vue.markRaw(new moqui.coedit.Provider({ client, lib: T, doc: this.doc, awareness: this.awareness,
                    docKey: "WikiPage:" + this.wikiPageId,
                    onStatus: status => this.$emit("editors", status.peers.map(p => p.name)),
                    onAccess: allowed => { clearTimeout(this.connectTimer); this.canEdit = allowed; this.draw(); this.remote(); },
                    onSaved: answer => this.$emit("saved", answer),
                    onFallback: info => { this.failed = true; this.report(info.message || this.t("The board is unavailable")); }
                }));
                this.doc.on("update", (update, origin) => {
                    if (origin !== this && !this.closed && !this.queuedFrame) this.queuedFrame = requestAnimationFrame(() => {
                        this.queuedFrame = null;
                        this.remote();
                    });
                });
                this.awareness.on("change", () => this.presence());
                this.connectTimer = setTimeout(() => {
                    if (!this.closed && !this.provider.joinedOnce) this.provider.fail("timeout", null, this.t("The collaboration connection is unavailable"));
                }, 20000);
                this.provider.start();
            },
            draw() {
                if (!this.reactRoot || this.closed || this.failed) return;
                const E = root.MoquiBoardEditor;
                if (!this.startData) {
                    const scene = this.initial();
                    const elems = this.doc ? board.read(this.doc) : scene.elements || [];
                    this.startData = { elements: elems.length || !this.skeleton || this.doc ? elems : E.convertToExcalidrawElements(this.skeleton),
                        appState: settings(this.doc ? this.doc.getMap("app").toJSON() : scene.appState || {}), libraryItems: this.libItems };
                }
                const options = {
                    initialData: this.startData, viewModeEnabled: this.readOnly || !this.canEdit, theme: this.dark ? "dark" : "light",
                    langCode: (document.documentElement.lang || "en").replace("_", "-"), aiEnabled: false, validateEmbeddable: false,
                    handleKeyboardGlobally: false, isCollaborating: !!this.doc, zenModeEnabled: this.stage || undefined,
                    UIOptions: { canvasActions: { loadScene: false, saveToActiveFile: false, export: false, saveAsImage: false } },
                    excalidrawAPI: api => {
                        if (this.api === api) return;
                        this.api = root.Vue.markRaw(api);
                        this.ready = true;
                        if (this.doc) this.remote(); else this.loadFiles(this.metaFiles);
                    },
                    onChange: (els, state, files) => this.changed(els, state, files),
                    onPointerUpdate: ({pointer, button}) => {
                        if (this.awareness) this.awareness.setLocalStateField("cursor", { pointer, button });
                    },
                    onScrollChange: (scrollX, scrollY, zoom) => {
                        if (this.awareness) this.awareness.setLocalStateField("viewport", {scrollX, scrollY, zoom: zoom.value});
                    },
                    onLinkOpen: (el, event) => { event.preventDefault(); this.openLink(el.link); }
                };
                this.reactRoot.render(E.h(E.Excalidraw, options, E.h(E.MainMenu, null,
                    E.h(E.MainMenu.DefaultItems.ClearCanvas))));
            },
            remote() {
                if (!this.api || this.closed) return;
                this.flushChanges();
                const elems = board.read(this.doc);
                this.api.updateScene({elements: elems, appState: this.doc.getMap("app").toJSON(), captureUpdate: "NEVER"});
                this.metaFiles = this.doc.getMap("files").toJSON();
                this.loadFiles(this.metaFiles);
            },
            presence() {
                if (!this.api || !this.awareness || this.closed) return;
                const collaborators = new Map();
                let viewport;
                for (const [id, state] of this.awareness.getStates()) {
                    if (id === this.awareness.clientID || !state.user) continue;
                    const c = state.user.color || "#4267aa";
                    collaborators.set(String(id), Object.assign({ username: state.user.name, color: { stroke: c, background: c },
                        selectedElementIds: state.selection || {} }, state.cursor));
                    if (this.follow && (this.follow === state.user.legId || this.follow === String(id))) viewport = state.viewport;
                }
                const data = { collaborators, captureUpdate: "NEVER" };
                if (viewport && Number.isFinite(viewport.zoom) && viewport.zoom > 0 && viewport.zoom <= 30 &&
                    Number.isFinite(viewport.scrollX) && Number.isFinite(viewport.scrollY)) {
                    data.appState = { scrollX: viewport.scrollX, scrollY: viewport.scrollY, zoom: {value: viewport.zoom} };
                }
                this.api.updateScene(data);
            },
            changed(els, state, files) {
                if (this.closed || this.failed) return;
                const picked = els.filter(e => !e.isDeleted && state.selectedElementIds[e.id]);
                const frame = picked.find(e => e.type === "frame") || els.find(e => picked.some(p => p.frameId === e.id));
                this.selectedMermaid = frame && frame.customData && frame.customData.mermaid ? frame.id : null;
                this.selectedLink = picked.length === 1 && this.customLink(picked[0].link) ? picked[0].link : "";
                if (this.awareness && this.previousSelection !== state.selectedElementIds) {
                    this.previousSelection = state.selectedElementIds;
                    this.awareness.setLocalStateField("selection", state.selectedElementIds);
                }
                if (this.readOnly || !this.canEdit) return;
                if (this.doc) {
                    this.pendingChanges = {els, state};
                    if (!this.writeFrame) this.writeFrame = requestAnimationFrame(() => this.flushChanges());
                }
                for (const file of Object.values(files || {})) {
                    if (!this.metaFiles[file.id] && !this.fileJobs.has(file.id)) this.queueFile(file.id, this.upload(file));
                }
            },
            flushChanges() {
                this.writeFrame = null;
                if (!this.pendingChanges || !this.doc) return;
                const {els, state} = this.pendingChanges;
                this.pendingChanges = null;
                board.write(this.doc, els, state, this);
            },
            customLink(link) {
                const scheme = /^(?!javascript:|data:|file:|vbscript:|blob:)([a-z][a-z0-9+.-]*):/i.exec(link || "");
                return !!(scheme && (this.linkSchemes || []).some(s => String(s).replace(/:$/, "").toLowerCase() === scheme[1].toLowerCase()));
            },
            openLink(link) {
                if (this.customLink(link)) this.$emit("open-link", link);
                else if (/^(https?:\/\/|mailto:|\/(?![\/\\]))/i.test(link || "")) moqui.openLink(link);
            },
            queueFile(id, promise) {
                const tracked = promise.catch(err => { if (!this.closed) this.report(err); throw err; });
                this.fileJobs.set(id, tracked);
                // A failure remains visible and blocks Save until the editor is reopened.
                tracked.catch(() => {});
            },
            async upload(file) {
                if (!this.uploadUrl || !imageTypes.includes(file.mimeType) || !String(file.dataURL).startsWith("data:" + file.mimeType + ";base64,")) throw Error(this.t("This image cannot be attached"));
                const blob = await (await fetch(file.dataURL)).blob();
                const form = new FormData();
                const ext = {"image/svg+xml":"svg", "image/jpeg":"jpg"}[file.mimeType] || file.mimeType.slice(6);
                form.append("file", blob, file.id + "." + ext);
                if (this.wikiPageId) form.append("wikiPageId", this.wikiPageId);
                const reply = await fetchJson(this.uploadUrl, { method: "POST", body: form, headers: this.token() });
                if (!reply.attachmentName) throw Error(this.t("The image attachment was not saved"));
                if (this.closed) return;
                const ref = {attachmentName: reply.attachmentName, mimeType: file.mimeType};
                this.metaFiles[file.id] = ref;
                if (this.doc) this.doc.getMap("files").set(file.id, ref);
            },
            loadFiles(refs) {
                for (const [id, ref] of Object.entries(refs || {})) {
                    if (this.fileJobs.has(id) || this.api.getFiles()[id]) continue;
                    this.queueFile(id, this.fetchImage(id, ref));
                }
            },
            async fetchImage(id, ref) {
                const name = typeof ref === "string" ? ref : ref.attachmentName;
                if (!this.filesUrl || !name || /[\\/]/.test(name) || name === "." || name === "..") throw Error(this.t("Invalid image attachment"));
                const target = this.filesUrl.includes("{name}") ? this.filesUrl.replace("{name}", encodeURIComponent(name)) :
                    this.filesUrl + encodeURIComponent(name);
                const response = await fetch(localUrl(target), {redirect: "error"});
                if (!response.ok) throw Error(this.t("Image attachment unavailable"));
                let blob = await response.blob();
                const mimeType = imageTypes.includes(blob.type) ? blob.type : ref.mimeType;
                if (!imageTypes.includes(mimeType)) throw Error(this.t("Unsupported image attachment"));
                if (blob.type !== mimeType) blob = blob.slice(0, blob.size, mimeType);
                const url = await dataUrl(blob);
                if (!this.closed) this.api.addFiles([{id, mimeType, dataURL: url, created: 1}]);
            },
            token() { return {"X-CSRF-Token": this.$root.moquiSessionToken || moqui.sessionToken || ""}; },
            exportArgs() { return {elements: this.api.getSceneElements(), appState: this.api.getAppState(), files: this.api.getFiles()}; },
            sceneText() {
                const s = this.exportArgs();
                return serialize(s.elements, s.appState, this.metaFiles);
            },
            async save(note) {
                if (!this.canEdit || this.readOnly || !this.ready || !this.saveUrl) throw Error(this.t("The board cannot be saved"));
                await Promise.all(this.fileJobs.values());
                this.flushChanges();
                if (this.provider) {
                    this.provider.flush();
                    const until = Date.now() + 10000;
                    while (this.provider.status().state !== "synced") {
                        if (this.closed || Date.now() >= until) throw Error(this.t("Waiting for board changes to sync"));
                        await new Promise(r => setTimeout(r, 30));
                    }
                }
                const scene = this.sceneText();
                const preview = new XMLSerializer().serializeToString(await root.MoquiBoardEditor.exportToSvg(this.exportArgs()));
                const data = await fetchJson(this.saveUrl, { method: "POST", headers: Object.assign({"Content-Type":"application/json"}, this.token()),
                    body: JSON.stringify({wikiPageId: this.wikiPageId, note: note || "", scene, preview}) });
                this.$emit("saved", data);
                return data;
            },
            async download(format) {
                let blob;
                if (format === "svg" || format === "png") {
                    if (!this.api) throw Error(this.t("The board is unavailable"));
                    await Promise.all(this.fileJobs.values());
                    blob = format === "png" ? await root.MoquiBoardEditor.exportToBlob({...this.exportArgs(), mimeType:"image/png"}) :
                        new Blob([new XMLSerializer().serializeToString(await root.MoquiBoardEditor.exportToSvg(this.exportArgs()))], {type:"image/svg+xml"});
                } else {
                    format = "excalidraw";
                    blob = new Blob([this.api ? this.sceneText() : JSON.stringify(this.initial())], {type:"application/json"});
                }
                const url = URL.createObjectURL(blob);
                const link = document.createElement("a");
                link.href = url; link.download = "Board." + format; link.click();
                setTimeout(() => URL.revokeObjectURL(url), 1000);
            },
            regenerate() {
                if (!this.api || this.readOnly || !this.canEdit || !this.skeleton) return;
                this.replaceElements(this.api.getSceneElements().map(e => e.id), root.MoquiBoardEditor.convertToExcalidrawElements(this.skeleton));
            },
            replaceElements(ids, additions) {
                const drop = new Set(ids);
                const old = this.api.getSceneElementsIncludingDeleted().map(e => drop.has(e.id) ?
                    {...e, isDeleted:true, version:e.version + 1, versionNonce: Math.floor(Math.random() * 2147483647), updated:Date.now()} : e);
                this.api.updateScene({elements: old.concat(additions), captureUpdate:"IMMEDIATELY"});
            },
            editMermaid(id) {
                this.editingFrame = id || "";
                const el = this.api.getSceneElements().find(e => e.id === id);
                this.updateSource(el ? el.customData.mermaid : "flowchart LR\n    A --> B");
            },
            updateSource(value) {
                this.source = value;
                clearTimeout(this.previewTimer);
                this.previewTimer = setTimeout(() => { this.previewText = value; }, 300);
            },
            async insertMermaid() {
                if (!this.source || this.converting || this.readOnly || !this.canEdit) return;
                this.converting = true;
                try {
                    const source = this.source;
                    const previous = this.api.getSceneElements().find(e => e.id === this.editingFrame);
                    const converted = await (await board.converter()).parseMermaidToExcalidraw(source, {securityLevel:"strict", startOnLoad:false});
                    const E = root.MoquiBoardEditor;
                    const children = E.convertToExcalidrawElements(converted.elements);
                    if (!children.length) throw Error(this.t("The diagram is empty"));
                    const [x, y, r, b] = E.getCommonBounds(children);
                    const frameId = crypto.randomUUID();
                    const state = this.api.getAppState();
                    const fx = previous ? previous.x : -state.scrollX + 40, fy = previous ? previous.y : -state.scrollY + 40;
                    const frame = E.convertToExcalidrawElements([{type:"frame", id:frameId, name:"Mermaid",
                        x:fx, y:fy, width:r-x+48, height:b-y+48, children:[]}], {regenerateIds:false})[0];
                    Object.assign(frame, {x:fx,y:fy,width:r-x+48,height:b-y+48,customData:{mermaid:source}});
                    if (converted.files) this.api.addFiles(Object.values(converted.files));
                    const remove = previous ? this.api.getSceneElements().filter(e => e.id === previous.id || e.frameId === previous.id).map(e => e.id) : [];
                    this.replaceElements(remove, [...children.map(e => ({...e,x:fx+24+e.x-x,y:fy+24+e.y-y,frameId:frame.id})),frame]);
                    this.source = null;
                } catch (err) { this.report(err); }
                finally { this.converting = false; }
            }
        },
        render() {
            const {h, resolveComponent: c} = root.Vue;
            const btn = (label, action, opts) => h(c("q-btn"), {flat:true,dense:true,label:this.t(label),onClick:action,...opts});
            const toolbar = [];
            if (this.canEdit && !this.readOnly && this.ready && !this.stage) {
                toolbar.push(btn("Insert Mermaid", () => this.editMermaid()));
                if (this.selectedMermaid) toolbar.push(btn("Edit as Mermaid", () => this.editMermaid(this.selectedMermaid)));
                if (this.skeleton) toolbar.push(btn("Regenerate", async () => {
                    if (await moqui.confirmAction(this.t("Replace the board with the supplied shapes?"))) this.regenerate();
                }));
                if (!this.wikiPageId && this.saveUrl) toolbar.push(btn("Save", () => this.save().catch(this.report)));
            }
            if (this.selectedLink) toolbar.push(btn(this.selectedLink, () => this.openLink(this.selectedLink)));
            const dialog = this.source === null ? null : h(c("q-dialog"), {modelValue:true, "onUpdate:modelValue":v => {if (!v) this.source=null;}}, () =>
                h(c("q-card"), {style:"width:min(52rem,95vw)"}, () => [
                    h(c("q-card-section"), {}, () => [h("h2", {class:"text-h6 q-my-none"}, this.t("Mermaid diagram")),
                        h(c("m-text-area"), {modelValue:this.source,"onUpdate:modelValue":this.updateSource,editorType:"code",language:"mermaid",label:this.t("Diagram source")}),
                        this.previewText && h(c("m-mermaid"), {key:this.previewText,text:this.previewText,height:"14rem"}),
                        this.editingFrame && h("p",this.t("Replacing the diagram discards hand edits to its shapes."))]),
                    h(c("q-card-actions"), {align:"right"}, () => [btn("Cancel",()=>{this.source=null;}),
                        btn(this.editingFrame?"Replace diagram":"Insert diagram",this.insertMermaid,{loading:this.converting,color:"primary"})])
                ]));
            return h("div", {class:"m-whiteboard"}, [
                toolbar.length ? h("div",{class:"row items-center q-gutter-xs"},toolbar) : null,
                !this.ready && !this.failed ? h("p",{role:"status"},this.t("Opening board…")) : null,
                this.error ? h("p",{role:"alert",class:"text-negative"},this.error) : null,
                this.failed && this.previewUrl ? h("img",{src:this.previewUrl,alt:this.t("Board preview"),style:"max-width:100%"}) : null,
                this.failed ? btn("Download board",()=>this.download("excalidraw").catch(this.report)) : null,
                h("div",{ref:"canvas",style:{height:this.stage?"100%":"min(72vh,56rem)",minHeight:"16rem",display:this.failed?"none":"block"},
                    onWheel:()=>{if(this.follow)this.$emit("unfollow");},onPointerdown:()=>{if(this.follow)this.$emit("unfollow");}}), dialog
            ]);
        }
    };
})(globalThis);
