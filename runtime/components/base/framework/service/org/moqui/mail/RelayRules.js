// What every path that changes a mail relay holds to (Admin's Connections, ConfigurationConnections.js; the tools REST
// PATCH email/servers, EmailServices.update#RelayServer), and the sign-in mail Admin's mail outcomes never send again
// (OperationMail.js): one copy of each rule.
//
// - The sign-in mail (a reset password, a sign-in code, a factor added, the new-device notice, PASS's verification link)
//   signs an account in or takes it over, so the relay it goes through (SYSTEM, the framework's own, or one such a
//   template names) is changed and tested only by a holder of the sealed ADMIN_ACCESS_MANAGE after a recent sign-in
//   (plans/Security-Review-Medium.md SEC-ADM-82).
// - A relay's link base URL (the start of every link in the mail it sends) is on one of the instance's own names
//   (ConfigurationServices.get#OwnHostNames; SEC-ADM-83).
const text = value => String(value ?? "").trim()

const SIGN_IN_TEMPLATES = [
    "PASSWORD_RESET",
    "SINGLE_USE_CODE",
    "ADDED_EMAIL_AUTHC_FACTOR",
    "EMAIL_AUTHC_FACTOR_SENT",
    "ACCOUNT_SIGN_IN"
]
// with the conf's PASS verification template, when there is one
const signInTemplates = () =>
    SIGN_IN_TEMPLATES.concat([text(ec.factory.getConfValue("pass-authority@verification-template"))].filter(Boolean))

// Whether the sign-in mail goes through the relay emailServerId.
const signInRelay = emailServerId =>
    text(emailServerId) === "SYSTEM" ||
    signInTemplates().some(
        emailTemplateId =>
            Number(
                ec.entity
                    .find("moqui.basic.email.EmailTemplate")
                    .condition({emailTemplateId, emailServerId: text(emailServerId)})
                    .disableAuthz()
                    .count()
            ) > 0
    )

// The host of a URL as get#OwnHostNames gives names: lower case, no port or trailing dot, an IPv6 address in brackets.
function hostOf(url) {
    const parts = /^[a-z][a-z0-9+.-]*:\/\/(?:[^\/?#@]*@)?(\[[^\]]+\]|[^\/?#:]+)/i.exec(text(url))
    return parts ? parts[1].toLowerCase().replace(/\.$/, "") : ""
}
const ownHosts = () =>
    Array.from(ec.service.sync().name("org.moqui.admin.ConfigurationServices.get#OwnHostNames").disableAuthz().call()?.hostNameList || [])
        .map(String)

// Why `url` may not be a relay's link base URL, or null. Empty is the instance's own root URL.
function linkBaseRefusal(url) {
    if (!text(url)) return null
    const own = ownHosts()
    if (own.includes(hostOf(url))) return null
    return (
        "The link base URL is on one of this instance's own names (" +
        (own.join(", ") || "none is configured") +
        "). To send links to another name of this instance, add it to the webroot webapp's allowed-hosts, or give its " +
        "public name as http-host (moqui_conf__webapp_list__webapp__WEBROOT__allowed_hosts, " +
        "moqui_conf__webapp_list__webapp__WEBROOT__http_host)"
    )
}

// The refusal of a change of the sign-in mail's relay for the signed-in user, or null: ADMIN_ACCESS_MANAGE and a sign-in in
// the last five minutes (require#RecentAuth adds its error and the X-Moqui-Reauth header). A caller with authorization
// disabled answers for itself.
function signInRelayRefusal(emailServerId) {
    if (ec.artifactExecution.authzDisabled() || !signInRelay(emailServerId)) return null
    if (!ec.user.hasPermission("ADMIN_ACCESS_MANAGE"))
        return {
            reasonCode: "READ_ONLY",
            reason: ec.l10n.localize("This relay sends the sign-in mail (password resets, sign-in codes): an access administrator changes it")
        }
    const recent = ec.service.sync().name("org.moqui.sso.RecentAuthServices.require#RecentAuth").call()?.recent
    return recent ? null : {reasonCode: "reauthRequired", reason: ec.l10n.localize("Confirm it is you: this relay carries the sign-in mail")}
}

;({SIGN_IN_TEMPLATES, signInTemplates, signInRelay, hostOf, ownHosts, linkBaseRefusal, signInRelayRefusal})
