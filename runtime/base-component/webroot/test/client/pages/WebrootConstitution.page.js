// The Constitution in the page (js/WebrootConstitution.js): the components it registers, a reference resolved by the server
// at each open and never before, a refusal shown in its place, focus back on the link, and a chat link recognized only as
// one of its run's trusted references. The dialog's drawing, focus trap and framing are browser checks (acceptance).
const constitution = moqui.constitution;
const calls = [];
let answers = [];
moqui.appRootPath = "";
moqui.request = (url, options) => {
    calls.push({ url, params: options.params });
    const next = answers.shift();
    return next instanceof Error ? Promise.reject(next) : Promise.resolve({ json: next || {} });
};

test("the reading view, the reference dialog and the drawer's Rules pane are registered", () => {
    for (const name of ["m-constitution-view", "m-reference-dialog", "m-assist-constitution"]) {
        assert.ok(moqui.qvtComponents[name], name);
        assert.equal(typeof moqui.qvtComponents[name].render, "function", name + " draws with a render function");
    }
});

test("a reference is resolved by the server when it is opened, not before, and a refusal takes its place", async () => {
    calls.length = 0;
    const dialog = constitution.dialogState();
    assert.equal(calls.length, 0);
    answers = [{ allowed: "Y", url: "https://grok-the-rock.org/rules#r1", title: "Rule one", origin: "https://grok-the-rock.org" }];
    await constitution.openReference(dialog, { referenceId: "C01-R01", runId: "R7", title: "Shown" });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "/assist/constitutionReference");
    assert.deepEqual(calls[0].params, { referenceId: "C01-R01", runId: "R7", title: "Shown" });
    assert.equal(dialog.open, true);
    assert.equal(dialog.reference.url, "https://grok-the-rock.org/rules#r1");
    assert.equal(dialog.title, "Rule one");
    answers = [{ allowed: "N", reasonCode: "originNotAllowed", message: "https://elsewhere.example is not trusted for reading in the app." }];
    await constitution.openReference(dialog, { referenceId: "C02-R01" });
    assert.equal(dialog.reference, null);
    assert.equal(dialog.refused, "https://elsewhere.example is not trusted for reading in the app.");
    answers = [new Error("offline")];
    await constitution.openReference(dialog, { referenceId: "C02-R01" });
    assert.ok(dialog.refused && dialog.refused.indexOf("could not be checked") >= 0);
});

test("closing unloads the reference and focus goes back to the link that opened it", async () => {
    const dialog = constitution.dialogState();
    const link = document.createElement("a");
    link.href = "#";
    link.textContent = "Rule one";
    document.body.appendChild(link);
    answers = [{ allowed: "Y", url: "/apps/wiki/rules", title: "Internal" }];
    await constitution.openReference(dialog, { referenceId: "C03-R01" }, link);
    assert.equal(constitution.href(dialog.reference.url), "/apps/wiki/rules");
    constitution.closeReference(dialog);
    assert.equal(dialog.open, false);
    assert.equal(dialog.reference, null);
    constitution.restoreFocus(dialog);
    assert.equal(document.activeElement, link);
    link.remove();
});

test("a chat link opens the dialog only when it is exactly one of its run's trusted references", async () => {
    answers = [{ runId: "R9", ruleList: [{ ruleId: "C01", referenceList: [
        { referenceId: "C01-R01", title: "Trusted", url: "https://grok-the-rock.org/a#b", trusted: "Y" },
        { referenceId: "C01-R02", title: "Not trusted", url: "https://elsewhere.example/c", trusted: "N" },
    ] }] }];
    assert.equal(constitution.knownReference("R9", "https://grok-the-rock.org/a#b"), null, "not loaded yet");
    await constitution.runReferences("R9");
    assert.equal(constitution.knownReference("R9", "https://grok-the-rock.org/a#b").referenceId, "C01-R01");
    assert.equal(constitution.knownReference("R9", "https://grok-the-rock.org/a"), null, "the exact URL, fragment and all");
    assert.equal(constitution.knownReference("R9", "https://elsewhere.example/c"), null, "an untrusted origin is an ordinary link");
    assert.equal(constitution.knownReference("R8", "https://grok-the-rock.org/a#b"), null, "another run's reference");
    const before = calls.length;
    await constitution.runReferences("R9");
    assert.equal(calls.length, before, "a run's references are read once");
});
