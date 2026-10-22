# Mail sign-in with Google and Microsoft (OAuth)

A mail account of the mail client (`org.moqui.impl.EmailClientServices`) can sign in to Gmail, Google Workspace, Microsoft 365
or Outlook.com with the provider's own sign-in instead of a password: OAuth 2.0 authorization code with PKCE in a popup, the
refresh token stored encrypted on the account's `EmailServer`, and XOAUTH2 for IMAP (the sync, the IDLE workers, the server
test), POP3 (the sync and the server test, `AUTH XOAUTH2`) and SMTP (sending). The code is `crates/moqui-core/src/service/mail/oauth.rs`; the providers are
`moqui.basic.email.EmailOAuthProvider` rows (Google and Microsoft are seeded). This page is what a deployment sets up. As of
Oct 19, 2026.

## What the deployment configures

Each provider needs the deployment's own OAuth client. Nothing is shared: no client id or secret ships with the framework.

| Property (or environment variable) | |
| --- | --- |
| `email_oauth_redirect_uri` | The callback registered at both providers, exactly. Empty: the `webroot` webapp's URL from the conf (`webapp_http_host`, `webapp_https_enabled`, the ports) and `/emailOAuth/callback`, such as `https://erp.example.com/emailOAuth/callback`. It is never taken from a request. |
| `email_oauth_google_client_id`, `email_oauth_google_client_secret` | The Google client. |
| `email_oauth_microsoft_client_id`, `email_oauth_microsoft_client_secret` | The Microsoft client. |
| `email_oauth_microsoft_tenant` | `common` (the default: work, school and personal accounts), `organizations`, `consumers` or a tenant id. |
| `entity_ds_crypt_pass` | Must be the deployment's own: the refresh tokens (and a client secret kept on the `EmailOAuthProvider` row instead of a property) are encrypted with it. A server with no field encryption refuses to store a sign-in. |

A provider whose client id or secret is missing, or whose row is `disabled`, is not offered: `discover#EmailServer` answers
`oauthConfigured` false and `start#OAuthSignIn` refuses it. `get#OAuthProviders` lists the providers, whether each is set up,
and the redirect URI to register. The client secret can also be set on the `EmailOAuthProvider` row (`clientSecret`, an
encrypted field); a property wins over the row.

## Google (Gmail and Google Workspace)

1. In the Google Cloud console make a project, then in **Google Auth Platform** set the branding (app name, support email,
   authorized domain of the redirect URI), the **audience**, and under **Data access** the scopes `https://mail.google.com/`,
   `openid` and `.../auth/userinfo.email`.
2. **Clients**, create an OAuth client of type **Web application** with the redirect URI above as an authorized redirect URI.
3. Set `email_oauth_google_client_id` and `email_oauth_google_client_secret`.

