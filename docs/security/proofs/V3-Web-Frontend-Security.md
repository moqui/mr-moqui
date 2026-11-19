# V3 Web Frontend Security

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V3 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V3.2 Unintended Content Interpretation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-3.2.1 | 1 | API answers and user files never shown in a page context | Framework | `web/mod.rs` `user_download_headers`, `file_headers`, `send_resource_response`: inert type else `application/octet-stream`, `nosniff`, `default-src 'none'; sandbox`, attachment; DAV the same (SEC-FIL-03 fixed); rich-text images raster only (`service/images.rs`); `/git` file answers not read | Rust `web::tests::a_users_download_has_an_inert_type_and_the_sandbox`; mr-pass Smithy "raw serves a file as plain text or a download, never as a page ...", "a release's file is answered as an inert type with a sandbox ..." |
| v5.0.0-3.2.2 | 1 | Text put in the page as text, never as markup | Application | tools: Vue `{{ }}` and `v-text`, `.html.fancy` escaping, `m-markdown` and `m-html-view` with DOMPurify (`WebrootVue.qvt.js` `sanitize`); the framework's own `v-html` sites sanitize (sampled, not all read) | to write: black-box |

## V3.3 Cookie Setup

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-3.3.1 | 1 | Cookies are Secure and named `__Host-` or `__Secure-` | Gap | session cookie `__Host-moqui.session` with `Secure` over https only (`user/session.rs` `format_session_cookie`, `cookie_flags`); `moqui.visitor` (`format_visitor_cookie`) and `moqui.device` (`user/device.rs:231`) have no prefix; plain http (the default) sets no `Secure` | - |
| v5.0.0-3.3.2 | 2 | SameSite chosen for each cookie's purpose | Framework | session and visitor `SameSite=Lax` (`session.rs` `cookie_flags`) beside the cross-site GET rule and the Origin check; device `SameSite=Strict` (`device.rs:231`) | SecurityHeaders "A02 the session cookie is HttpOnly and SameSite=Lax"; SecurityDevice "a registration code makes one browser the device, once, with an HttpOnly SameSite=Strict cookie" |
| v5.0.0-3.3.3 | 2 | Cookies named `__Host-` unless meant for other hosts | Gap | only the session cookie, only over https (`SECURE_SESSION_COOKIE_NAME`); `moqui.visitor`, `moqui.device` and every cookie over plain http have none | - |
| v5.0.0-3.3.4 | 2 | Session cookie HttpOnly, its value sent only by Set-Cookie | Framework | `HttpOnly` on session and visitor (`cookie_flags`) and device; the CSRF token is a separate value; session id not written into pages (sampled) | SecurityHeaders "A02 the session cookie is HttpOnly and SameSite=Lax" |

## V3.4 Browser Security Mechanism Headers

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-3.4.1 | 1 | HSTS on every answer, a year, subdomains | Gap | `max-age=31536000` (conf `screen-secure`) only on https screen-chain answers to a host not under `localhost` (`web/mod.rs` `finish_response`); host routes and host refusals none (only the chain calls `finish_response`); no `includeSubDomains` (conf could add it); https itself is the deployer's | - |
| v5.0.0-3.4.2 | 1 | CORS answers only listed origins; wildcard never credentialed | Framework | `web/mod.rs` `refused_origin` (401 before the servlet), `apply_cors` (echoes an allowed origin only; `*` gets no credentials, SEC-WEB-10 fixed); conf `webapp-list/webapp[webroot]@allow-origins` empty | SecurityCors "A02 an unknown origin is 401 with no Access-Control-Allow-Origin ...", "A02 the server's own origin gets Access-Control-Allow-Origin with credentials" |
| v5.0.0-3.4.3 | 2 | Global CSP: object-src and base-uri none, source allowlist | Gap | default `screen-render` CSP is `frame-ancestors 'none'; form-action 'self'` only (`MoquiDefaultConf.xml`): no object-src, base-uri or script allowlist; the apps drop it whole (SEC-CLI-04, deferred) | - |
| v5.0.0-3.4.4 | 2 | nosniff on every answer | Gap | conf `screen-render` `X-Content-Type-Options` on screen-chain answers; host routes and host refusals none (only the chain calls `finish_response`); user files set their own | - |
| v5.0.0-3.4.5 | 2 | A referrer policy keeps URLs from other sites | Configuration | none shipped; a `response-header` of type `screen-render` named `Referrer-Policy` under `webapp-list/webapp[webroot]` sets it; outside links in markdown get `rel="noopener noreferrer"` | - |
| v5.0.0-3.4.6 | 2 | CSP frame-ancestors on every answer | Gap | `frame-ancestors 'none'` (conf `screen-render`) on screen-chain answers; host routes none; a user file's own policy (`USER_FILE_SANDBOX`) replaces it with none | - |

## V3.5 Browser Origin Separation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-3.5.1 | 1 | Cross-origin calls to sensitive functions need forgery proof | Framework | `screen/render.rs` `csrf_rejected` (token on POST, PUT, PATCH, DELETE, constant time; `@require-session-token` true); `screen/mod.rs` `serve_at_to` answers 401 to a foreign `Origin` on any method; header logins skip only the token | SecurityAuthn "A07 a POST runs with the session token and stores nothing without it ..."; SecurityRest "A01 an entity REST POST without the session token does not create"; PASS: to write: black-box |
| v5.0.0-3.5.2 | 1 | Preflight-based defence not passed by simple requests | Framework | relies on no preflight: the token check and `refused_origin` (`screen/mod.rs` `serve_at_to`) apply whatever the content type | SecurityCors "A02 an unknown origin is 401 ... (#method)" (GET, OPTIONS); a foreign-Origin form POST: to write: black-box |
| v5.0.0-3.5.3 | 1 | No sensitive actions on GET, HEAD or OPTIONS | Gap | cross-site GET of an acting transition 403 (`render.rs` `cross_site_write`, `web::started_by_another_site`), GET with URL parameters 400 (`insecure_transition`); a same-origin GET with none still runs it, as an `<img>` in rich text does (SEC-CLI-82); only `Sec-Fetch-Site` is read | - |
| v5.0.0-3.5.4 | 2 | Separate applications on separate host names | Configuration | a `root-screen` per `host` with `framework-routes="refuse"`, a `public-site` `host` (map); by default every app shares one origin; SEC-RTE-11 (restricted host serves some framework paths); SEC-CLI-81 for the apps | - |
| v5.0.0-3.5.5 | 2 | postMessage from untrusted origins or malformed dropped | Framework | `js/MoquiLib.js` email OAuth `onMessage` (origin, source, `type`, `signInId`); `js/WebrootAssistSandbox.js` `onPort` (parent source, nonce); Assist canvas over a `MessageChannel` port | to write: black-box |

## V3.7 Other Browser Security Considerations

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-3.7.1 | 2 | Only supported, current client-side technologies | Framework | Vue 3, Quasar and the rest of `webroot/libs` (`MANIFEST.json`, `libs/refresh.mjs`); no plugin content | to write: black-box |
| v5.0.0-3.7.2 | 2 | Off-site automatic redirects only to allowlisted hosts | Framework | `web/mod.rs` `is_same_origin_redirect` (Login and `/sso` `returnTo`); `render.rs` `http_redirect_location` cuts an absolute redirect to its path while `@http-host` is empty (with it set an app's own absolute redirect goes as written: builder's); SSO targets from `AuthFlow` rows, `@app-redirect-uris` | SecurityAccess "A01 the login does not follow a returnTo off the site"; SecurityHeaders "A01 the login does not follow a returnTo from a spoofed Host"; PASS: to write: black-box |
