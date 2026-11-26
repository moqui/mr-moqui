/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The Workspace's TypeScript checker (plans/design-dev-aide.md 5.4, question 10; the browser path, as the framework does
 * not take TypeScript for scripts): a worker (DevAideTsWorker.js) runs the vendored compiler (libs/typescript, 3.6 MB,
 * with the ES2023 declarations) over the project's `moqui types` declarations (the Workspace's typeDeclarations
 * transition) and checks a server script's buffer, its diagnostics going into m-code-editor's lint and Problems. Off by
 * default: the editor setting typeCheck (DEV_AIDE_EDITOR) turns it on, and nothing loads until the first script tab
 * with it on asks. A script file at a location runs as the body of a function (core/script/parse.rs file_function), so
 * it is checked wrapped as that does: a return at its top is the script's value, its declarations its own. The worker
 * ends after a while unused, giving back its memory, and starts again on the next check.
 *
 * moqui.devAide.types: the pure parts (which files, the wrapping, diagnostics as problems), and Checker.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const devAide = moqui.devAide = moqui.devAide || {};
    const types = devAide.types = devAide.types || {};

    // A server script: a JavaScript file under a component's script/ directory (component://<name>/script/...), the
    // files a service or an action runs by location. A browser file is not one: it is not checked against the server's
    // declarations.
    types.isScript = (path) => /(^|\/)script\/(.+\/)?[^/]+\.(js|qjs|mjs|cjs)$/i.test(String(path || ""));
    // The wrapper of file_function: the script as a function's body, opening on its first line so its lines are the
    // file's (a last expression's return, which file_function adds, changes no type).
    types.PREFIX = "(function(){";
    types.wrap = (text) => types.PREFIX + String(text || "") + "\n})()";
    // Diagnostics with names a script reads from the context it runs in (an in-parameter, a name the caller set) are
    // shown as information, not as errors: 2304 cannot find name, 2552 cannot find name (did you mean).
    const CONTEXT_NAMES = [2304, 2552];
    // The worker's diagnostics ({start, length, code, category, syntactic, message}, offsets in the wrapped text) as the
    // editor's problems of `text`: 1-based line and column, a syntax error an error, a type's a warning, a context name
    // information; the wrapper's own text gives none.
    types.problems = (diagnostics, text) => {
        const source = String(text || "");
        const offset = types.PREFIX.length;
        const out = [];
        for (const item of diagnostics || []) {
            const at = (item.start || 0) - offset;
            if (at < 0 || at > source.length) continue;
            const before = source.slice(0, at);
            const line = before.split("\n").length;
            const column = at - before.lastIndexOf("\n");
            const severity = item.syntactic ? "error" : CONTEXT_NAMES.indexOf(item.code) >= 0 ? "info" : "warning";
            out.push({ line, column, severity, message: item.message, rule: "ts" + item.code });
        }
        return out;
    };

    // The checker of a page. options: worker, compiler, libs (the URLs of DevAideTsWorker.js, libs/typescript's
    // typescript.min.js and lib-es2023.js), declarations(knownHash) (a promise of {hash, files} from typeDeclarations;
    // files left out when knownHash is still theirs), idleMs (5 minutes), timeoutMs (60 s for an answer).
    class Checker {
        constructor(options) {
            this.options = options;
            this.next = 0;
            this.waiting = new Map();
            this.worker = null;
            this.ready = null;
            this.hash = null;
        }
        call(message) {
            return new Promise((resolve, reject) => {
                const id = ++this.next;
                const timer = setTimeout(() => {
                    this.waiting.delete(id);
                    reject(new Error("the type checker did not answer"));
                    this.stop();
                }, this.options.timeoutMs || 60000);
                this.waiting.set(id, { resolve, reject, timer });
                this.worker.postMessage(Object.assign({ id }, message));
            });
        }
        // the worker, the compiler and the project's declarations, once (again after it stopped)
        start() {
            if (this.ready) return this.ready;
            const worker = this.worker = new Worker(this.options.worker);
            worker.onmessage = (event) => {
                const answer = event.data || {};
                const entry = this.waiting.get(answer.id);
                if (!entry) return;
                this.waiting.delete(answer.id);
                clearTimeout(entry.timer);
                if (answer.error) entry.reject(new Error(answer.error));
                else entry.resolve(answer);
            };
            worker.onerror = (event) => {
                for (const entry of this.waiting.values()) { clearTimeout(entry.timer); entry.reject(new Error(event.message || "the type checker failed")); }
                this.waiting.clear();
                this.stop();
            };
            this.hash = null;
            this.ready = this.call({ type: "init", compiler: this.options.compiler, libs: this.options.libs })
                .then(() => this.refresh())
                .catch((error) => { this.stop(); throw error; });
            return this.ready;
        }
        // the declarations again when they changed (their hash)
        refresh() {
            return Promise.resolve(this.options.declarations(this.hash)).then((answer) => {
                if (!answer || !answer.files || answer.hash === this.hash) return;
                this.hash = answer.hash;
                return this.call({ type: "types", files: answer.files });
            });
        }
        // the problems of `text`, a script's buffer
        check(text) {
            clearTimeout(this.idleTimer);
            return this.start().then(() => this.call({ type: "check", text: types.wrap(text) })).then((answer) => {
                this.idleTimer = setTimeout(() => this.stop(), this.options.idleMs || 300000);
                return types.problems(answer.diagnostics, text);
            });
        }
        stop() {
            clearTimeout(this.idleTimer);
            if (this.worker) this.worker.terminate();
            this.worker = null;
            this.ready = null;
            for (const entry of this.waiting.values()) { clearTimeout(entry.timer); entry.reject(new Error("the type checker stopped")); }
            this.waiting.clear();
        }
    }
    types.Checker = Checker;
    types.loaded = true;
})(typeof window !== "undefined" ? window : globalThis);
