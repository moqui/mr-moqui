/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The split and the wizard in the page. moqui is /js/MoquiLib.js and /js/WebrootVue.qvt.js.
// The library starts the app only when #moqui-config, Vue and Quasar are all present.
"use strict";

test("a split stacks below 1024 px and when side and main do not fit, and keeps the side width in its limits", () => {
    assert.deepEqual(moqui.splitLayout({ wide: true, width: 1200, sideWidth: 320, minMain: 480 }), { stacked: false, side: 320, min: 200, max: 720 });
    assert.equal(moqui.splitLayout({ wide: false, width: 1200, sideWidth: 320, minMain: 480 }).stacked, true);
    // a container narrower than 200 + 480, as beside an open Assist drawer
    assert.equal(moqui.splitLayout({ wide: true, width: 640, sideWidth: 320, minMain: 480 }).stacked, true);
    // a saved width, and one that would squeeze the main pane
    assert.equal(moqui.splitLayout({ wide: true, width: 1200, sideWidth: 320, minMain: 480, saved: 410 }).side, 410);
    assert.equal(moqui.splitLayout({ wide: true, width: 1200, sideWidth: 320, minMain: 480, saved: 900 }).side, 720);
    assert.equal(moqui.splitLayout({ wide: true, width: 1200, sideWidth: 320, minMain: 480, saved: 50 }).side, 200);
    // not measured yet: the attribute's width, the defaults for text and nonsense
    assert.equal(moqui.splitLayout({ wide: true, width: 0, sideWidth: "280", minMain: "x" }).side, 280);
    assert.equal(moqui.splitLayout({ wide: true, width: 0, sideWidth: 280 }).stacked, false);
    assert.deepEqual(moqui.splitLayout({ wide: true, width: 1000 }), { stacked: false, side: 320, min: 200, max: 520 });
});

test("a step's URL keeps the current parameters under its own and the answer's, without the session token", () => {
    assert.equal(moqui.wizardStepUrl("/apps/gallery/Wizard/Options?step=2", { importId: "I1", moquiSessionToken: "t", empty: "" }, { importId: "I2", rows: "5" }),
        "/apps/gallery/Wizard/Options?importId=I2&step=2&rows=5");
    assert.equal(moqui.wizardStepUrl("/apps/x/Wizard/Source", {}, null), "/apps/x/Wizard/Source");
    assert.equal(moqui.wizardStepUrl("/apps/x/Wizard/Source", { q: "a b" }, null), "/apps/x/Wizard/Source?q=a%20b");
});

test("after an answer the wizard stays on errors, goes where another screen is named, and else to the next step", () => {
    const path = ["gallery", "Wizard", "Customer"];
    assert.deepEqual(moqui.wizardAfterAnswer({ errors: ["Customer name is required"] }, path, 3, 0), { kind: "stay" });
    assert.deepEqual(moqui.wizardAfterAnswer({ validationErrors: [{ field: "x", message: "bad" }] }, path, 3, 0), { kind: "stay" });
    // the transition answered url=".": the next step, with the answer's parameters
    assert.deepEqual(moqui.wizardAfterAnswer({ screenPathList: ["apps", "gallery", "Wizard", "Customer"], screenUrl: "/apps/gallery/Wizard/Customer?draftId=D1",
        screenParameters: { draftId: "D1" } }, path, 3, 0), { kind: "step", index: 1, parameters: { draftId: "D1" } });
    // no screen named (a JSON answer of a service): the next step
    assert.deepEqual(moqui.wizardAfterAnswer({ messageInfos: [] }, path, 3, 1), { kind: "step", index: 2, parameters: {} });
    // the transition chose another screen: there
    assert.deepEqual(moqui.wizardAfterAnswer({ screenPathList: ["apps", "gallery", "List"], screenUrl: "/apps/gallery/List" }, path, 3, 0),
        { kind: "url", url: "/apps/gallery/List" });
    assert.deepEqual(moqui.wizardAfterAnswer({ redirectUrl: "https://example.com/" }, path, 3, 0), { kind: "redirect", url: "https://example.com/" });
    // the last step follows the answer, or stays
    assert.deepEqual(moqui.wizardAfterAnswer({ screenPathList: ["apps", "gallery", "Wizard", "Review"], screenUrl: "/apps/gallery/Wizard/Review?done=Y" },
        ["gallery", "Wizard", "Review"], 3, 2), { kind: "url", url: "/apps/gallery/Wizard/Review?done=Y" });
    assert.deepEqual(moqui.wizardAfterAnswer({}, ["gallery", "Wizard", "Review"], 3, 2), { kind: "stay" });
});

