# The framework's client specs

HTTP and WebSocket proofs of the framework's security, run from outside the server by `moqui test-client`
([plans/archive/ClientTestHarness.md](../../../../../plans/archive/ClientTestHarness.md) 7). They port Moqui Java's pytest suite
(`framework/test/*.py` in the Java checkout, with `SECURITY_TESTS.md` and `SecurityTestSupport.groovy`); the catalog
below lists each of its tests.

```sh
moqui test-client framework --serve                  # starts the server suite.js declares, runs, stops it
./moq run                                            # or a server of your own, in one terminal,
./moq test-client framework                          # and the specs against http://localhost:8080
moqui test-client framework --spec SecurityAuthn,SecurityCors --server https://test.example.com --insecure
```

`--serve` starts `moqui test framework --spec ClientFixtures --db snapshot --serve` under
`conf/MoquiLoadTestConf.xml` (`suite.js`): the production conf with the screen, transition and service tarpits off, so
that `LoadBasic` is not answered 429. Against another server:

- **The conf.** `SecurityCors` needs a server that allows no foreign origin: the production or load test conf. On the dev conf
  (`webapp_allow_origins=*`) its `setupSpec` fails with "this server allows every origin (the dev conf)", as the Python
  suite's `require_production_conf` fails it, and the foreign-origin proof of `SecurityWebSocket` skips. The other
  specs pass on any conf.
- **The fixture users.** `../ClientFixtures.spec.js` makes the `sec.*` accounts the proofs log in as, with
  Java's passwords, as `SecurityTestSupport.ensureUsers` did. They are not demo data. `sec.ip.v4`, `sec.ip.loop`
  and `sec.mfa.totp` are rows only: a password login from here is not how those proofs start. For a server
  `--serve` did not start, run it on that server's database:
  `moqui test framework --spec ClientFixtures --db existing`. Without them the features that need them skip,
  naming the command.
- **Users.** The other proofs log in as `john.doe`/`moqui` (`--param user=`, `--param password=`). None gives it a
  wrong password, since an account locks on its fourth failed login, and none logs it out over REST, which
  would end its other sessions (`UserAccount.hasLoggedOut`).

Each feature's name starts with its OWASP id, and a comment above it names the Python tests it ports, so
`grep -n test_<name> *.spec.js` finds the proof.

## Differences from Java

Recorded in [Parity-Diffs](../../../../../plans/Parity-Diffs.md) (P9-06 and the client test harness rows):

- A preflight answers 204, not 200.
- The session cookie is `moqui.session`, not `JSESSIONID`.
- The session-token failure is 401 "Session token is missing or is not valid" (`crates/moqui-core/src/screen/render.rs`),
  where Java says "Session token required" or "Session token does not match".
- An anonymous `/notws` upgrade is 401; Java accepts it and ignores the null user. So the foreign-origin proof uses a
  logged-in session, with the same session opening it from the server's own origin as the control.
