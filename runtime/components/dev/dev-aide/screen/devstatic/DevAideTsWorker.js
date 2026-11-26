/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The TypeScript checker's worker (plans/design-dev-aide.md 5.4, question 10): the compiler (libs/typescript, vendored by
 * build-js) over the ES2023 declarations and the project's `moqui types` files, checking one JavaScript buffer at a
 * time as checkJs does. Messages, each answered with its id:
 *   {type: "init", id, compiler, libs}: importScripts of the compiler and of the declarations script (self.MoquiTsLibs)
 *   {type: "types", id, files: [{name, text}]}: the project's declarations, in place of any before
 *   {type: "check", id, text}: the buffer's diagnostics, [{start, length, code, category, message}] in its offsets
 * The declarations are parsed once and kept; each check is a program over them that reuses the last one, so only the
 * buffer is parsed again. Started by DevAideTypes.js on the first script tab with the check on, ended when idle.
 */
"use strict";

// the compiler; its script declares the global ts, so this file's own name is another
let compiler = null;
const files = new Map(); // name -> {text, source}
let typeNames = [];
let previous = null;
const BUFFER = "/src/script.js";
const LIB_DIR = "/lib/";

const options = () => ({
    target: compiler.ScriptTarget.ES2022, lib: ["lib.es2023.d.ts"], allowJs: true, checkJs: true, noEmit: true, strict: false,
    noImplicitAny: false, skipLibCheck: true, noResolve: true, types: [], newLine: compiler.NewLineKind.LineFeed
});

function put(name, text) {
    const known = files.get(name);
    if (known && known.text === text) return;
    files.set(name, { text, source: null });
}

const host = {
    getSourceFile(name, languageVersion) {
        const file = files.get(name);
        if (!file) return undefined;
        if (!file.source) file.source = compiler.createSourceFile(name, file.text, languageVersion, true);
        return file.source;
    },
    getDefaultLibFileName: () => LIB_DIR + "lib.es2023.d.ts",
    getDefaultLibLocation: () => "/lib",
    writeFile() {},
    getCurrentDirectory: () => "/",
    getDirectories: () => [],
    fileExists: (name) => files.has(name),
    readFile: (name) => (files.has(name) ? files.get(name).text : undefined),
    getCanonicalFileName: (name) => name,
    useCaseSensitiveFileNames: () => true,
    getNewLine: () => "\n",
    directoryExists: () => true
};

function init(message) {
    if (compiler) return {};
    importScripts(message.compiler, message.libs);
    compiler = self.ts;
    if (!compiler || !compiler.createProgram) throw new Error("the TypeScript compiler did not load");
    for (const [name, text] of Object.entries(self.MoquiTsLibs || {})) put(LIB_DIR + name, text);
    return { version: compiler.version };
}

function types(message) {
    for (const name of typeNames) files.delete(name);
    typeNames = (message.files || []).filter((file) => /\.d\.ts$/.test(file.name)).map((file) => {
        const name = "/types/" + file.name;
        put(name, String(file.text || ""));
        return name;
    });
    previous = null;
    return { files: typeNames.length };
}

function check(message) {
    if (!compiler) throw new Error("the checker is not started");
    put(BUFFER, String(message.text || ""));
    const program = compiler.createProgram({ rootNames: typeNames.concat([BUFFER]), options: options(), host, oldProgram: previous || undefined });
    previous = program;
    const source = program.getSourceFile(BUFFER);
    const syntax = program.getSyntacticDiagnostics(source);
    // a buffer that does not parse has only its syntax errors: the types of what it means are not asked
    const found = syntax.length ? syntax : program.getSemanticDiagnostics(source);
    return {
        diagnostics: found.filter((item) => item.file && item.file.fileName === BUFFER).map((item) => ({
            start: item.start || 0, length: item.length || 0, code: item.code,
            category: item.category === compiler.DiagnosticCategory.Error ? "error" : item.category === compiler.DiagnosticCategory.Warning ? "warning" : "info",
            syntactic: syntax.length > 0,
            message: compiler.flattenDiagnosticMessageText(item.messageText, "\n")
        }))
    };
}

self.onmessage = (event) => {
    const message = event.data || {};
    let answer;
    try {
        if (message.type === "init") answer = init(message);
        else if (message.type === "types") answer = types(message);
        else if (message.type === "check") answer = check(message);
        else throw new Error("no message " + message.type);
    } catch (error) {
        answer = { error: String((error && error.message) || error) };
    }
    self.postMessage(Object.assign({ id: message.id }, answer));
};