test("a split side by side fills the viewport below its top, so each pane scrolls in itself; stacked the page scrolls", () => {
    assert.equal(moqui.splitHeight({ stacked: false, top: 120, viewport: 900 }), "764px");
    // at least 360 px, the height attribute as given, nothing stacked or before the viewport is known
    assert.equal(moqui.splitHeight({ stacked: false, top: 700, viewport: 800 }), "360px");
    assert.equal(moqui.splitHeight({ stacked: false, height: "70vh", top: 120, viewport: 900 }), "70vh");
    assert.equal(moqui.splitHeight({ stacked: true, top: 120, viewport: 900 }), undefined);
    assert.equal(moqui.splitHeight({ stacked: false, top: 120, viewport: 0 }), undefined);
    assert.equal(moqui.splitHeight({ stacked: false, top: 100, viewport: 900, margin: 0 }), "800px");
});

test("the wizard covers the step while its form posts and waits for the screen the answer names", () => {
    const wizard = moqui.qvt.components["m-subscreens-wizard"];
    assert.ok(wizard.template.includes('<q-inner-loading :showing="submitting"'), "the step is covered while it posts");
    assert.ok(wizard.template.includes(':disable="submitting"'), "Back waits");
    const calls = [];
    const self = Object.assign({ submitting: true, steps: [{ name: "A", pathWithParams: "/apps/x/W/A" }, { name: "B", pathWithParams: "/apps/x/W/B" }], activeIndex: 1,
        $root: { currentPathList: ["x", "W", "B"], currentParameters: {}, getLinkPath: (url) => url, setUrl: (url, body, done) => calls.push([url, done]) } }, wizard.methods);
    // the last step answered the same step with done=Y: the spinner stays until that screen is there
    wizard.methods.stepAnswered.call(self, null, { screenPathList: ["apps", "x", "W", "B"], screenUrl: "/apps/x/W/B?done=Y" });
    assert.equal(self.submitting, true);
    self.headerTo("A");
    assert.equal(calls.length, 1, "previous headers wait for the active submission too");
    assert.equal(calls[0][0], "/apps/x/W/B?done=Y");
    calls[0][1]();
    assert.equal(self.submitting, false);
    // an answer with errors stays, and the spinner goes at once
    self.submitting = true;
    wizard.methods.stepAnswered.call(self, null, { errors: ["no"] });
    assert.equal(self.submitting, false);
    // a step form that posts by itself (Enter, its own button) shows the spinner too
    wizard.methods.stepSubmitting.call(self);
    assert.equal(self.submitting, true);
});


