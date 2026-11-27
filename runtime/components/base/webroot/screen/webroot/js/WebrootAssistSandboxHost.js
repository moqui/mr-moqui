/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
/*
 * The holder of a Vue SFC canvas's frame (GET /assist/sandboxHost; docs/ai/Assist.md "Vue SFC canvases"), on this
 * site's origin and framed by the drawer. It frames the sandbox (GET /assist/sandbox, an opaque origin) with its own
 * query and fragment, and hands it the drawer's port once both have loaded. Its policy lets its frame load only
 * /assist/sandbox, and a browser holds a frame's own navigations to the policy of the document that frames it, so the
 * component cannot navigate its frame to another site with what it read (plans/archive/Security-Review-Medium.md SEC-AI-81).
 * A second load of the frame is the component navigating it: this document then leaves, which the drawer's watchdog
 * sees as a load of its frame and closes the canvas. Nothing from the frame is read or answered here.
 */
(function () {
    "use strict";
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.setAttribute("allow", "");
    frame.setAttribute("title", "Canvas");
    let loads = 0;
    let waiting = null;
    const hand = (event) => {
        try { frame.contentWindow.postMessage(event.data, "*", Array.from(event.ports || [])); } catch (err) { /* gone */ }
    };
    frame.addEventListener("load", () => {
        loads += 1;
        if (loads > 1) { window.location.replace("about:blank"); return; }
        if (waiting) { hand(waiting); waiting = null; }
    });
    window.addEventListener("message", (event) => {
        if (event.source !== window.parent || !event.data || event.data.t !== "port" || !event.ports || !event.ports[0]) return;
        if (loads === 1) hand(event);
        else if (loads === 0) waiting = event;
    });
    frame.src = "/assist/sandbox" + window.location.search + window.location.hash;
    document.body.appendChild(frame);
})();
