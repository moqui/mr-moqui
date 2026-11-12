/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Floats in the /qapps shell (plans/Terminal-Fancy.md §10, D12, D13, D17): windows over the shell that are not dialogs. They
 * are not modal, several are open at once, they move and resize by pointer events (mouse, touch and pen) and by keys, snap to
 * halves and quarters, tile, cascade and minimize. The word is float: "panel" already names the tray panel, the record panels
 * and the layout panels.
 *
 *   - moqui.floats, the store: the kinds of content that float (register), the windows (state.floats: id, kind, key, title,
 *     x, y, w, h, z, minimized, maximized, hidden), what opens, docks, closes, raises and arranges them, and the places kept
 *     in localStorage per device (D13), under moqui.scopedKey("moqui.floats");
 *   - m-float-layer, in the shell's root outside #m-canvas: a fixed layer over the page, the header and the drawers, below
 *     dialogs, menus, notices and the call's ring bar. It draws what the store lists and holds no content of its own; the
 *     minimized windows are chips in a strip at its bottom, beside the Arrange menu;
 *   - m-float, one window: a title bar (the kind's icon or status dot, the title, a badge; Minimize, Maximize, Dock, Close),
 *     eight resize grips, and an empty body element with a stable id.
 *
 * Content comes to the window; the window never makes it (D17). The owner of a piece of content keeps its component where it is
 * in the component tree and moves its element into the body: a Vue teleport,
 *     <teleport :to="floats.target(kind, key) || 'body'" :disabled="!floats.target(kind, key)">
 * or, for content that is an element of its own (a terminal), floats.whenReady(kind, key) and appendChild. target answers the
 * body's selector only once the body is mounted, so a teleport never names an element that is not there yet (Vue 3.5 resolves
 * a teleport's target when its to changes, which is why to changes with it). Nothing is mounted again, so a draft, a run or a
 * screen survives the move; a moved element loses its scroll position, and each kind puts its own back.
 *
 * Below 600 px, and on a density="handheld" screen, nothing floats (§10.8): open answers false, canFloat() says so for an
 * owner's Undock button, and the windows open when the page narrows are docked, closed.
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml): its components are in moqui.qvtComponents, which
 * WebrootVue.qvt.js registers and precompiles into WebrootVue.qvt.render.js. Its tests are test/client/WebrootFloats.spec.js.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);

    const STORE_KEY = "moqui.floats";
    // nothing floats in a viewport narrower than this (§10.8)
    const MIN_VIEWPORT = 600;
    // a drag whose pointer is this close to an edge snaps to that half, and this close to a corner along an edge to its quarter
    const EDGE = 16;
    const CORNER = 96;
    // what an arrow key moves or resizes a window by, the gap arranging leaves, and the offset of a cascade
    const STEP = 16;
    const GAP = 8;
    const CASCADE = 32;
    // a pointer that moves less than this has clicked, not dragged
    const SLOP = 3;
    const MIN_SIZE = { w: 240, h: 160 };
    const EDGES = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];
    const FOCUSABLE = "[autofocus], textarea, input:not([type=hidden]), select, [contenteditable=true], button, a[href], [tabindex]:not([tabindex='-1'])";

    const placeKey = (kind, key) => kind + "|" + (key || "");
    const idOf = (kind, key) => kind + (key ? ":" + key : "");
    // an id as an element id and a selector: letters, digits, - and _
    const domIdOf = (id) => String(id).replace(/[^A-Za-z0-9_-]/g, "-");
    const valueOf = (value, key, float) => (typeof value === "function" ? value(key, float) : value);
    const storage = () => { try { return root.localStorage || null; } catch (err) { return null; } };
    const storeKey = () => (moqui && moqui.scopedKey ? moqui.scopedKey(STORE_KEY) : STORE_KEY);
    const rectOf = (obj) => ({ x: Math.round(obj.x), y: Math.round(obj.y), w: Math.round(obj.w), h: Math.round(obj.h) });
    const num = (value) => typeof value === "number" && isFinite(value);

    const floats = {
        labels: { minimize: "Minimize", maximize: "Maximize", restore: "Restore", dock: "Dock", close: "Close", arrange: "Arrange windows",
            tile: "Tile", cascade: "Cascade", column: "Column at the right", minimizeAll: "Minimize all", windows: "Floating windows",
            moveKeys: "arrow keys move, Shift and arrow keys resize", show: "Show", gather: "Gather" },
        EDGES,
        MIN_VIEWPORT,
        // the kinds by name: their definitions, functions and all, not reactive
        kinds: {},
        // the windows in the order they opened (never reordered: a moved element loses its scroll, so z orders them); the
        // bodies mounted (ready, by id); the snap outline of a drag; the area the windows keep to; a handheld screen
        state: reactive({ floats: [], ready: {}, snap: null, area: { x: 0, y: 0, w: 0, h: 0 }, handheld: false }),
        // what the storage had at load: the places by kind and key (also of windows docked since), and the windows that were
        // floating, which their kinds open again when they register (wasFloated)
        saved: { places: {} },
        restoring: {},
        loaded: false,
        waiters: [],

        // ---- kinds ----

        // A kind of content that floats (§10.1). def:
        //   title, icon, badge, dot: a value or a function (key, float) of the window's title, icon, count badge and status dot
        //     (a dot is a color name: positive, warning, negative, grey), read at each render so a reactive count shows;
        //   defaultSize {w, h}, minSize {w, h}; place "right" (at the right below the header) or "center" (the default);
        //   dock(shown, key, float): put the content back where it lives, shown there or closed; the window is gone by then;
        //   inReach: true when Assist may read the floated content (§10.7, D18), false or absent when it may not;
        //   focus(key, bodyEl): give the content the keys (the first focusable element of the body otherwise);
        //   gatherLabel: an Arrange item that docks every window of the kind (§10.4's Gather);
        //   home(key, float): where the content docks, measured when a drag starts: {rect, edge}, or null for nowhere now. A
        //     window dragged to that edge of the area (left, right, top or bottom; outside the corners), or with no edge onto
        //     the rect, shows the rect as a Dock outline and docks at the drop, in place of that edge's snap (§15).
        // Registering again replaces the definition. A kind asks wasFloated(kind, key) and opens itself when it was.
        register(kind, def) {
            floats.load();
            floats.kinds[kind] = Object.assign({ minSize: MIN_SIZE, defaultSize: { w: 400, h: 480 }, place: "center" }, def || {});
            return floats.kinds[kind];
        },
        // whether this (kind, key) was floating when the page was last left, and has not been opened or docked since
        wasFloated(kind, key) {
            floats.load();
            return !!floats.restoring[placeKey(kind, key)] && floats.canFloat();
        },

        // ---- the windows ----

        find(kind, key) { return floats.state.floats.find((f) => f.kind === kind && f.key === (key || "")) || null; },
        get(id) { return floats.state.floats.find((f) => f.id === id) || null; },
        // the element ids of a window and of its body
        domId(id) { return "m-float-" + domIdOf(id); },
        bodyId(id) { return "m-float-body-" + domIdOf(id); },
        // The teleport target of a (kind, key): the body's selector once its window is open and its body mounted, else null.
        target(kind, key) {
            const f = floats.find(kind, key);
            return f && floats.state.ready[f.id] ? "#" + floats.bodyId(f.id) : null;
        },
        bodyOf(kind, key) {
            const f = floats.find(kind, key);
            return f && floats.state.ready[f.id] && root.document ? root.document.getElementById(floats.bodyId(f.id)) : null;
        },
        // A promise of the body element of a (kind, key), for content that moves its own element; null when the window closes
        // before its body is there.
        whenReady(kind, key) {
            const now = floats.bodyOf(kind, key);
            if (now) return Promise.resolve(now);
            if (!floats.find(kind, key)) return Promise.resolve(null);
            return new Promise((resolve) => floats.waiters.push({ kind, key: key || "", resolve }));
        },
        // m-float's mount and unmount of its body
        bodyReady(id, on) {
            const s = floats.state;
            if (on) s.ready[id] = true; else delete s.ready[id];
            const f = floats.get(id);
            floats.waiters = floats.waiters.filter((waiter) => {
                const mine = f && waiter.kind === f.kind && waiter.key === f.key;
                if (!mine && floats.find(waiter.kind, waiter.key)) return true;
                waiter.resolve(mine && on ? floats.bodyOf(f.kind, f.key) : null);
                return false;
            });
        },
        // the shown title, icon, badge and dot of a window, from its kind
        titleOf(f) { const def = floats.kinds[f.kind] || {}; return String(valueOf(def.title, f.key, f) || f.title || f.kind); },
        iconOf(f) { const def = floats.kinds[f.kind] || {}; return valueOf(def.icon, f.key, f) || "sym_o_select_window"; },
        badgeOf(f) { const def = floats.kinds[f.kind] || {}; const value = valueOf(def.badge, f.key, f); return value ? String(value) : ""; },
        dotOf(f) { const def = floats.kinds[f.kind] || {}; return valueOf(def.dot, f.key, f) || ""; },
        // a kind's own buttons on the title bar, before Minimize: [{icon, label, run(key, float)}] (a terminal's End session)
        actionsOf(f) { const def = floats.kinds[f.kind] || {}; const list = valueOf(def.actions, f.key, f); return Array.isArray(list) ? list : []; },

        // Whether anything may float now: a viewport 600 px or wider and not a handheld screen (§10.8).
        canFloat() {
            const s = floats.state;
            if (!s.area.w) floats.measure();
            return s.area.w >= MIN_VIEWPORT && !s.handheld && !(moqui && moqui.currentScreenDensity === "handheld");
        },

        // Open a window of (kind, key), or raise the one open (its id). options: title; rect {x, y, w, h}; restore, to take the
        // kept order, minimized, maximized and hidden of before the load. Answers the id, or false when the kind is unknown or
        // nothing may float.
        open(kind, key, options) {
            floats.load();
            // measured again: at the layer's mount the header may not be laid out yet, and a window must not cover it
            floats.measure();
            const def = floats.kinds[kind];
            const k = key || "";
            if (!def) return false;
            const existing = floats.find(kind, k);
            if (existing) { floats.focus(existing.id); return existing.id; }
            if (!floats.canFloat()) return false;
            const opts = options || {};
            const pk = placeKey(kind, k);
            const was = opts.restore ? floats.restoring[pk] : null;
            delete floats.restoring[pk];
            const s = floats.state;
            const place = opts.rect || floats.saved.places[pk] || floats.defaultRect(def);
            const rect = floats.clampRect(place, s.area, def.minSize);
            const f = { id: idOf(kind, k), kind, key: k, title: String(opts.title || ""), x: rect.x, y: rect.y, w: rect.w, h: rect.h,
                z: floats.topZ() + 1, minimized: !!(was && was.minimized), maximized: !!(was && was.maximized), hidden: !!(was && was.hidden), restoreRect: null };
            if (was && num(was.z)) f.z = was.z;
            s.floats.push(f);
            floats.save();
            return f.id;
        },
        // Close: the content docks closed (§10.2: closing a float ends nothing). Dock: it docks and shows where it lives.
        close(id) { return floats.end(id, false); },
        dock(id) { return floats.end(id, true); },
        // The window goes first and then the kind docks its content, so a dock that opens the content where it lives (the tray
        // on a tab) finds it no longer floating.
        end(id, shown) {
            const f = floats.get(id);
            if (!f) return false;
            const s = floats.state;
            floats.saved.places[placeKey(f.kind, f.key)] = rectOf(f.restoreRect || f);
            delete floats.restoring[placeKey(f.kind, f.key)];
            s.floats.splice(s.floats.indexOf(f), 1);
            delete s.ready[id];
            if (s.snap && s.snap.id === id) s.snap = null;
            floats.bodyReady(id, false);
            floats.save();
            const def = floats.kinds[f.kind];
            if (def && def.dock) def.dock(shown, f.key, f);
            return true;
        },
        // every window docked, shown or closed; of one kind when it is given
        dockAll(shown, kind) {
            floats.state.floats.filter((f) => !kind || f.kind === kind).map((f) => f.id).forEach((id) => floats.end(id, shown));
        },
        // Hidden: still a float (Assist's Mod+J, a header button), drawn nowhere and its body still mounted. Shown again by show
        // or focus.
        hide(id) { const f = floats.get(id); if (!f) return false; f.hidden = true; floats.save(); return true; },
        show(id) {
            const f = floats.get(id);
            if (!f) return false;
            f.hidden = false;
            f.minimized = false;
            floats.raise(id);
            return true;
        },
        minimize(id) { const f = floats.get(id); if (!f) return false; f.minimized = true; floats.save(); return true; },
        // whether a window is shown: not hidden and not minimized
        shown(f) { return !!f && !f.hidden && !f.minimized; },
        topZ() { return floats.state.floats.reduce((top, f) => Math.max(top, f.z || 0), 0); },
        isTop(f) {
            const shown = floats.state.floats.filter((one) => floats.shown(one));
            return !!f && floats.shown(f) && shown.every((one) => one === f || one.z < f.z);
        },
        // to the top of the order; the order is kept small
        raise(id) {
            const f = floats.get(id);
            if (!f) return false;
            if (!floats.isTop(f) || !floats.shown(f)) f.z = floats.topZ() + 1;
            if (f.z > 1000) floats.state.floats.slice().sort((a, b) => a.z - b.z).forEach((one, index) => { one.z = index + 1; });
            floats.save();
            return true;
        },
        // Shown, raised and the content given the keys (§10.2: a click raises the window and gives its content the keys).
        focus(id) {
            if (!floats.show(id)) return false;
            floats.focusContent(id);
            return true;
        },
        focusContent(id) {
            const f = floats.get(id);
            if (!f || !root.document) return;
            const def = floats.kinds[f.kind] || {};
            root.setTimeout(() => {
                const body = root.document.getElementById(floats.bodyId(id));
                if (!body) return;
                if (def.focus && def.focus(f.key, body) !== false) return;
                if (body.contains(root.document.activeElement)) return;
                // without scrolling: the content stays where its reader left it
                const first = body.querySelector(FOCUSABLE);
                if (first && first.focus) first.focus({ preventScroll: true });
                else { body.setAttribute("tabindex", "-1"); body.focus({ preventScroll: true }); }
            }, 0);
        },
        // The header button of a floated piece of content (§10.5, §10.6): a hidden or minimized window shows, one under another
        // is raised, the top one hides. With none open, one opens. Answers the id, or false.
        toggle(kind, key) {
            const f = floats.find(kind, key);
            if (!f) return floats.open(kind, key);
            if (!floats.shown(f) || !floats.isTop(f)) floats.focus(f.id);
            else floats.hide(f.id);
            return f.id;
        },
        maximize(id, on) {
            const f = floats.get(id);
            if (!f) return false;
            f.maximized = on === undefined ? !f.maximized : !!on;
            if (f.maximized && f.restoreRect) { Object.assign(f, f.restoreRect); f.restoreRect = null; }
            floats.raise(id);
            return true;
        },
        // Ids of the bodies Assist may reach (§10.7): the shown windows of a kind that says inReach, with their body there.
        reach() {
            const s = floats.state;
            return s.floats.filter((f) => floats.shown(f) && s.ready[f.id] && (floats.kinds[f.kind] || {}).inReach).map((f) => floats.bodyId(f.id));
        },

        // ---- the area and the geometry (pure: the specs check them) ----

        // the viewport and the bottom of the shell's header (#top), which the windows keep below; a spec replaces it
        viewport() {
            const doc = root.document;
            const header = doc && doc.getElementById ? doc.getElementById("top") : null;
            const top = header && header.getBoundingClientRect ? Math.max(0, Math.round(header.getBoundingClientRect().bottom)) : 0;
            return { w: root.innerWidth || 0, h: root.innerHeight || 0, top };
        },
        measure() {
            const view = floats.viewport();
            const top = Math.min(view.top || 0, Math.max(0, view.h - 200));
            floats.state.area = { x: 0, y: top, w: view.w, h: Math.max(0, view.h - top) };
            return floats.state.area;
        },
        // A browser resize: the windows stay inside the viewport; a viewport that can no longer float docks them, closed.
        onViewport() {
            const area = floats.measure();
            if (!floats.canFloat()) { floats.dockAll(false); return; }
            floats.state.floats.forEach((f) => {
                const def = floats.kinds[f.kind] || {};
                Object.assign(f, floats.clampRect(f, area, def.minSize));
                if (f.restoreRect) f.restoreRect = floats.clampRect(f.restoreRect, area, def.minSize);
            });
            floats.save();
        },
        // the handheld screen of the root (m-float-layer watches it): nothing floats while it is shown
        setHandheld(on) {
            floats.state.handheld = !!on;
            if (on) floats.dockAll(false);
        },
        defaultRect(def) {
            const area = floats.state.area;
            const size = def.defaultSize || { w: 400, h: 480 };
            const w = Math.min(size.w, area.w - 2 * GAP);
            const h = Math.min(size.h, area.h - 2 * GAP);
            let x = def.place === "right" ? area.x + area.w - w - GAP : area.x + Math.round((area.w - w) / 2);
            let y = def.place === "right" ? area.y + GAP : area.y + Math.round((area.h - h) / 3);
            // a window opened where another already is goes down and in a step, so both can be seen
            while (floats.state.floats.some((f) => Math.abs(f.x - x) < 4 && Math.abs(f.y - y) < 4)) {
                x += def.place === "right" ? -CASCADE : CASCADE;
                y += CASCADE;
            }
            return { x, y, w, h };
        },
        // a rect inside the area: no smaller than the minimum (unless the area is), no larger than the area
        clampRect(rect, area, min) {
            const least = min || MIN_SIZE;
            const w = Math.round(Math.min(Math.max(num(rect.w) ? rect.w : least.w, Math.min(least.w, area.w)), area.w));
            const h = Math.round(Math.min(Math.max(num(rect.h) ? rect.h : least.h, Math.min(least.h, area.h)), area.h));
            const x = Math.round(Math.min(Math.max(num(rect.x) ? rect.x : area.x, area.x), area.x + area.w - w));
            const y = Math.round(Math.min(Math.max(num(rect.y) ? rect.y : area.y, area.y), area.y + area.h - h));
            return { x, y, w, h };
        },
        // §10.3: a pointer at an edge snaps to that half, near a corner along an edge to that quarter; null elsewhere
        snapZone(px, py, area) {
            const left = px <= area.x + EDGE, right = px >= area.x + area.w - EDGE;
            const top = py <= area.y + EDGE, bottom = py >= area.y + area.h - EDGE;
            const nearLeft = px <= area.x + CORNER, nearRight = px >= area.x + area.w - CORNER;
            const nearTop = py <= area.y + CORNER, nearBottom = py >= area.y + area.h - CORNER;
            if ((left && nearTop) || (top && nearLeft)) return "top-left";
            if ((right && nearTop) || (top && nearRight)) return "top-right";
            if ((left && nearBottom) || (bottom && nearLeft)) return "bottom-left";
            if ((right && nearBottom) || (bottom && nearRight)) return "bottom-right";
            if (left) return "left";
            if (right) return "right";
            if (top) return "top";
            if (bottom) return "bottom";
            return null;
        },
        snapRect(zone, area) {
            const halfW = Math.round(area.w / 2), halfH = Math.round(area.h / 2);
            const x = zone.indexOf("right") >= 0 ? area.x + area.w - halfW : area.x;
            const y = zone.indexOf("bottom") >= 0 ? area.y + area.h - halfH : area.y;
            const w = zone === "top" || zone === "bottom" ? area.w : halfW;
            const h = zone === "left" || zone === "right" ? area.h : halfH;
            return { x, y, w, h };
        },
        // The kind's home for a window (def.home), or null: a rect with a size, and an edge of the area or none.
        homeOf(f) {
            const def = floats.kinds[f.kind];
            if (!def || typeof def.home !== "function") return null;
            let home = null;
            try { home = def.home(f.key, f); } catch (err) { home = null; }
            const rect = home && home.rect;
            if (!rect || !num(rect.x) || !num(rect.y) || !(rect.w > 0) || !(rect.h > 0)) return null;
            return { rect: rectOf(rect), edge: ["left", "right", "top", "bottom"].indexOf(home.edge) >= 0 ? home.edge : null };
        },
        // whether a drag at (x, y), in snap zone `zone`, is at the window's home
        atHome(home, zone, x, y) {
            if (!home) return false;
            if (home.edge) return zone === home.edge;
            const r = home.rect;
            return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
        },
        // a window in a snap zone; its place before is kept, for a drag out of it and for Dock
        snapTo(id, zone, before) {
            const f = floats.get(id);
            if (!f) return false;
            f.restoreRect = rectOf(before || f);
            f.maximized = false;
            Object.assign(f, floats.snapRect(zone, floats.state.area));
            floats.save();
            return true;
        },

        // ---- gestures: the pointer handlers of m-float call these, and a spec can too ----

        // A drag of the title bar from (cx, cy): move(x, y) for each pointer move, end(x, y) at the drop. A window maximized or
        // snapped takes its own size again under the pointer, at the same fraction of its width. The snap outline shows while
        // the pointer is in a zone; the drop takes it.
        beginMove(id, cx, cy) {
            const f = floats.get(id);
            const s = floats.state;
            const gesture = { moved: false, zone: null, move() {}, end() { return gesture; }, cancel() {} };
            if (!f) return gesture;
            const min = (floats.kinds[f.kind] || {}).minSize;
            let start = rectOf(f.maximized ? s.area : f);
            // where the kind docks, measured once the drag has begun
            let home;
            gesture.move = (x, y) => {
                if (!gesture.moved && Math.abs(x - cx) < SLOP && Math.abs(y - cy) < SLOP) return;
                if (!gesture.moved) {
                    gesture.moved = true;
                    if (f.maximized || f.restoreRect) {
                        const own = f.maximized ? rectOf(f) : f.restoreRect;
                        const fraction = start.w ? (cx - start.x) / start.w : 0.5;
                        start = { x: cx - fraction * own.w, y: start.y, w: own.w, h: own.h };
                        f.maximized = false;
                        f.restoreRect = null;
                        f.w = own.w;
                        f.h = own.h;
                    }
                }
                const rect = floats.clampRect({ x: start.x + x - cx, y: start.y + y - cy, w: start.w, h: start.h }, s.area, min);
                f.x = rect.x;
                f.y = rect.y;
                if (home === undefined) home = floats.homeOf(f);
                const zone = floats.snapZone(x, y, s.area);
                if (floats.atHome(home, zone, x, y)) {
                    gesture.zone = "home";
                    s.snap = { id, zone: "home", rect: rectOf(home.rect) };
                } else {
                    gesture.zone = zone;
                    s.snap = zone ? { id, zone, rect: floats.snapRect(zone, s.area) } : null;
                }
            };
            gesture.end = (x, y) => {
                if (num(x) && num(y)) gesture.move(x, y);
                s.snap = null;
                if (gesture.moved && gesture.zone === "home") {
                    // its place before the drag is the one kept for the next time it floats
                    Object.assign(f, floats.clampRect(start, s.area, min));
                    floats.dock(id);
                } else if (gesture.moved && gesture.zone) floats.snapTo(id, gesture.zone, start);
                else floats.save();
                return gesture;
            };
            gesture.cancel = () => { s.snap = null; Object.assign(f, floats.clampRect(start, s.area, min)); floats.save(); };
            return gesture;
        },
        // A resize by an edge or corner grip (n, s, e, w, ne, nw, se, sw), kept to the kind's minimum and the area.
        beginResize(id, edge, cx, cy) {
            const f = floats.get(id);
            const s = floats.state;
            const gesture = { moved: false, move() {}, end() { return gesture; }, cancel() {} };
            if (!f) return gesture;
            const min = (floats.kinds[f.kind] || {}).minSize || MIN_SIZE;
            const start = rectOf(f.maximized ? s.area : f);
            if (f.maximized) { f.maximized = false; Object.assign(f, start); }
            gesture.move = (x, y) => {
                gesture.moved = gesture.moved || Math.abs(x - cx) >= SLOP || Math.abs(y - cy) >= SLOP;
                if (!gesture.moved) return;
                Object.assign(f, floats.resized(start, edge, x - cx, y - cy, s.area, min));
                f.restoreRect = null;
            };
            gesture.end = (x, y) => { if (num(x) && num(y)) gesture.move(x, y); floats.save(); return gesture; };
            gesture.cancel = () => { Object.assign(f, start); floats.save(); };
            return gesture;
        },
        resized(start, edge, dx, dy, area, min) {
            let { x, y, w, h } = start;
            const right = start.x + start.w, bottom = start.y + start.h;
            if (edge.indexOf("e") >= 0) w = Math.min(Math.max(start.w + dx, min.w), area.x + area.w - start.x);
            if (edge.indexOf("w") >= 0) { x = Math.min(Math.max(start.x + dx, area.x), right - min.w); w = right - x; }
            if (edge.indexOf("s") >= 0) h = Math.min(Math.max(start.h + dy, min.h), area.y + area.h - start.y);
            if (edge.indexOf("n") >= 0) { y = Math.min(Math.max(start.y + dy, area.y), bottom - min.h); h = bottom - y; }
            return rectOf({ x, y, w, h });
        },
        // The keys of a focused title bar: the arrows move a step, with Shift they resize. Answers whether it took the key.
        keyMove(id, event) {
            const f = floats.get(id);
            const step = { ArrowLeft: [-STEP, 0], ArrowRight: [STEP, 0], ArrowUp: [0, -STEP], ArrowDown: [0, STEP] }[event && event.key];
            if (!f || !step || event.ctrlKey || event.metaKey || event.altKey) return false;
            const s = floats.state;
            const min = (floats.kinds[f.kind] || {}).minSize || MIN_SIZE;
            if (f.maximized) { f.maximized = false; }
            f.restoreRect = null;
            if (event.shiftKey) Object.assign(f, floats.resized(rectOf(f), "se", step[0], step[1], s.area, min));
            else Object.assign(f, floats.clampRect({ x: f.x + step[0], y: f.y + step[1], w: f.w, h: f.h }, s.area, min));
            floats.save();
            return true;
        },

        // ---- arranging (§10.3) ----

        // mode: tile, cascade, column (at the right), minimizeAll, or gather (every window of kind docked, shown, §10.4)
        arrange(mode, kind) {
            const s = floats.state;
            const area = s.area;
            if (mode === "gather") { floats.dockAll(true, kind); return true; }
            const list = s.floats.filter((f) => !f.hidden);
            if (!list.length) return false;
            if (mode === "minimizeAll") { list.forEach((f) => { f.minimized = true; }); floats.save(); return true; }
            const n = list.length;
            const place = (f, rect) => {
                f.minimized = false;
                f.maximized = false;
                f.restoreRect = null;
                Object.assign(f, floats.clampRect(rect, area, (floats.kinds[f.kind] || {}).minSize));
            };
            if (mode === "tile") {
                const cols = Math.ceil(Math.sqrt(n));
                const rows = Math.ceil(n / cols);
                const h = Math.floor((area.h - GAP * (rows + 1)) / rows);
                list.forEach((f, index) => {
                    const row = Math.floor(index / cols);
                    const inRow = row === rows - 1 ? n - cols * (rows - 1) : cols;
                    const col = index - row * cols;
                    const w = Math.floor((area.w - GAP * (inRow + 1)) / inRow);
                    place(f, { x: area.x + GAP + col * (w + GAP), y: area.y + GAP + row * (h + GAP), w, h });
                });
            } else if (mode === "cascade") {
                list.forEach((f, index) => {
                    const size = (floats.kinds[f.kind] || {}).defaultSize || { w: f.w, h: f.h };
                    const w = Math.min(size.w, area.w - 2 * GAP), h = Math.min(size.h, area.h - 2 * GAP);
                    const span = Math.max(1, Math.floor(Math.min(area.w - w, area.h - h) / CASCADE));
                    const offset = (index % span) * CASCADE;
                    place(f, { x: area.x + GAP + offset, y: area.y + GAP + offset, w, h });
                    f.z = index + 1;
                });
            } else if (mode === "column") {
                const w = Math.min(480, Math.floor(area.w / 2));
                const h = Math.floor((area.h - GAP * (n + 1)) / n);
                list.forEach((f, index) => place(f, { x: area.x + area.w - w - GAP, y: area.y + GAP + index * (h + GAP), w, h }));
            } else return false;
            floats.save();
            return true;
        },

        // ---- kept and restored (§10.9, D13) ----

        // the windows (place, size, order, minimized, maximized, hidden) and the places of the ones docked, in localStorage;
        // none when storage is off
        save() {
            const store = storage();
            if (!store) return;
            const s = floats.state;
            const out = { floats: s.floats.map((f) => Object.assign({ kind: f.kind, key: f.key, z: f.z, minimized: f.minimized, maximized: f.maximized, hidden: f.hidden },
                rectOf(f.restoreRect || f))), places: floats.saved.places };
            s.floats.forEach((f) => { out.places[placeKey(f.kind, f.key)] = rectOf(f.restoreRect || f); });
            try { store.setItem(storeKey(), JSON.stringify(out)); } catch (err) { /* full or off */ }
        },
        // what the storage has, once (and again with force, for a spec)
        load(force) {
            if (floats.loaded && !force) return;
            floats.loaded = true;
            floats.saved = { places: {} };
            floats.restoring = {};
            const store = storage();
            let kept = null;
            try { kept = store ? JSON.parse(store.getItem(storeKey()) || "null") : null; } catch (err) { kept = null; }
            if (!kept || typeof kept !== "object") return;
            const places = kept.places && typeof kept.places === "object" ? kept.places : {};
            Object.keys(places).forEach((pk) => {
                const place = places[pk];
                if (place && num(place.x) && num(place.y) && num(place.w) && num(place.h)) floats.saved.places[pk] = rectOf(place);
            });
            (Array.isArray(kept.floats) ? kept.floats : []).forEach((f) => {
                if (!f || typeof f.kind !== "string") return;
                const pk = placeKey(f.kind, f.key);
                floats.restoring[pk] = { z: num(f.z) ? f.z : 0, minimized: !!f.minimized, maximized: !!f.maximized, hidden: !!f.hidden };
                if (num(f.x) && num(f.y) && num(f.w) && num(f.h)) floats.saved.places[pk] = rectOf(f);
            });
        }
    };
    moqui.floats = floats;
    floats.measure();

    // ---- the components (registered by WebrootVue.qvt.js) ----

    const components = {};
    const labelsOf = (vm) => {
        const shell = vm && vm.$root && vm.$root.labels ? vm.$root.labels : {};
        const out = Object.assign({}, floats.labels);
        Object.keys(floats.labels).forEach((key) => { if (shell["float_" + key]) out[key] = shell["float_" + key]; });
        return out;
    };
    // the drag of a bar or a grip, by mouse, touch or pen (MoquiLib.js)
    const primary = (event) => moqui.pointer.primary(event);
    const track = (event, gesture, after) => moqui.pointer.track(event, gesture, after);

    // The layer: a fixed layer over the shell that draws the windows, the snap outline of a drag and the strip at its bottom
    // (the Arrange menu and the minimized windows as chips). It holds no content. It keeps the windows inside a resized
    // viewport and docks them on a handheld screen.
    components["m-float-layer"] = {
        name: "mFloatLayer",
        data() { return { s: floats.state }; },
        computed: {
            labels() { return labelsOf(this); },
            chips() { return this.s.floats.filter((f) => f.minimized && !f.hidden); },
            anyShown() { return this.s.floats.some((f) => !f.hidden); },
            // the kinds with a Gather item and a window open
            gatherKinds() {
                return Object.keys(floats.kinds).filter((kind) => floats.kinds[kind].gatherLabel && this.s.floats.some((f) => f.kind === kind))
                    .map((kind) => ({ kind, label: valueOf(floats.kinds[kind].gatherLabel) }));
            },
            handheld() { return !!(this.$root && this.$root.handheld); }
        },
        watch: { handheld: { immediate: true, handler(on) { floats.setHandheld(on); } } },
        methods: {
            arrange(mode, kind) { floats.arrange(mode, kind); },
            restore(id) { floats.focus(id); },
            titleOf(f) { return floats.titleOf(f); },
            iconOf(f) { return floats.iconOf(f); },
            badgeOf(f) { return floats.badgeOf(f); },
            rectStyle(rect) { return { left: rect.x + "px", top: rect.y + "px", width: rect.w + "px", height: rect.h + "px" }; },
            onResize() {
                clearTimeout(this.resizeTimer);
                this.resizeTimer = setTimeout(() => floats.onViewport(), 100);
            }
        },
        mounted() {
            floats.measure();
            root.addEventListener("resize", this.onResize);
            // the Arrange items as key map actions: no keys of their own, a KEYMAP preference gives them some
            // ({"global": {"floatsTile": "Mod+Alt+T"}}, docs/ui/Qvt-Client.md "Keys")
            const keymap = moqui.keymap;
            const own = keymap && keymap.overrides ? keymap.overrides.global || {} : {};
            if (keymap) this.keysOff = keymap.registerScope("global", ["tile", "cascade", "column", "minimizeAll"].map((mode) => {
                const action = "floats" + mode.charAt(0).toUpperCase() + mode.slice(1);
                return { keys: own[action] || "", action, label: this.labels[mode], run: () => floats.arrange(mode) };
            }));
        },
        unmounted() {
            root.removeEventListener("resize", this.onResize);
            clearTimeout(this.resizeTimer);
            if (this.keysOff) this.keysOff();
        },
        template:
            '<div v-if="s.floats.length" class="m-float-layer" data-float-layer>' +
                '<m-float v-for="f in s.floats" :key="f.id" :float="f"></m-float>' +
                '<div v-if="s.snap" class="m-float-snap" :class="{\'m-float-snap--home\': s.snap.zone === \'home\'}" :style="rectStyle(s.snap.rect)" aria-hidden="true" data-float-snap :data-float-snap-zone="s.snap.zone">' +
                    '<span v-if="s.snap.zone === \'home\'" class="m-float-snap-label">{{labels.dock}}</span></div>' +
                '<div v-if="anyShown" class="m-float-strip row items-center no-wrap" role="toolbar" :aria-label="labels.windows">' +
                    '<q-btn flat round dense size="sm" icon="sym_o_view_quilt" id="m-float-arrange" class="m-float-strip-btn" :aria-label="labels.arrange">' +
                        '<q-tooltip>{{labels.arrange}}</q-tooltip>' +
                        '<q-menu anchor="top left" self="bottom left"><q-list dense style="min-width:200px;">' +
                            '<q-item clickable v-close-popup data-float-arrange="tile" @click="arrange(\'tile\')"><q-item-section avatar><q-icon name="sym_o_grid_view"></q-icon></q-item-section><q-item-section>{{labels.tile}}</q-item-section></q-item>' +
                            '<q-item clickable v-close-popup data-float-arrange="cascade" @click="arrange(\'cascade\')"><q-item-section avatar><q-icon name="sym_o_stacks"></q-icon></q-item-section><q-item-section>{{labels.cascade}}</q-item-section></q-item>' +
                            '<q-item clickable v-close-popup data-float-arrange="column" @click="arrange(\'column\')"><q-item-section avatar><q-icon name="sym_o_view_column"></q-icon></q-item-section><q-item-section>{{labels.column}}</q-item-section></q-item>' +
                            '<q-item clickable v-close-popup data-float-arrange="minimizeAll" @click="arrange(\'minimizeAll\')"><q-item-section avatar><q-icon name="sym_o_minimize"></q-icon></q-item-section><q-item-section>{{labels.minimizeAll}}</q-item-section></q-item>' +
                            '<template v-if="gatherKinds.length"><q-separator></q-separator>' +
                                '<q-item v-for="one in gatherKinds" :key="one.kind" clickable v-close-popup :data-float-arrange="\'gather-\' + one.kind" @click="arrange(\'gather\', one.kind)">' +
                                    '<q-item-section avatar><q-icon name="sym_o_dock_to_right"></q-icon></q-item-section><q-item-section>{{one.label}}</q-item-section></q-item></template>' +
                        '</q-list></q-menu></q-btn>' +
                    '<q-chip v-for="f in chips" :key="f.id" clickable dense square :icon="iconOf(f)" class="m-float-chip" :data-float-chip="f.id" :aria-label="labels.show + \' \' + titleOf(f)" @click="restore(f.id)">' +
                        '<span class="ellipsis">{{titleOf(f)}}</span><q-badge v-if="badgeOf(f)" color="negative" rounded class="q-ml-xs">{{badgeOf(f)}}</q-badge></q-chip>' +
                '</div>' +
            '</div>'
    };

    // One window (§10.2): a non-modal dialog labelled by its title. The title bar moves it (pointer capture, so a drag outside
    // the window keeps going) and its arrow keys move or resize it; a double click maximizes and restores. A click anywhere
    // raises it; Esc is the content's, never the window's. The body is empty: its owner's teleport fills it.
    components["m-float"] = {
        name: "mFloat",
        props: { float: { type: Object, required: true } },
        data() { return { s: floats.state, edges: EDGES }; },
        computed: {
            labels() { return labelsOf(this); },
            title() { return floats.titleOf(this.float); },
            icon() { return floats.iconOf(this.float); },
            badge() { return floats.badgeOf(this.float); },
            dot() { return floats.dotOf(this.float); },
            actions() { return floats.actionsOf(this.float); },
            domId() { return floats.domId(this.float.id); },
            bodyId() { return floats.bodyId(this.float.id); },
            top() { return floats.isTop(this.float); },
            // hidden or minimized: kept laid out but invisible (visibility, not display), so the content keeps its scroll
            shown() { return floats.shown(this.float); },
            frame() {
                const f = this.float;
                const rect = f.maximized ? this.s.area : f;
                return { left: rect.x + "px", top: rect.y + "px", width: rect.w + "px", height: rect.h + "px", zIndex: f.z };
            }
        },
        methods: {
            raise() { floats.raise(this.float.id); },
            minimize() { floats.minimize(this.float.id); },
            toggleMax() { floats.maximize(this.float.id); },
            dock() { floats.dock(this.float.id); },
            close() { floats.close(this.float.id); },
            act(action) { action.run(this.float.key, this.float); },
            onBarKey(event) { if (floats.keyMove(this.float.id, event)) event.preventDefault(); },
            startMove(event) {
                if (!primary(event) || (event.target && event.target.closest && event.target.closest(".q-btn, button, a, input"))) return;
                const id = this.float.id;
                track(event, floats.beginMove(id, event.clientX, event.clientY), (gesture) => { if (!gesture.moved) floats.focusContent(id); });
            },
            startResize(edge, event) {
                if (!primary(event)) return;
                event.preventDefault();
                track(event, floats.beginResize(this.float.id, edge, event.clientX, event.clientY), null);
            }
        },
        mounted() { floats.bodyReady(this.float.id, true); },
        beforeUnmount() { floats.bodyReady(this.float.id, false); },
        template:
            '<section class="m-float column no-wrap" :class="{\'m-float--top\': top, \'m-float--max\': float.maximized, \'m-float--hidden\': !shown}" :style="frame"' +
                    ' :aria-hidden="shown ? undefined : \'true\'"' +
                    ' role="dialog" aria-modal="false" :aria-label="title" :id="domId" :data-float-kind="float.kind" :data-float-key="float.key" @pointerdown.capture="raise">' +
                '<div class="m-float-bar row items-center no-wrap" tabindex="0" role="group" :aria-label="title + \': \' + labels.moveKeys" data-float-bar' +
                        ' @pointerdown="startMove" @dblclick="toggleMax" @keydown="onBarKey">' +
                    '<span v-if="dot" class="m-float-dot" :class="\'bg-\' + dot" aria-hidden="true"></span>' +
                    '<q-icon v-else :name="icon" size="18px" class="m-float-icon" aria-hidden="true"></q-icon>' +
                    '<div class="col ellipsis m-float-title">{{title}}</div>' +
                    '<q-badge v-if="badge" color="negative" rounded class="q-mx-xs" data-float-badge>{{badge}}</q-badge>' +
                    '<q-btn v-for="action in actions" :key="action.label" flat round dense size="sm" :icon="action.icon" :aria-label="action.label" :data-float-action="action.id || action.label" @click="act(action)"><q-tooltip>{{action.label}}</q-tooltip></q-btn>' +
                    '<q-btn flat round dense size="sm" icon="sym_o_minimize" :aria-label="labels.minimize" data-float-action="minimize" @click="minimize"><q-tooltip>{{labels.minimize}}</q-tooltip></q-btn>' +
                    '<q-btn flat round dense size="sm" :icon="float.maximized ? \'sym_o_close_fullscreen\' : \'sym_o_open_in_full\'" :aria-label="float.maximized ? labels.restore : labels.maximize"' +
                        ' data-float-action="maximize" @click="toggleMax"><q-tooltip>{{float.maximized ? labels.restore : labels.maximize}}</q-tooltip></q-btn>' +
                    '<q-btn flat round dense size="sm" icon="sym_o_dock_to_right" :aria-label="labels.dock" data-float-action="dock" @click="dock"><q-tooltip>{{labels.dock}}</q-tooltip></q-btn>' +
                    '<q-btn flat round dense size="sm" icon="sym_o_close" :aria-label="labels.close" data-float-action="close" @click="close"><q-tooltip>{{labels.close}}</q-tooltip></q-btn>' +
                '</div>' +
                '<div class="m-float-body" :id="bodyId" data-float-body></div>' +
                '<div v-for="edge in edges" :key="edge" class="m-float-grip" :class="\'m-float-grip--\' + edge" :data-float-grip="edge" aria-hidden="true" @pointerdown="startResize(edge, $event)"></div>' +
            '</section>'
    };

    // Before WebrootVue.qvt.js its registry takes them; after it they are registered late on the running application.
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(components).forEach((name) => qvt.registerLate(name, components[name]));
    else {
        const registry = moqui.qvtComponents = moqui.qvtComponents || {};
        Object.keys(components).forEach((name) => { registry[name] = components[name]; });
    }
    floats.components = components;

    if (typeof module !== "undefined" && module.exports) module.exports = floats;
})(globalThis);
