// WebrootAssistCanvas.js assigns module.exports and then returns when Vue is absent.
// moqui.assist.canvas is set only after the renderer, which these tests leave off.
globalThis.module = { exports: {} };