test("an opt-in wizard keeps only unsent non-secret fields for the same record and discards them after save or exit", () => {
    const wizard = moqui.qvt.components["m-subscreens-wizard"];
    const nav = [];
    const self = Object.assign(wizard.data(), wizard.methods, {
        draftForm: "AccountOptions", draftFields: "accountName,recordFilingEnumId,syncWindowDays", draftParameter: "emailAccountId",
        steps: [{ name: "Test", pathWithParams: "/W/Test" }, { name: "Options", pathWithParams: "/W/Options" }, { name: "Finish", pathWithParams: "/W/Finish" }], activeIndex: 1,
        formsChanged() {},
        $root: { currentParameters: { emailAccountId: "A1" }, currentPathList: ["W", "Options"], getLinkPath: (url) => url,
            setUrl: (url, body, done) => { nav.push(url); if (done) done(); } }
    });
    Object.defineProperty(self, "draftContext", { get: () => wizard.computed.draftContext.call(self) });
    const form = () => ({ fields: { moquiFormName: "AccountOptions", accountName: "Saved", recordFilingEnumId: "ErfAllMatches",
        syncWindowDays: "365", memberUserIds: [], mailPassword: "server-secret", emailAccountId: self.$root.currentParameters.emailAccountId },
        submitForm() { throw new Error("Back must not submit or change sharing"); } });
    const edited = form();
    self.addForm(edited);
    edited.fields.accountName = "UI review local mail";
    edited.fields.recordFilingEnumId = "ErfLinkedOnly";
    edited.fields.syncWindowDays = "90";
    edited.fields.memberUserIds = ["USER2"];
    self.back();
    self.removeForm(edited); // browser Back and a previous-step header also unmount the form
    assert.equal(nav.length, 1);
    assert.deepEqual(self.draftValues, { accountName: "UI review local mail", recordFilingEnumId: "ErfLinkedOnly", syncWindowDays: "90" });
    const revisited = form();
    self.addForm(revisited);
    assert.equal(revisited.fields.accountName, "UI review local mail");
    assert.equal(revisited.fields.recordFilingEnumId, "ErfLinkedOnly");
    assert.equal(revisited.fields.syncWindowDays, "90");
    assert.deepEqual(revisited.fields.memberUserIds, [], "pending remote members require Next; restoring IDs without labels can lose them on the next search");
    assert.equal(revisited.fields.mailPassword, "server-secret", "unlisted credentials are not restored");
    self.captureDraftSubmit(revisited)({ errors: ["Invalid choice"] });
    self.stepAnswered(revisited, { errors: ["Invalid choice"] });
    assert.ok(self.draftValues, "a refused save retains the draft");
    self.captureDraftSubmit(revisited)({});
    self.stepAnswered(revisited, {});
    self.removeForm(revisited);
    assert.equal(self.draftValues, null, "successful save clears it and unmount does not re-save stale options");
    const changedAccount = form();
    self.addForm(changedAccount);
    changedAccount.fields.accountName = "Account A1 draft";
    self.$root.currentParameters.emailAccountId = "A2";
    self.removeForm(changedAccount);
    assert.equal(self.draftValues, null, "a late old-account unmount cannot become the new account's draft");
    const nextAccount = form(); self.addForm(nextAccount);
    assert.equal(nextAccount.fields.accountName, "Saved");
    nextAccount.fields.accountName = "A2 draft"; self.removeForm(nextAccount);
    self.$root.currentParameters = {}; self.syncDraftScope();
    assert.equal(self.draftValues, null, "fresh setup with no account clears the scope");
    self.$root.currentParameters = { emailAccountId: "A2" };
    const last = form(); self.addForm(last); last.fields.accountName = "last draft"; self.removeForm(last);
    self.activeIndex = 2; self.$root.currentPathList = ["W", "Finish"];
    self.stepAnswered(null, {});
    assert.equal(self.draftValues, null, "Finish discards any earlier unsent draft");
    self.addForm(last); self.removeForm(last); wizard.beforeUnmount.call(self);
    assert.equal(self.draftValues, null, "leaving a wizard discards its in-memory draft");
    self.draftForm = null; self.addForm(last); self.removeForm(last);
    assert.equal(self.draftValues, null, "wizards without the opt-in keep their original behavior");
});


