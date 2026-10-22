// The globals of an agent spec file (test/agent/*.agent.js): agentSpec() and the checks of a task. The harness
// (testkit/agents) evaluates the test prelude, then this, then the file, and drives the tasks through __agentEval: their
// plan as JSON, their setup and cleanup functions, and the checks over what a run did. Regular expressions are tested
// here, so a check may use any JavaScript RegExp.
(function (g) {
    const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
    const SPEC_FIELDS = ["agent", "surface", "user", "tasks", "timeout", "requires", "setup", "cleanup"];
    const TASK_FIELDS = ["name", "start", "user", "say", "turns", "approvals", "answers", "expect", "verify", "setup",
        "cleanup", "timeout", "ignore", "judge"];
    const TURN_FIELDS = ["say", "expect"];
    const EXPECT_FIELDS = ["path", "parameters", "values", "answer", "answerNot", "called", "notCalled", "results",
        "held", "noWrite", "maxWrongCalls", "maxToolCalls", "questions", "state", "noInvented"];
    const SURFACES = ["drawer", "chat", "tui", "job", "a2a", "mcp"];

    const A = {};
    define("__agentEval", A);
    A.reset = (file) => {
        A.file = file;
        A.spec = null;
        A.errors = [];
    };
    A.reset(null);

    const message = (e) => (e && e.message ? e.message : String(e));
    const show = (value) => {
        if (value instanceof RegExp) return String(value);
        if (value === undefined) return "nothing";
        try { return JSON.stringify(value, (key, item) => (item instanceof RegExp ? String(item) : item)); } catch (e) { return String(value); }
    };
    const checkFields = (where, object, allowed) => {
        for (const key of Object.keys(object || {})) if (!allowed.includes(key)) A.errors.push(`${where} has an unknown field ${key}`);
    };

    // ---- registration ----

    define("agentSpec", (spec) => {
        if (A.spec) A.errors.push("agentSpec() is called more than once");
        if (!spec || typeof spec !== "object") { A.errors.push("agentSpec() needs an object"); return; }
        checkFields("agentSpec()", spec, SPEC_FIELDS);
        if (spec.surface && !SURFACES.includes(spec.surface)) A.errors.push(`agentSpec() surface ${spec.surface} is not one of ${SURFACES.join(", ")}`);
        if (!Array.isArray(spec.tasks) || !spec.tasks.length) A.errors.push("agentSpec() needs tasks, an array of task objects");
        const names = new Set();
        (spec.tasks || []).forEach((task, index) => {
            const where = `task ${task && task.name ? `"${task.name}"` : index + 1}`;
            if (!task || typeof task !== "object") { A.errors.push(`${where} is not an object`); return; }
            checkFields(where, task, TASK_FIELDS);
            if (typeof task.name !== "string" || !task.name) A.errors.push(`${where} needs a name`);
            else if (names.has(slug(task.name))) A.errors.push(`${where}: another task has the same name`);
            names.add(slug(task.name || ""));
            if (task.turns && task.say) A.errors.push(`${where} has both say and turns`);
            if (!task.turns && !task.say) A.errors.push(`${where} needs say or turns`);
            if (task.turns) {
                if (!Array.isArray(task.turns) || !task.turns.length) A.errors.push(`${where}: turns is not an array of turns`);
                else task.turns.forEach((turn, at) => {
                    checkFields(`${where} turn ${at + 1}`, turn, TURN_FIELDS);
                    if (!turn || typeof turn.say !== "string" || !turn.say) A.errors.push(`${where} turn ${at + 1} needs say`);
                    if (turn && turn.expect) checkFields(`${where} turn ${at + 1} expect`, turn.expect, EXPECT_FIELDS);
                });
            }
            if (task.expect) checkFields(`${where} expect`, task.expect, EXPECT_FIELDS);
            for (const fn of ["verify", "setup", "cleanup"]) {
                if (task[fn] != null && typeof task[fn] !== "function") A.errors.push(`${where}: ${fn} is not a function`);
            }
            (task.approvals || []).forEach((answer, at) => {
                if (approval(answer) == null) A.errors.push(`${where} approval ${at + 1} is not "approve", {edit: {field: value}} or {reject: "feedback"}`);
            });
            if (task.judge) A.errors.push(`${where}: the LLM judge is not built; leave judge out`);
        });
        A.spec = spec;
    });

    // A task's file name: its name in lower case, words joined by -.
    const slug = (name) => String(name).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

    // A scripted answer to an approval: approve, edit with new values, or reject with feedback.
    const approval = (answer) => {
        if (answer === "approve" || (answer && answer.approve === true)) return { kind: "approve" };
        if (answer && answer.edit && typeof answer.edit === "object") return { kind: "edit", values: answer.edit };
        if (answer && typeof answer.reject === "string") return { kind: "reject", feedback: answer.reject };
        if (answer === "reject") return { kind: "reject", feedback: "" };
        return null;
    };

    const start = (value) => {
        if (value == null) return null;
        if (typeof value === "string") return { path: value };
        return { path: String(value.path || ""), parameters: value.parameters || {}, title: value.title || null };
    };

    // How many writes the task expects held for approval, for the metrics: its expect's number, else its turns'.
    const expectedHeld = (task) => {
        if (task.expect && typeof task.expect.held === "number") return task.expect.held;
        if (task.expect && task.expect.held != null && typeof task.expect.held === "object") return 1;
        if (task.expect && task.expect.noWrite) return 0;
        const turns = (task.turns || []).filter((turn) => turn.expect && typeof turn.expect.held === "number");
        return turns.length ? turns.reduce((sum, turn) => sum + turn.expect.held, 0) : null;
    };

    A.plan = () => {
        const spec = A.spec || {};
        const tasks = (spec.tasks || []).map((task) => ({
            name: task.name,
            slug: slug(task.name),
            start: start(task.start),
            user: task.user || null,
            turns: task.turns ? task.turns.map((turn) => ({ say: turn.say }))
                : (Array.isArray(task.say) ? task.say : [task.say]).map((say) => ({ say: String(say) })),
            approvals: (task.approvals || []).map(approval).filter((answer) => answer != null),
            answers: (task.answers || []).map(String),
            timeout: task.timeout || null,
            ignore: task.ignore || null,
            expectHeld: expectedHeld(task),
        }));
        if (!A.spec) A.errors.push("the file does not call agentSpec()");
        return JSON.stringify({
            errors: A.errors,
            spec: { agent: spec.agent || "assist", surface: spec.surface || "drawer", user: spec.user || "john.doe",
                timeout: spec.timeout || null, requires: spec.requires || [] },
            tasks,
        });
    };

    // ---- setup and cleanup ----

    const run = (fn) => {
        if (typeof fn !== "function") return JSON.stringify({});
        try { fn(); return JSON.stringify({}); } catch (e) { return JSON.stringify({ error: message(e) }); }
    };
    A.specPhase = (name) => run(A.spec && A.spec[name]);
    A.taskPhase = (index, name) => run(A.spec && A.spec.tasks[index] && A.spec.tasks[index][name]);

    // ---- checks ----

    // Whether `actual` is what `expected` asks for: a RegExp tests the text, an object its keys (partly), an array each
    // item at least once, null an empty value, anything else equal as text.
    const fits = (expected, actual) => {
        if (expected instanceof RegExp) return actual != null && expected.test(typeof actual === "string" ? actual : JSON.stringify(actual));
        if (expected === null) return actual == null || actual === "";
        if (Array.isArray(expected)) return Array.isArray(actual) && expected.every((item) => actual.some((one) => fits(item, one)));
        if (typeof expected === "object") {
            if (actual == null || typeof actual !== "object") return false;
            return Object.keys(expected).every((key) => fits(expected[key], actual[key]));
        }
        return actual != null && String(actual) === String(expected);
    };

    // A call that matches `want`: a tool name, or {tool, args, result}.
    const callMatches = (want, call) => {
        if (typeof want === "string") return call.tool === want;
        if (want.tool && call.tool !== want.tool) return false;
        if (want.args && !fits(want.args, call.arguments)) return false;
        if (want.result && !fits(want.result, call.result)) return false;
        if (want.error != null && Boolean(want.error) !== Boolean(call.error)) return false;
        return true;
    };

    // The failures of one expect over a view of the run: its answer, its calls, the held writes, the screen.
    const evaluate = (where, expect, view) => {
        const failures = [];
        const fail = (text) => failures.push(`${where}${text}`);
        const screen = view.screen || {};
        if (expect.path !== undefined && !fits(expect.path, screen.path)) {
            fail(`the screen is ${show(screen.path)}, expected ${show(expect.path)}`);
        }
        if (expect.parameters) {
            const parameters = screen.parameters || {};
            for (const key of Object.keys(expect.parameters)) {
                if (!fits(expect.parameters[key], parameters[key])) {
                    fail(`screen parameter ${key} is ${show(parameters[key])}, expected ${show(expect.parameters[key])}`);
                }
            }
        }
        if (expect.values) {
            const forms = screen.values || {};
            for (const key of Object.keys(expect.values)) {
                const want = expect.values[key];
                if (want && typeof want === "object" && !(want instanceof RegExp)) {
                    // a form's values
                    const form = forms[key] || {};
                    for (const field of Object.keys(want)) {
                        if (!fits(want[field], form[field])) fail(`form ${key} field ${field} is ${show(form[field])}, expected ${show(want[field])}`);
                    }
                } else {
                    // a field of any form
                    const found = Object.values(forms).map((form) => form[key]).filter((value) => value !== undefined);
                    if (!found.some((value) => fits(want, value))) fail(`no form has ${key} ${show(want)} (found ${show(found)})`);
                }
            }
        }
        const answers = expect.answer === undefined ? [] : [].concat(expect.answer);
        for (const pattern of answers) {
            if (!fits(pattern, view.answer || "")) fail(`the answer does not match ${show(pattern)}: ${show(view.answer)}`);
        }
        for (const pattern of expect.answerNot === undefined ? [] : [].concat(expect.answerNot)) {
            if (fits(pattern, view.answer || "")) fail(`the answer matches ${show(pattern)}, which it should not: ${show(view.answer)}`);
        }
        const calls = view.calls || [];
        for (const want of [].concat(expect.called || [])) {
            if (!calls.some((call) => callMatches(want, call))) fail(`no call matches ${show(want)}; calls: ${show(calls.map((call) => call.tool))}`);
        }
        for (const want of [].concat(expect.notCalled || [])) {
            const bad = calls.filter((call) => callMatches(want, call));
            if (bad.length) fail(`a call matches ${show(want)}, which should not be called: ${show(bad.map((call) => call.arguments))}`);
        }
        for (const want of [].concat(expect.results || [])) {
            const pattern = want instanceof RegExp || typeof want === "string" ? { result: want } : want;
            const candidates = calls.filter((call) => !pattern.tool || call.tool === pattern.tool);
            if (!candidates.some((call) => fits(pattern.result, call.result))) fail(`no result${pattern.tool ? ` of ${pattern.tool}` : ""} matches ${show(pattern.result)}`);
        }
        const held = view.held || [];
        if (expect.held !== undefined) {
            if (typeof expect.held === "number") {
                if (held.length !== expect.held) fail(`${held.length} write(s) held for approval, expected ${expect.held}: ${show(held.map((write) => write.transition))}`);
            } else if (!held.some((write) => fits(expect.held, write))) {
                fail(`no held write matches ${show(expect.held)}: ${show(held)}`);
            }
        }
        if (expect.noWrite) {
            const writes = calls.filter((call) => call.classification && call.classification !== "read");
            if (held.length || writes.length) fail(`a write was attempted: ${show(held.map((write) => write.transition).concat(writes.map((call) => call.tool)))}`);
        }
        if (expect.maxWrongCalls !== undefined) {
            const wrong = calls.filter((call) => call.error).length;
            if (wrong > expect.maxWrongCalls) fail(`${wrong} wrong call(s), at most ${expect.maxWrongCalls} expected`);
        }
        if (expect.maxToolCalls !== undefined && calls.length > expect.maxToolCalls) {
            fail(`${calls.length} tool call(s), at most ${expect.maxToolCalls} expected`);
        }
        if (expect.questions !== undefined) {
            const asked = view.questions || [];
            if (typeof expect.questions === "number" ? asked.length !== expect.questions : !asked.some((question) => fits(expect.questions, question))) {
                fail(`questions asked: ${show(asked)}, expected ${show(expect.questions)}`);
            }
        }
        if (expect.state !== undefined && !fits(expect.state, view.state)) fail(`the run ended ${show(view.state)}, expected ${show(expect.state)}`);
        if (expect.noInvented && (view.invented || []).length) fail(`the answer links what no tool returned: ${show(view.invented)}`);
        return failures;
    };

    // What the whole task did: every call, held write and question of its turns, the last turn's answer and screen.
    const whole = (observed) => {
        const turns = observed.turns || [];
        const last = turns[turns.length - 1] || {};
        const all = (key) => turns.reduce((list, turn) => list.concat(turn[key] || []), []);
        return Object.assign({}, last, { calls: all("calls"), held: all("held"), questions: all("questions"),
            invented: all("invented"), turns });
    };

    A.check = (index, observedJson) => {
        const task = A.spec.tasks[index];
        const observed = JSON.parse(observedJson);
        const failures = [];
        if (task.turns) {
            task.turns.forEach((turn, at) => {
                if (turn.expect) failures.push(...evaluate(`turn ${at + 1}: `, turn.expect, (observed.turns || [])[at] || {}));
            });
        }
        const view = whole(observed);
        if (task.expect) failures.push(...evaluate("", task.expect, view));
        if (typeof task.verify === "function") {
            try { task.verify(view); } catch (e) { failures.push(`verify: ${message(e)}`); }
        }
        return JSON.stringify({ failures });
    };
})(globalThis);
