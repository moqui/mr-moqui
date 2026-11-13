/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The recent sign-in check of sensitive actions, in the browser: moqui.reauth (org.moqui.sso.RecentAuthServices over the
 * /ReAuth transitions; docs/security/Recent-Auth.md). An action the server guards with check#RecentAuth is refused with
 * reasonCode reauthRequired when the user did not sign in in the last five minutes; this asks the user to confirm who they
 * are and lets the caller try again.
 *
 * - check(): Promise<{recent, method, secondFactor, authFlowId, authFlowList}>.
 * - confirm(): asks for the password, then the code of the second factor when the account has one (offering to send it to
 *   an email or phone factor), in Quasar dialogs. Resolves when the server took them; rejects {reasonCode: 'cancelled'}
 *   when the user cancels. An account without a password goes to a fresh sign-in at its identity provider
 *   (/sso/login with reauth=true), which comes back to this page: the promise does not resolve, and the user does the
 *   action again there.
 * - ensure(): check(), then confirm() when the sign-in is not recent.
 * - retry(fn): fn(); when it is refused with reauthRequired, confirm() and fn() once more.
 */
(function (root) {
    "use strict";
    const moqui = (root.moqui = root.moqui || {});
    const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
    const BASE = "/ReAuth/";
    const TRIES = 5;

    function request(name, body) {
        const headers = { Accept: "application/json" };
        const init = { method: body ? "POST" : "GET", headers, credentials: "same-origin" };
        if (body) {
            headers["Content-Type"] = "application/json";
            if (moqui.sessionToken) headers["X-CSRF-Token"] = moqui.sessionToken;
            init.body = JSON.stringify(body);
        }
        return root.fetch(BASE + name, init).then((response) => {
            const token = response.headers.get("X-CSRF-Token");
            if (token && moqui.onSessionToken) moqui.onSessionToken(token);
            return response.json().catch(() => ({})).then((json) => {
                if (!response.ok) throw { reasonCode: response.status === 401 ? "notSignedIn" : "failed", errors: (json && json.errors) || String(response.status) };
                return json || {};
            });
        });
    }

    function dialog(options) {
        const vm = moqui.webrootVue;
        const open = vm && vm.$q && vm.$q.dialog ? (o) => vm.$q.dialog(o) : root.Quasar && root.Quasar.Dialog ? (o) => root.Quasar.Dialog.create(o) : null;
        if (!open) return Promise.reject({ reasonCode: "noDialog", errors: t("Sign in again to do this") });
        return new Promise((resolve, reject) => {
            open(Object.assign({ persistent: true, cancel: { label: t("Cancel"), flat: true, noCaps: true } }, options))
                .onOk((value) => resolve(value)).onCancel(() => reject({ reasonCode: "cancelled" }));
        });
    }

    const check = () => request("check");

    /** The SSO sign-in again: this page is left for the provider, and comes back here. */
    function viaSso(info) {
        const flows = info.authFlowList || [];
        const flow = flows.find((f) => f.authFlowId === info.authFlowId) || flows[0];
        if (!flow) return Promise.reject({ reasonCode: "noSso", errors: t("Sign out and sign in again to do this") });
        return dialog({ title: t("Confirm it is you"), message: t("Sign in again with {provider} to do this. You come back to this page; then do it again.").replace("{provider}", flow.description || flow.authFlowId),
            ok: { label: t("Sign in again"), color: "primary", unelevated: true, noCaps: true } }).then(() => {
            const back = root.location.pathname + root.location.search;
            root.location.assign("/sso/login?authFlowId=" + encodeURIComponent(flow.authFlowId) + "&reauth=true&returnTo=" + encodeURIComponent(back));
            return new Promise(() => null);
        });
    }

    /** The code of the second factor: one to send first when the account has an email or phone factor, then the code. */
    async function withCode(answer) {
        let text = answer.codeText || t("Enter the authentication code");
        const sendable = answer.sendableFactors || [];
        if (sendable.length) {
            const items = [{ label: t("I have a code"), value: "" }].concat(sendable.map((f) => ({ label: t("Send a code to") + " " + f.label, value: f.factorId })));
            const factorId = await dialog({ title: t("Confirm it is you"), message: text, options: { type: "radio", model: "", items },
                ok: { label: t("Next"), color: "primary", unelevated: true, noCaps: true } });
            if (factorId) {
                const sent = await request("sendCode", { factorId });
                if (sent.errors) text = sent.errors;
            }
        }
        for (let i = 0; i < TRIES; i++) {
            const code = await dialog({ title: t("Confirm it is you"), message: text, prompt: { model: "", type: "text", isValid: (v) => !!String(v || "").trim() },
                ok: { label: t("Confirm"), color: "primary", unelevated: true, noCaps: true } });
            const out = await request("password", { code: String(code).trim() });
            if (out.reauthenticated) return true;
            if (out.errors && !out.codeNeeded) throw { reasonCode: out.reasonCode || "notConfirmed", errors: out.errors };
            text = out.codeText || t("Authentication code is not valid");
        }
        throw { reasonCode: "notConfirmed", errors: t("Authentication code is not valid") };
    }

    /** Asks the user to confirm who they are (see above); resolves when the server took it. */
    async function confirm(known) {
        const info = known || await check();
        if (info.method === "sso") return viaSso(info);
        let message = t("Enter your password to do this.");
        for (let i = 0; i < TRIES; i++) {
            const password = await dialog({ title: t("Confirm it is you"), message, prompt: { model: "", type: "password", isValid: (v) => !!v },
                ok: { label: t("Confirm"), color: "primary", unelevated: true, noCaps: true } });
            const out = await request("password", { password });
            if (out.reauthenticated) return true;
            if (out.codeNeeded) return withCode(out);
            message = out.errors || t("The password is not valid");
        }
        throw { reasonCode: "notConfirmed", errors: t("The password is not valid") };
    }

    async function ensure() {
        const info = await check();
        if (info.recent) return true;
        return confirm(info);
    }

    function retry(fn) {
        return Promise.resolve().then(fn).catch((err) => {
            if (!err || err.reasonCode !== "reauthRequired") throw err;
            return confirm().then(() => fn());
        });
    }

    moqui.reauth = { check, confirm, ensure, retry };
})(window);