test("a wizard acknowledges only its submitted snapshot after a delayed response", async () => {
    const wizard = moqui.qvt.components["m-subscreens-wizard"];
    const formOptions = moqui.qvt.components["m-form"];
    const originalFetch = globalThis.fetch;
    const pending = [];
    const posted = [];
    const makeWizard = () => {
        const self = Object.assign(wizard.data(), wizard.methods, {
            draftForm: "AccountOptions", draftFields: "accountName,recordFilingEnumId", draftParameter: "emailAccountId",
            formsChanged() {}, isStepForm(form) { return this.forms[0] === form; },
            $root: { currentParameters: { emailAccountId: "A1" }, loading: 0, appRootPath: "", moquiSessionToken: "csrf", getCsrfToken() {} }
        });
        Object.defineProperty(self, "draftContext", { get: () => wizard.computed.draftContext.call(self) });
        return self;
    };
    const makeForm = (self) => {
        const fieldsInitial = {moquiFormName:"AccountOptions", accountName:"Saved", recordFilingEnumId:"ErfLinkedOnly", mailPassword:"secret"};
        const form = Object.assign(formOptions.data.call({fieldsInitial}), {
            mWizard:self, method:"POST", action:"/save-options", $root:self.$root, $refs:{qForm:{$el:document.createElement("form")}},
            handleResponse() {}, $emit() {}
        });
        self.addForm(form);
        return form;
    };
    const answer = async (json, form) => {
        const handleResponse = form.handleResponse;
        const handled = new Promise((resolve, reject) => {
            form.handleResponse = function (response) {
                try { handleResponse.call(this, response); resolve(); }
                catch (error) { reject(error); throw error; }
            };
        });
        // Resolving fetch does not settle Response.text(). Wait for the submitted form's real completion callback.
        pending.shift()(new Response(JSON.stringify(json), {headers:{"Content-Type":"application/json"}}));
        try { await handled; }
        finally { form.handleResponse = handleResponse; }
    };
    try {
        globalThis.fetch = (_url, init) => { posted.push(init.body); return new Promise(resolve => pending.push(resolve)); };
        const saved = makeWizard(); const first = makeForm(saved); first.fields.accountName="Submitted";
        formOptions.methods.submitGo.call(first); saved.removeForm(first);
        assert.deepEqual(saved.draftValues, {accountName:"Submitted",recordFilingEnumId:"ErfLinkedOnly"}, "only allowlisted values enter the snapshot");
        assert.equal(posted[0].get("accountName"), "Submitted", "the acknowledgment snapshots what the form serialized");
        const restored = makeForm(saved);
        await answer({}, first);
        saved.removeForm(restored);
        assert.equal(saved.draftValues, null, "an accepted snapshot cannot be recaptured by its unchanged remounted form");

        const refused = makeWizard(); const retry = makeForm(refused); retry.fields.accountName="Retry me";
        formOptions.methods.submitGo.call(retry); refused.removeForm(retry); await answer({errors:["Fix the options"]}, retry);
        assert.equal(refused.draftValues.accountName, "Retry me", "a refusal keeps the pending values");

        const edited = makeWizard(); const older = makeForm(edited); older.fields.accountName="Older";
        formOptions.methods.submitGo.call(older); edited.removeForm(older);
        const newer = makeForm(edited); newer.fields.accountName="Newer edit"; edited.removeForm(newer); await answer({}, older);
        assert.equal(edited.draftValues.accountName, "Newer edit", "an older acceptance cannot clear a newer local snapshot");

        const switched = makeWizard(); const oldAccount = makeForm(switched); oldAccount.fields.accountName="Account A1";
        formOptions.methods.submitGo.call(oldAccount); switched.removeForm(oldAccount);
        switched.$root.currentParameters.emailAccountId="A2";
        const nextAccount = makeForm(switched); nextAccount.fields.accountName="Account A2"; switched.removeForm(nextAccount); await answer({}, oldAccount);
        assert.equal(switched.draftValues.accountName, "Account A2", "an old account's acceptance cannot clear the new scope");

        const left = makeWizard(); const leaving = makeForm(left); leaving.fields.accountName="Leaving";
        formOptions.methods.submitGo.call(leaving); wizard.beforeUnmount.call(left); left.removeForm(leaving); await answer({}, leaving);
        assert.equal(left.draftValues, null, "teardown invalidates the acknowledgment and late form removal");
        assert.equal(makeWizard().draftValues, null, "a fresh wizard starts without the old snapshot");

        // The real completion path must not undo snapshot protection: a focused field can change while its POST is pending.
        const live = makeWizard(); const liveForm = makeForm(live); const navigation = [];
        live.activeIndex=1;
        live.steps=[{name:"Test",pathWithParams:"/W/Test"},{name:"Options",pathWithParams:"/W/Options"},{name:"Finish",pathWithParams:"/W/Finish"}];
        live.$root.currentPathList=["W","Options"];
        live.$root.getLinkPath=(url)=>url;
        live.$root.setUrl=(url, _body, done)=>{ navigation.push(url); live.removeForm(liveForm); if(done)done(); };
        liveForm.handleResponse=formOptions.methods.handleResponse;
        liveForm.fields.accountName="Sent value";
        formOptions.methods.submitGo.call(liveForm);
        liveForm.fields.accountName="Edited while saving";
        await answer({screenPathList:["W","Options"],screenUrl:"/W/Options?emailAccountId=A1",screenParameters:{emailAccountId:"A1"}}, liveForm);
        assert.deepEqual(navigation, ["/W/Finish?emailAccountId=A1"], "real handleResponse and stepAnswered retain the normal next-step route");
        assert.equal(live.draftValues.accountName, "Edited while saving", "step completion and unmount preserve values newer than the submitted snapshot");
        assert.equal(live.submitting, false);
        const confirmation={fields:{moquiFormName:"AccountTestConfirm",emailAccountId:"A1"}};
        live.addForm(confirmation);
        assert.equal(live.captureDraftSubmit(confirmation), null, "another step's form does not capture or acknowledge the Options draft");
        live.removeForm(confirmation);
        assert.equal(live.draftValues.accountName, "Edited while saving");
        live.activeIndex=2; live.$root.currentPathList=["W","Finish"];
        live.stepAnswered(null, {});
        assert.equal(live.draftValues, null, "explicit Finish still discards an earlier unsent draft");
    } finally { globalThis.fetch = originalFetch; }
});