What Google requires (checked Oct 19, 2026 against Google's documentation):

- `https://mail.google.com/` is the only scope IMAP, POP and SMTP accept, and it is a **restricted** scope.
- **Audience Internal** (a Google Workspace organization, for its own users only): no verification, no user cap, and refresh
  tokens do not expire after a week. The right choice for a company's own mail.
- **Audience External, publishing status Testing**: only the listed test users (at most 100) can sign in, past the
  "unverified app" warning, and their **refresh tokens expire after 7 days**: each account then needs a new sign-in (the
  account shows "Sign-in failed" and its Fix). Fine for trying it out, not for daily use.
- **External, In production**: needs Google's verification, and for a restricted scope whose data reaches a server, the
  restricted-scope review with an annual third-party security assessment (CASA). Google lists exceptions: personal use (fewer
  than 100 users, who see the unverified-app warning), internal use, and a Workspace administrator marking the app trusted in
  the Admin console.
- A refresh token also stops working when the user changes their Google password (Gmail scopes), after six months unused, when
  the user removes the app's access, or past 100 refresh tokens per account per client (the oldest goes). Since March 14, 2025
  Google accepts only OAuth (or an app password) for third-party IMAP and SMTP.

## Microsoft (Microsoft 365 and Outlook.com)

1. In the Microsoft Entra admin center, **App registrations**, register an app. **Supported account types**: "Accounts in any
   organizational directory and personal Microsoft accounts" for the `common` tenant (work, school and Outlook.com); a single
   organization may register for its own directory only and set `email_oauth_microsoft_tenant` to its tenant id.
2. **Authentication**: a **Web** platform with the redirect URI above.
3. **Certificates & secrets**: a client secret (it expires, at most after 24 months: rotate it before then).
4. **API permissions**: the delegated permissions `IMAP.AccessAsUser.All`, `SMTP.Send`, `offline_access`, `openid`, `email` and
   `profile`, and `POP.AccessAsUser.All` when accounts read their mail over POP3. The sign-in asks for them as
   `https://outlook.office.com/IMAP.AccessAsUser.All` and `https://outlook.office.com/SMTP.Send`, with `offline_access` for the
   refresh token; an account whose server is POP3 also asks for `https://outlook.office.com/POP.AccessAsUser.All` (the provider's
   `popScope`), which must be granted, and its refreshes ask for it too.
5. Set `email_oauth_microsoft_client_id` and `email_oauth_microsoft_client_secret`.

What Microsoft requires (from Microsoft's "Authenticate an IMAP, POP or SMTP connection using OAuth"; the dates below to
verify against the Message Center when deploying):

- OAuth for IMAP, POP and SMTP works for Microsoft 365 and Outlook.com accounts. A tenant that does not let users consent to
  apps needs an administrator to grant consent once.
- Microsoft 365: IMAP must be on for the mailbox, and **SMTP AUTH** must be allowed for it (it is off by default in many
  tenants: `Set-CASMailbox -SmtpClientAuthenticationDisabled $false`, or the organization setting). Basic (password) SMTP AUTH
  is being retired in Exchange Online (Message Center MC786329; the latest timeline seen disables it by default at the end of
  December 2026). To verify.
- Outlook.com stopped taking passwords (basic authentication) for IMAP and SMTP in September 2024, so a personal Microsoft
  account needs this sign-in. To verify.
- Microsoft has no endpoint that revokes a single refresh token: removing an account deletes the token here only (a user removes
  the app at account.microsoft.com, or an administrator in Entra). It rotates refresh tokens; the new one is stored at each refresh.

## POP3

An account whose incoming server is POP3 (`storeProtocol` pop3 or pop3s) signs in with SASL XOAUTH2 (`AUTH XOAUTH2`, RFC 5034):
the response goes after the server's `+` continuation, never on the AUTH line (RFC 5034 keeps that to 255 octets, and an access
token is longer); a refused token is the provider's `+` error challenge, answered with an empty line, then `-ERR`, and is tried
once more with a new token. The sign-in asks for the provider's `popScope` too (Microsoft: `POP.AccessAsUser.All`; Google's
`https://mail.google.com/` covers POP, and POP must be on in the Gmail settings). A new account's sign-in makes an IMAP account,
or with `start#OAuthSignIn storeProtocol pop3` a POP account on the provider's POP3 server (`EmailOAuthProvider.popHost`,
`popPort`, `popProtocol`: seeded `pop.gmail.com` and `outlook.office365.com`, port 995, `pop3s`; a provider with no `popHost`
refuses it). That sign-in asks for the POP scope, and the account is made only when it succeeds with the scope granted.
`get#OAuthProviders` answers each provider's IMAP, POP3 and SMTP servers. An existing account is POP once its server is set to
POP3 (update#EmailAccount), and its next sign-in then asks for the POP scope; an account switched to POP3 after its sign-in fails
its first POP sign-in at Microsoft (no POP scope granted) and waits for a new sign-in.

## The flow

1. The wizard (or a Fix button) calls `start#OAuthSignIn` with the account (`emailAccountId`) or a new account's address
   (`emailAddress`, `accountName`, `displayName`, `accountKindEnumId`, `storeProtocol` imap or pop3) and gets `{authUrl, signInId}`; `moqui.oauthPopup` opens the
   popup in the click and points it there (`webroot/js/README.md`, "Mail sign-in popup"). A new account is made paused
   (`statusId` `EasPaused`, the default) with a sync window of `syncWindowDays` (365 by default, 0 for all mail), so the sync job
   leaves it until the app has finished its setup and made it active (`update#EmailAccount statusId EasActive`); `statusId`
   `EasActive` makes it sync at once, only with a window. An account signing in again keeps its status and window.
   `find#OAuthProvider(emailAddress, storeHost)` says whose sign-in an address takes with no network (the domain, then the incoming
   server; the MX hosts only with `lookupMx`), and `get#OAuthProviders` lists each provider's domains and suffixes.
2. The provider sends the popup back to `/emailOAuth/callback` (the opener also asks `get#OAuthStatus` every two seconds, so a
   page whose window Google's Cross-Origin-Opener-Policy cut off hears the outcome; it gives up after three minutes, and the
   promise's `cancel()` stops it). `complete#OAuthSignIn` takes the state once from the user's
   session, exchanges the code with the PKCE verifier, checks the granted mail scopes and the ID token (audience, issuer, expiry,
   nonce; its signature is not checked, as it comes straight from the token endpoint over TLS), compares the signed-in address with
   the account's, stores the refresh token encrypted (the first access token stays in memory), and makes the account (or makes it
   active again). The page tells the opener (this server's own origin only) and closes.
3. Before each IMAP or SMTP sign-in an access token is refreshed when it ends within a minute, one refresh at a time per account.
   The access token is kept in memory and, encrypted, on `EmailServerAccessToken` (not cached, not on the exported
   `EmailServer`), so the other servers of a cluster use it with no refresh of their own. A refresh locks the `EmailServer` row for
   update, so one server of the cluster refreshes at a time; it reads the stored token again under the lock (another server may
   have refreshed meanwhile), and stores the token and a new refresh token from a provider that rotates them (Microsoft) in the
   same transaction, so none is lost. A stored token that will not decrypt, or that a mail server refused, is refreshed.
   A refresh token the provider refuses marks the account EasAuthFailed ("sign in again"), logged once; the other accounts go on.
4. `delete#EmailAccount`, `revoke#OAuthSignIn` (sign out) and switching the account back to a password revoke the token at the
   provider (Google) after the commit, best effort, and delete it and the stored access token here. A sign-in of the same address on another account here is
   not revoked, because the provider revokes the whole grant.

## Security

- Tokens never leave the server: no service answers one, the popup's page carries none, and logs name only the server.
- The state is single use, kept server side in the user's session with the PKCE verifier and nonce, bound to the user and to the
  account or address, and ends after ten minutes.
- The redirect URI is the deployment's, PKCE (S256) is always used, and the page posts only to its own origin.
- A sign-in as a different address than the account's is refused (`addressMismatch`) unless the user confirmed the change
  (`allowAddressChange`), which then changes the account's address.

## Not done

Microsoft Graph or Gmail API access; calendar and contact sync over the same client
(design §21 keeps those for later); a check of the ID token's signature (not needed for a token from the token endpoint).