- The origin is compared whole, scheme, host and port; Java compares the host only. So the server's host on another
  port or scheme is refused: 401 for CORS (SecurityCors > A02 the server's host on #which is 401 ...) and 403 for
  `/notws` (SecurityWebSocket > A01 a /notws upgrade from the server's host on #which is refused ...). These have no
  Python test.

Not a difference, but a choice of the suite: an account locks on its fourth failed login (`max-failures="3"`), as in
Java, and the suite must not lock the demo user, so no proof gives `john.doe` a wrong password. The wrong-password
and lockout proofs use `sec.lock.test`.

## Catalog

143 tests: 125 ported, 0 pending, 18 not applicable. Each test's status is `ported: <Spec> > <feature>`, `pending`
(with what it needs when that is more than a port), or `not applicable` with the reason, so grep counts them:
`grep -c '^| .test_.* | pending' README.md`. Multipart, HMAC (`/rest/sm`), the further `sec.*` users, and the tarpit
proofs are ported. `SecurityTarpit` skips while `tarpit-enabled` is false, which is this suite's load test conf; it
passes against `runtime/conf/MoquiProductionConf.xml` with `ClientFixtures` loaded. The two Swagger schema rows stay
not applicable: `/rest/entity.swagger` answers 404 before auth, and the schema gate itself is the `/rest/entity.json`
proof. The Selenium smokes beside the suite (`openui_*`, `llm_admin_*`, `vue_sfc_*`) and the A2A interop script
are Java Assist checks and out of scope.

### test_a01_access.py

| Python test | Here |
| --- | --- |
| `test_tools_without_session_is_not_the_tool` | ported: SecurityAccess > A01 Tools without a session redirects to the login, not the tool |
| `test_rest_e1_without_credentials_is_401` | ported: SecurityAccess > A01 #path without credentials is 401 |
| `test_rest_s1_without_credentials_is_unauthorized` | ported: SecurityAccess > A01 #path without credentials is 401 |
| `test_removed_rest_api_key_is_not_found` | ported: SecurityAccess > A01 the removed endpoint #method #path is 404, not merely unauthenticated |
| `test_removed_rest_moqui_session_token_is_not_found` | ported: SecurityAccess > A01 the removed endpoint #method #path is 404, not merely unauthenticated |

### test_a01_more.py

| Python test | Here |
| --- | --- |
| `test_echopath_extra_path_is_html_escaped` | ported: SecurityScreens > A01 an extra path on echopath is HTML-escaped
| `test_fop_without_login_is_not_a_pdf` | ported: SecurityScreens > A01 FOP of a screen without a login is not a PDF
| `test_email_tracking_unknown_id_returns_png` | ported: SecurityScreens > A01 an unknown email tracking id returns the 1x1 PNG |
| `test_status_from_loopback_is_json` | ported: SecurityStatus > A01 /status from loopback is JSON
| `test_status_x_forwarded_for_does_not_grant_access_from_spoofed_loopback` | ported: SecurityStatus > A01 X-Forwarded-For does not grant a spoofed loopback on /status
| `test_status_forwarded_for_header_does_not_hide_loopback` | ported: SecurityStatus > A01 Forwarded does not hide /status from loopback
| `test_login_form_action_is_login_path` | ported: SecurityScreens > A01 the login form posts to the login path
| `test_static_path_dotdot_does_not_escape` | ported: SecurityAccess > A01 a static path with .. does not serve screen XML |
| `test_view_only_cannot_run_service_run_over_http` | ported: SecurityScreens > A01 a VIEW-only user cannot run a service over HTTP
| `test_none_user_cannot_open_tools` | ported: SecurityAccess > A01 a user with no authorization cannot open Tools |
| `test_view_only_cannot_open_sql_runner` | ported: SecurityAccess > A01 a VIEW-only user cannot open the SQL Runner |
| `test_elastic_proxy_without_login_is_401` | ported: SecurityProxy > A01 /search without a login is 401
| `test_elastic_proxy_logged_in_without_permission_is_denied` | ported: SecurityProxy > A01 a logged-in user without SearchRemote is denied /search
| `test_kibana_proxy_without_login_is_401` | ported: SecurityProxy > A01 /dashboard without a login is 401
| `test_unauthenticated_data_import_does_not_fetch_location` | ported: SecurityScreens > A01 an anonymous data import does not fetch a location
| `test_apps_get_preferences_without_login_is_401` | ported: SecurityAccess > A01 getPreferences without a login is 401 with no preferences |
| `test_apps_qzsign_without_login_is_401` | ported: SecurityScreens > A01 qzSign without a login is 401 and not a signature
| `test_htmlr_anonymous_error_does_not_include_stack` | ported: SecurityScreens > A01 no HTML renderer is mounted at #path
| `test_htmlr_tools_without_login_is_not_a_pdf` | ported: SecurityScreens > A01 no HTML renderer is mounted at #path
| `test_system_message_unknown_remote_is_rejected` | ported: SecurityMessages > A01 an unknown /rest/sm remote is rejected
| `test_login_get_with_password_is_rejected` | ported: SecurityScreens > A01 a GET of the login with a password does not log in
| `test_web_inf_is_not_served` | ported: SecurityScreens > A01 WEB-INF is not served
| `test_groovysh_http_get_is_not_a_shell` | not applicable: /groovysh is excluded |
| `test_notws_http_get_is_not_a_websocket_handshake` | ported: SecurityScreens > A01 a GET of /notws is not a WebSocket handshake
| `test_sql_runner_query_string_sql_is_not_executed` | ported: SecurityScreens > A01 the SQL Runner does not run a statement from the query string
| `test_menu_data_tools_without_login_is_not_the_menu` | ported: SecurityScreens > A01 menu data for Tools without a login is not the menu
| `test_elastic_proxy_basic_auth_without_permission_is_403` | ported: SecurityProxy > A01 Basic auth without SearchRemote is 403 on /search
| `test_elastic_proxy_bad_basic_auth_is_401` | ported: SecurityProxy > A01 bad Basic auth on /search is 401
| `test_elastic_proxy_api_key_header_garbage_is_401` | ported: SecurityProxy > A01 a garbage api_key on /search does not authenticate
| `test_elastic_proxy_basic_auth_with_permission_reaches_the_cluster` | ported: SecurityProxy > A01 Basic auth with SearchRemote gets past authorization on /search
| `test_fop_filename_quote_stays_in_one_content_disposition` | ported: SecurityScreens > A01 a FOP filename quote stays in one Content-Disposition
| `test_datasnapshot_view_cannot_download` | ported: SecurityScreens > A01 a VIEW-only user cannot download a data snapshot
| `test_datasnapshot_parent_segment_filename_is_not_found` | ported: SecurityScreens > A01 a DataSnapshot download of a parent segment is not found
| `test_datasnapshot_parent_segment_delete_is_rejected` | ported: SecurityScreens > A01 a DataSnapshot #name of a parent segment is rejected
| `test_datasnapshot_parent_segment_import_is_rejected` | ported: SecurityScreens > A01 a DataSnapshot #name of a parent segment is rejected
| `test_elfinder_component_webroot_put_does_not_write_screen` | ported: SecurityScreens > A01 an ElFinder put does not write a screen under component://webroot
| `test_anonymous_tools_csv_is_not_a_data_dump` | ported: SecurityScreens > A01 an anonymous Tools CSV is not a data dump
| `test_view_only_tools_csv_is_not_a_privilege_escalation` | ported: SecurityScreens > A01 a VIEW-only Tools CSV is not a privilege escalation
| `test_authorized_tools_csv_is_csv` | ported: SecurityScreens > A01 a Tools CSV for a user who may open Tools is CSV
| `test_production_data_import_rejects_remote_location` | ported: SecurityScreens > A01 a data import rejects a remote location
| `test_multipart_executable_upload_is_rejected` | ported: SecurityMisc > A01 an uploaded #kind is #verdict

### test_a01_redirect.py

| Python test | Here |
| --- | --- |
| `test_login_returnTo_does_not_redirect_offsite` | ported: SecurityAccess > A01 the login does not follow a returnTo off the site |
| `test_login_referer_does_not_redirect_offsite` | ported: SecurityMisc > A01 the login does not follow a Referer off the site
| `test_login_returnTo_does_not_follow_host_header` | ported: SecurityMisc > A01 the login does not follow a returnTo from a spoofed Host |
| `test_login_returnTo_same_origin_path_is_allowed` | ported: SecurityAccess > A01 the login follows a returnTo on the site |

### test_a01_ws.py

| Python test | Here |
| --- | --- |
| `test_groovysh_anonymous_upgrade_is_not_a_shell` | not applicable: /groovysh is excluded |
| `test_groovysh_admin_can_eval` | not applicable: /groovysh is excluded |
| `test_groovysh_anonymous_after_admin_session_is_not_a_shell` | not applicable: /groovysh is excluded |
| `test_groovysh_view_only_is_not_a_shell` | not applicable: /groovysh is excluded |
| `test_notws_anonymous_upgrade_accepts_or_closes` | ported: SecurityWebSocket > A01 an anonymous /notws upgrade is 401 (replaced: the upgrade is 401 here; Java accepts it) |
| `test_websocket_evil_origin_is_rejected` | ported: SecurityWebSocket > A01 a /notws upgrade from a foreign Origin is refused, with a logged-in session (skips on the dev conf) |
| `test_groovysh_same_origin_admin_can_eval` | not applicable: /groovysh is excluded |

### test_a02_headers.py

| Python test | Here |
| --- | --- |
| `test_html_response_has_csp_and_frame_options` | ported: SecurityHeaders > A02 an HTML page has a CSP with frame-ancestors and form-action, X-Frame-Options and nosniff |
| `test_session_cookie_is_httponly` | ported: SecurityHeaders > A02 the session cookie is HttpOnly and SameSite=Lax |
| `test_cors_no_origin_header_has_no_allow_origin` | ported: SecurityHeaders > A02 a request with no Origin gets no Access-Control-Allow-Origin |
| `test_cors_unknown_origin_is_401` | ported: SecurityCors > A02 an unknown origin is 401 with no Access-Control-Allow-Origin (#method) |
| `test_cors_same_origin_returns_allow_origin` | ported: SecurityCors > A02 the server's own origin gets Access-Control-Allow-Origin with credentials |
| `test_cors_preflight_same_origin_is_allowed` | ported: SecurityCors > A02 a preflight from the server's own origin is allowed |
| `test_h2_console_is_not_mounted` | ported: SecurityHeaders > A02 no H2 console is mounted (there is no H2 console; the 404 proof is kept) |
| `test_session_cookie_samesite_lax` | ported: SecurityHeaders > A02 the session cookie is HttpOnly and SameSite=Lax |
| `test_login_html_is_not_publicly_cacheable` | ported: SecurityHeaders > A02 the login page is not publicly cacheable |
| `test_cors_preflight_unknown_origin_is_401` | ported: SecurityCors > A02 an unknown origin is 401 with no Access-Control-Allow-Origin (#method) |

### test_a05_injection.py

| Python test | Here |
| --- | --- |
| `test_rpc_set_preference_rejects_html_by_default` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_sql_looking_preference_is_stored_as_data` | ported: SecurityMisc > A05 a SQL-looking preference is stored as data

### test_a07_authn.py

| Python test | Here |
| --- | --- |
| `test_rest_login_succeeds_without_csrf_token` | ported: SecurityAuthn > A07 the REST login needs no session token |
| `test_post_without_csrf_token_rejected_after_login` | ported: SecurityAuthnMore > A07 a Service Run POST without the session token is rejected
| `test_post_with_csrf_token_is_not_a_csrf_error` | ported: SecurityAuthnMore > A07 a Service Run POST with the session token is not a session-token error
| `test_api_key_in_query_string_does_not_authenticate` | ported: SecurityAuthn > A07 an api_key #where does not log in |
| `test_post_with_csrf_token_executes_preference_round_trip` | ported: SecurityAuthn > A07 a POST runs with the session token and stores nothing without it (csrf: #csrf) |
| `test_post_without_csrf_does_not_store_preference` | ported: SecurityAuthn > A07 a POST runs with the session token and stores nothing without it (csrf: #csrf) |
| `test_api_key_header_with_garbage_does_not_authenticate` | ported: SecurityAuthn > A07 an api_key #where does not log in |
| `test_api_key_header_from_getLoginKey_authenticates` | ported: SecurityAuthnMore > A07 an api_key header of the fixture login key authenticates |
| `test_login_key_header_authenticates` | ported: SecurityAuthnMore > A07 a login_key header of the fixture login key authenticates
| `test_api_key_in_json_body_authenticates` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_http_basic_on_screen_is_not_unauthenticated` | ported: SecurityAuthnMore > A07 Basic auth on a screen is not unauthenticated
| `test_login_issues_a_new_session_id` | ported: SecurityAuthn > A07 the login form issues a new session id |

### test_a07_more.py

| Python test | Here |
| --- | --- |
| `test_rest_login_wrong_password_is_401` | ported: SecurityAuthnMore > A07 a REST login with the wrong password is 401 with the shared text
| `test_rest_login_unknown_user_is_401` | ported: SecurityAuthn > A07 a REST login of an unknown user is 401 with the shared text |
| `test_sendOtp_without_preauth_is_rejected` | ported: SecurityAuthnMore > A07 sendOtp without a pre-authenticated user is rejected |
| `test_verifyOtp_without_preauth_does_not_log_in` | ported: SecurityAuthnMore > A07 verifyOtp without a pre-authenticated user does not log in |
| `test_mfa_password_only_does_not_fully_log_in` | ported: SecurityAuthnMore > A07 an MFA password alone does not fully log in
| `test_mfa_login_with_wrong_code_does_not_log_in` | ported: SecurityAuthnMore > A07 an MFA login with the wrong code does not log in
| `test_mfa_login_with_code_succeeds` | ported: SecurityAuthnMore > A07 an MFA login with the single-use code succeeds
| `test_create_initial_admin_http_fails_when_users_exist` | ported: SecurityAuthnMore > A07 create initial admin over HTTP fails when users exist
| `test_short_password_is_rejected_over_http` | ported: SecurityAuthnMore > A07 a short password is rejected over HTTP
| `test_failed_logins_lock_dedicated_user` | ported: SecurityAuthnMore > A07 failed logins lock the dedicated user
| `test_login_json_response_omits_password_parameters` | ported: SecurityAuthnMore > A07 a login JSON response omits password parameters
| `test_login_json_failed_omits_password_parameters` | ported: SecurityAuthnMore > A07 a failed login JSON response omits password parameters
| `test_login_unknown_and_wrong_password_share_public_text` | ported: SecurityAuthnMore > A07 an unknown user and a wrong password share the public text
| `test_password_reset_unknown_user_shows_shared_message` | ported: SecurityAuthnMore > A07 a password reset for an unknown user shows the shared message
| `test_password_reset_unknown_and_existing_share_public_text` | ported: SecurityAuthnMore > A07 a password reset for an unknown and an existing user share the public text
| `test_ip_restricted_user_cannot_login_from_loopback` | ported: SecurityAuthnMore > A07 a user restricted to another address cannot log in from here
| `test_loopback_ip_allowed_user_can_login` | ported: SecurityAuthnMore > A07 a user allowed from loopback can log in

### test_a10_errors.py

| Python test | Here |
| --- | --- |
| `test_unauthorized_error_page_html_has_no_script_tag` | ported: SecurityMisc > A10 the unauthorized page escapes a message and has no script tag
| `test_rest_unauth_html_error_has_no_script_or_stack` | ported: SecurityMisc > A10 a REST 401 asked for as HTML has no script tag or stack frame
| `test_internal_error_page_does_not_include_stack_trace_when_hit_directly` | ported: SecurityMisc > A10 the internal error page hit directly has no stack trace
| `test_rest_unauth_json_does_not_include_stack_frames` | ported: SecurityErrors > A10 a REST 401 in JSON has no stack frames |

### test_api_rest.py

| Python test | Here |
| --- | --- |
| `test_rpc_without_login_does_not_run_create_user` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_rpc_find_enumeration_without_allow_is_not_user_create` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_rest_userInfo_transition_is_gone` | ported: SecurityAccess > A01 the removed endpoint #method #path is 404, not merely unauthenticated |
| `test_rest_e1_user_account_without_entity_authz_is_not_200` | ported: SecurityRest > A01 john.doe reading entity REST UserAccount is not a password dump
| `test_x_http_method_override_without_auth_is_401` | ported: SecurityRest > A01 a method override on entity REST #case
| `test_x_http_method_override_still_requires_csrf` | ported: SecurityRest > A01 a method override on entity REST #case
| `test_entity_rest_post_without_csrf_does_not_create` | ported: SecurityRest > A01 an entity REST POST without the session token does not create
| `test_rpc_json_without_csrf_does_not_reset_password` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_rest_e1_basic_auth_wrong_password_is_401` | ported: SecurityAuthn > A07 Basic on /rest/e1: #who (an unknown user, not john.doe with a wrong password) |
| `test_rest_e1_basic_auth_valid_is_not_unauthenticated` | ported: SecurityAuthn > A07 Basic on /rest/e1: #who |
| `test_system_message_hmac_missing_header_is_403` | ported: SecurityMessages > A01 a /rest/sm HMAC post with no signature header is 403
| `test_system_message_hmac_bad_signature_is_403` | ported: SecurityMessages > A01 a /rest/sm HMAC post with a bad signature is 403
| `test_system_message_hmac_valid_does_not_require_login` | ported: SecurityMessages > A01 a valid /rest/sm HMAC signature needs no login
| `test_system_message_hmac_timestamp_outside_window_is_403` | ported: SecurityMessages > A01 a /rest/sm timestamp HMAC outside the window is 403
| `test_system_message_hmac_timestamp_in_window_is_accepted` | ported: SecurityMessages > A01 a /rest/sm timestamp HMAC inside the window is accepted
| `test_system_message_smat_none_accepts_anonymous` | ported: SecurityMessages > A01 a /rest/sm SmatNone remote accepts an anonymous post
| `test_entity_rest_narrow_authz_allows_only_its_entity` | ported: SecurityRest > A01 a narrow entity VIEW reads only its entity and cannot create
| `test_entity_rest_catchall_authz_allows_other_entities` | ported: SecurityRest > A01 a catch-all entity ALL reads another entity and can create
| `test_entity_sync_put_needs_all_authz` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess); the proof calls put#EntitySyncData over /rpc/json |
| `test_entity_sync_put_with_all_authz_stores` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess); the proof calls put#EntitySyncData over /rpc/json |
| `test_service_rest_moqui_api_view_can_read_users` | ported: SecurityRest > A01 MOQUI_API #who can list users and the JSON omits password fields
| `test_service_rest_users_one_omits_password_hash_fields` | ported: SecurityRest > A01 #which omits UserAccount password fields
| `test_entity_rest_user_account_omits_password_hash_fields` | ported: SecurityRest > A01 #which omits UserAccount password fields |
| `test_service_rest_moqui_api_view_cannot_update_users` | ported: SecurityRest > A01 #who is 403
| `test_service_rest_no_api_authz_cannot_read_users` | ported: SecurityRest > A01 #who is 403
| `test_service_rest_moqui_api_all_can_read_users` | ported: SecurityRest > A01 MOQUI_API #who can list users and the JSON omits password fields
| `test_x_http_method_override_authz_uses_override_action` | ported: SecurityRest > A01 a method override is authorized as #action
| `test_service_rest_users_patch_does_not_store_current_password` | ported: SecurityRest > A01 a user PATCH does not store currentPassword
| `test_service_rest_users_patch_does_not_store_disabled` | ported: SecurityRest > A01 a user PATCH does not store disabled
| `test_service_rest_users_patch_stores_full_name` | ported: SecurityRest > A01 a user PATCH stores userFullName
| `test_entity_rest_does_not_create_user_login_key` | ported: SecurityRest > A01 entity REST does not create or read UserLoginKey
| `test_email_server_host_not_writable_without_admin` | ported: SecurityRest > A01 an email server host is not writable without ADMIN
| `test_entity_sync_put_does_not_store_user_group_permission` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess); the proof calls put#EntitySyncData over /rpc/json |
| `test_jsonrpc_array_body_returns_one_result_per_call` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_jsonrpc_array_body_over_limit_is_rejected` | not applicable: JSON-RPC (/rpc/json) was left out on purpose; /rpc/json is 404 (SecurityAccess) |
| `test_system_message_hmac_timestamp_replay_is_rejected` | ported: SecurityMessages > A01 a replayed /rest/sm timestamp HMAC is rejected
| `test_rest_schema_anonymous_is_401` | not applicable: /rest/entity.swagger answers 404 before any auth check (Swagger 2 is not built); the schema proof on /rest/entity.json is test_rest_schema_without_permission_is_403 |
| `test_rest_schema_without_permission_is_403` | ported: SecurityRest > A01 /rest/entity.json #who
| `test_rest_schema_does_not_set_acao_wildcard` | not applicable: /rest/entity.swagger answers 404 before any auth check (Swagger 2 is not built); the schema proof on /rest/entity.json is test_rest_schema_without_permission_is_403 |

### test_z_tarpit.py

| Python test | Here |
| --- | --- |
| `test_screen_tarpit_returns_429_after_burst` | ported: SecurityTarpit > A01 the dashboard is 429 after the ALL_SCREENS burst
| `test_transition_tarpit_for_sec_tap_user_returns_429` | ported: SecurityTarpit > A01 setPreference is 429 under sec.tap.user's transition tarpit
