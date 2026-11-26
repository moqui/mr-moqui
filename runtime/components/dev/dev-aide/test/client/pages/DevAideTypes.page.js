/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The TypeScript checker (DevAideTypes.js and its worker DevAideTsWorker.js, plans/design-dev-aide.md 5.4), in the page:
// which files are server scripts, the wrapping as file_function runs them, diagnostics as the editor's problems, and the
// worker on the vendored compiler: a type error in a script's buffer is a problem at its line, a top-level return is
// none, and a name the script reads from its context is information. The declarations are a small stand-in for the
// project's `moqui types` files, so the check stays well under a second.
"use strict";

const types = moqui.devAide.types;

test("a server script is a JavaScript file under script/, wrapped as the function body it runs as", () => {
    assert.ok(types.isScript("script/org/moqui/Run.js"));
    assert.ok(types.isScript("runtime/component/app/script/Run.qjs"));
    assert.ok(!types.isScript("screen/devstatic/DevAide.js"), "a browser file");
    assert.ok(!types.isScript("service/Things.xml"));
    assert.ok(!types.isScript("scripts/build.js"), "not a component's script/");
    assert.equal(types.wrap("return 1"), "(function(){return 1\n})()");
    // the problems are the text's own: line and column after the wrapper, severities by kind
    const text = "const a = 1\nec.entity.fnd(a)";
    const at = types.PREFIX.length + text.indexOf("fnd");
    const problems = types.problems([
        { start: at, length: 3, code: 2339, syntactic: false, message: "Property 'fnd' does not exist" },
        { start: types.PREFIX.length + 6, length: 1, code: 2304, syntactic: false, message: "Cannot find name 'a'" },
        { start: 3, length: 1, code: 1005, syntactic: true, message: "in the wrapper" },
        { start: types.PREFIX.length, length: 1, code: 1005, syntactic: true, message: "';' expected" }
    ], text);
    assert.deepEqual(problems.map((problem) => [problem.line, problem.column, problem.severity, problem.rule]),
        [[2, 11, "warning", "ts2339"], [1, 7, "info", "ts2304"], [1, 1, "error", "ts1005"]]);
});

test("the worker checks a script's buffer against the declarations: a type error is a problem at its line", async () => {
    const declarations = [{ name: "moqui-host.d.ts", text: "interface MoquiEntityFind { one(): Record<string, unknown> | null }\n" +
        "interface MoquiEntityFacade { find(entityName: string): MoquiEntityFind }\n" +
        "interface MoquiServerEc { entity: MoquiEntityFacade }\n" },
        { name: "moqui-globals.d.ts", text: "declare const ec: MoquiServerEc;\n" }];
    let asked = 0;
    const checker = new types.Checker({ worker: "/files/component/dev-aide/screen/devstatic/DevAideTsWorker.js",
        compiler: "/libs/typescript/typescript.min.js", libs: "/libs/typescript/lib-es2023.js",
        declarations: (known) => { asked++; return Promise.resolve(known === "h1" ? { hash: "h1" } : { hash: "h1", files: declarations }); } });
    try {
        const problems = await checker.check("const order = ec.entity.find(\"Order\").one()\nconst wrong = ec.entity.fnd(\"Order\")\nreturn order");
        // 2339 no such property, or 2551 with a "did you mean"
        const wrong = problems.find((problem) => problem.rule === "ts2339" || problem.rule === "ts2551");
        assert.ok(wrong, JSON.stringify(problems));
        assert.equal(wrong.line, 2);
        assert.ok(wrong.message.includes("fnd"), wrong.message);
        // a return at the top is the script's value, not an error: the file is a function's body
        assert.ok(!problems.some((problem) => problem.rule === "ts1108"), JSON.stringify(problems));
        // a name from the context is information; a clean buffer has nothing
        const named = await checker.check("const id = orderId\nec.entity.find(id)");
        assert.deepEqual(named.map((problem) => [problem.severity, problem.rule]), [["info", "ts2304"]]);
        assert.deepEqual(await checker.check("return ec.entity.find(\"Order\").one()"), []);
        // a syntax error is an error, and the declarations were read once
        const broken = await checker.check("const = 1");
        assert.ok(broken.length && broken.every((problem) => problem.severity === "error"), JSON.stringify(broken));
        assert.equal(asked, 1);
    } finally {
        checker.stop();
    }
});
