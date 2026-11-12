/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The SSO relying party behind `moqui_core::sso` (`plans/archive/Sso.md`): `huskarl` runs the authorization code grant with
//! PKCE, the state and nonce checks, and the ID token validation against the provider's JWKS; this module gives it an
//! HTTP client (async reqwest on the edge runtime, 1 MiB bodies, no redirects), builds a grant for each `AuthFlow`, and
//! reads the user info.
//!
//! OpenID Connect discovery is `discoveryUri`, or the provider's well-known document: Google and Apple have fixed
//! ones, Keycloak is `baseUri/realms/realm`, and Azure AD is the `common` tenant unless `discoveryUri` names one. The
//! Moqui username is `preferred_username`, else `email`. OAuth 2 flows use the row's URLs or the named client's
//! endpoints (the pac4j clients of the Java component). OAuth 1.0 (`OctOauth10`, Twitter) is not carried forward. The
//! client secret goes by HTTP Basic, as Java's `CLIENT_SECRET_BASIC`; Apple's is a JWT signed with the ES256 key in
//! `secret`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use bytes::Bytes;
use huskarl::core::client_auth::ClientSecret;
use huskarl::core::error::RetryAdvice;
use huskarl::core::http::{HttpClient, HttpResponse, Idempotency};
use huskarl::core::platform::MaybeSendBoxFuture;
use huskarl::core::secrets::{ProvidedSecret, SecretString};
use huskarl::core::server_metadata::AuthorizationServerMetadata;
use huskarl::core::EndpointUrl;
use huskarl::grant::authorization_code::{AuthorizationCodeGrant, CompleteInput, PendingState, StartInput};
use moqui_core::sso::{FlowConfig, FlowKind, OidcConfig, SsoClient, SsoProfile, SsoStart};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use serde_json::Value as Json;

/// Largest provider response read.
const MAX_BODY: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
/// A grant (with its discovery document and, for Apple, its client secret) is rebuilt after this long.
const GRANT_LIFE: Duration = Duration::from_secs(6 * 60 * 60);

/// Install the client into core.
pub fn install() {
    moqui_core::sso::install_sso_client(Arc::new(HuskarlSso::default()));
}

/// The HTTP transport huskarl uses.
#[derive(Clone)]
struct ReqwestTransport {
    client: reqwest::Client,
    /// No request goes to an internal address (`FlowConfig::deny_internal`).
    deny_internal: bool,
}

impl ReqwestTransport {
    /// A transport over one of the two SSO clients, so connections and the TLS setup are reused across logins. With
    /// `deny_internal` (a flow whose `allowPrivateNetwork` is `N`) the client's resolver refuses a host with a
    /// loopback, private or link-local address, as `moqui_lib::http::RestClient::deny_internal_addresses` does, and
    /// [`send`](Self::send) refuses such an address written out. Neither client follows a redirect.
    fn new(deny_internal: bool) -> Result<Self, Error> {
        static OPEN: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
        static PUBLIC: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
        let slot = if deny_internal { &PUBLIC } else { &OPEN };
        let client = slot
            .get_or_init(|| {
                let mut builder = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(TIMEOUT);
                if deny_internal {
                    builder = builder.dns_resolver(Arc::new(moqui_lib::http::PublicOnly));
                }
                builder.build().map_err(|err| err.to_string())
            })
            .as_ref()
            .map_err(|err| Error::new(format!("could not build the SSO HTTP client: {err}")))?;
        Ok(Self {
            client: client.clone(),
            deny_internal,
        })
    }

    async fn send(&self, request: http::Request<Bytes>) -> Result<HttpResponse, String> {
        let (parts, body) = request.into_parts();
        if self.deny_internal {
            let url = reqwest::Url::parse(&parts.uri.to_string()).map_err(|err| err.to_string())?;
            if moqui_lib::http::internal_literal(&url) {
                return Err(format!(
                    "{} is an internal address, which this provider may not use",
                    url.host_str().unwrap_or_default()
                ));
            }
        }
        let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).map_err(|err| err.to_string())?;
        let mut builder = self.client.request(method, parts.uri.to_string());
        for (name, value) in &parts.headers {
            builder = builder.header(name.as_str(), value.as_bytes());
        }
        let mut response = builder.body(body.to_vec()).send().await.map_err(|err| {
            // the resolver's refusal of an internal address is the reason, under reqwest's own text
            let mut source = std::error::Error::source(&err);
            while let Some(cause) = source {
                let text = cause.to_string();
                if text.contains("is an internal address") {
                    return text;
                }
                source = cause.source();
            }
            err.to_string()
        })?;
        let status = http::StatusCode::from_u16(response.status().as_u16()).map_err(|err| err.to_string())?;
        let mut headers = http::HeaderMap::new();
        for (name, value) in response.headers() {
            if let (Ok(name), Ok(value)) = (
                http::HeaderName::from_bytes(name.as_str().as_bytes()),
                http::HeaderValue::from_bytes(value.as_bytes()),
            ) {
                headers.append(name, value);
            }
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(format!("the response is larger than {MAX_BODY} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            headers,
            body: Bytes::from(body),
        })
    }
}

impl HttpClient for ReqwestTransport {
    fn execute(
        &self,
        request: http::Request<Bytes>,
        _idempotency: Idempotency,
    ) -> MaybeSendBoxFuture<'_, Result<HttpResponse, huskarl::core::Error>> {
        Box::pin(async move {
            self.send(request)
                .await
                .map_err(|message| huskarl::core::Error::new(RetryAdvice::No, std::io::Error::other(message)))
        })
    }
}

/// A grant built for one flow and redirect URI.
struct Built {
    key: String,
    at: Instant,
    grant: Arc<AuthorizationCodeGrant>,
    /// The discovery document, for OIDC.
    metadata: Option<Arc<AuthorizationServerMetadata>>,
    /// The OIDC user info endpoint, from discovery; the OAuth one is the provider's.
    user_info: Option<String>,
}

/// The client core calls. The grants are shared with the tasks on the edge runtime.
#[derive(Default)]
pub struct HuskarlSso {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    grants: Mutex<HashMap<String, Arc<Built>>>,
}

/// The endpoints and profile fields of a named OAuth 2 client.
struct Provider {
    authorize: &'static str,
    token: &'static str,
    user_info: &'static str,
    scope: &'static str,
    /// The profile field that is the provider's id, and the one that is the username.
    id: &'static str,
    username: &'static str,
    /// Dropbox reads the account with a POST.
    user_info_post: bool,
}

fn provider(client_type: &str) -> Option<Provider> {
    let entry = |authorize, token, user_info, scope, id, username| Provider {
        authorize,
        token,
        user_info,
        scope,
        id,
        username,
        user_info_post: false,
    };
    Some(match client_type {
        "OctGitHub" => entry(
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            "https://api.github.com/user",
            "read:user user:email",
            "id",
            "login",
        ),
        "OctGoogle" => entry(
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://openidconnect.googleapis.com/v1/userinfo",
            "email profile",
            "sub",
            "email",
        ),
        "OctFacebook" => entry(
            "https://www.facebook.com/v19.0/dialog/oauth",
            "https://graph.facebook.com/v19.0/oauth/access_token",
            "https://graph.facebook.com/me?fields=id,name,email,first_name,last_name",
            "email public_profile",
            "id",
            "email",
        ),
        "OctLinkedIn" => entry(
            "https://www.linkedin.com/oauth/v2/authorization",
            "https://www.linkedin.com/oauth/v2/accessToken",
            "https://api.linkedin.com/v2/userinfo",
            "openid profile email",
            "sub",
            "email",
        ),
        "OctBitBucket" => entry(
            "https://bitbucket.org/site/oauth2/authorize",
            "https://bitbucket.org/site/oauth2/access_token",
            "https://api.bitbucket.org/2.0/user",
            "account email",
            "uuid",
            "username",
        ),
        "OctDropBox" => Provider {
            user_info_post: true,
            ..entry(
                "https://www.dropbox.com/oauth2/authorize",
                "https://api.dropboxapi.com/oauth2/token",
                "https://api.dropboxapi.com/2/users/get_current_account",
                "account_info.read",
                "account_id",
                "email",
            )
        },
        "OctPaypal" => entry(
            "https://www.paypal.com/signin/authorize",
            "https://api-m.paypal.com/v1/oauth2/token",
            "https://api-m.paypal.com/v1/identity/oauth2/userinfo?schema=paypalv1.1",
            "openid email profile",
            "user_id",
            "email",
        ),
        "OctWordPress" => entry(
            "https://public-api.wordpress.com/oauth2/authorize",
            "https://public-api.wordpress.com/oauth2/token",
            "https://public-api.wordpress.com/rest/v1/me",
            "auth",
            "ID",
            "username",
        ),
        "OctYahoo" => entry(
            "https://api.login.yahoo.com/oauth2/request_auth",
            "https://api.login.yahoo.com/oauth2/get_token",
            "https://api.login.yahoo.com/openid/v1/userinfo",
            "openid email profile",
            "sub",
            "email",
        ),
        "OctFoursquare" => entry(
            "https://foursquare.com/oauth2/authenticate",
            "https://foursquare.com/oauth2/access_token",
            "https://api.foursquare.com/v2/users/self?v=20240101",
            "",
            "id",
            "email",
        ),
        _ => return None,
    })
}

/// The OIDC discovery document location of a flow.
fn discovery_uri(config: &OidcConfig) -> Result<String, Error> {
    if !config.discovery_uri.is_empty() {
        return Ok(config.discovery_uri.clone());
    }
    let well_known = "/.well-known/openid-configuration";
    match config.client_type.as_str() {
        "OctGoogle" => Ok(format!("https://accounts.google.com{well_known}")),
        "OctApple" => Ok(format!("https://appleid.apple.com{well_known}")),
        "OctAzureAd" => Ok(format!("https://login.microsoftonline.com/common/v2.0{well_known}")),
        "OctKeycloak" if !config.base_uri.is_empty() && !config.realm.is_empty() => Ok(format!(
            "{}/realms/{}{well_known}",
            config.base_uri.trim_end_matches('/'),
            config.realm
        )),
        _ => Err(Error::new("the OidcFlow has no discoveryUri")),
    }
}

fn endpoint(url: &str, what: &str) -> Result<EndpointUrl, Error> {
    url.parse::<EndpointUrl>()
        .map_err(|err| Error::new(format!("the {what} URL {url} is not valid: {err}")))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Apple's client secret: an ES256 JWT from the team to `https://appleid.apple.com`, signed with the private key.
fn apple_client_secret(config: &OidcConfig) -> Result<String, Error> {
    use p256::ecdsa::signature::Signer;
    use p256::pkcs8::DecodePrivateKey;
    if config.apple_team_id.is_empty() || config.apple_key_id.is_empty() {
        return Err(Error::new("an Apple OidcFlow needs appleTeamId and appleKeyId"));
    }
    let key = p256::ecdsa::SigningKey::from_pkcs8_pem(config.secret.trim())
        .map_err(|err| Error::new(format!("the Apple private key in secret does not decode: {err}")))?;
    let now = now_secs();
    let encode = |value: &Json| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    let header = encode(&serde_json::json!({ "alg": "ES256", "kid": config.apple_key_id }));
    let claims = encode(&serde_json::json!({
        "iss": config.apple_team_id,
        "iat": now,
        "exp": now + 86_400,
        "aud": "https://appleid.apple.com",
        "sub": config.client_id,
    }));
    let signing_input = format!("{header}.{claims}");
    let signature: p256::ecdsa::Signature = key.sign(signing_input.as_bytes());
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
    Ok(format!("{signing_input}.{signature}"))
}

fn client_auth(secret: String) -> ClientSecret {
    ClientSecret::builder()
        .client_secret(ProvidedSecret::new(SecretString::new(secret)))
        .prefer_basic_auth(true)
        .build()
}

impl Inner {
    async fn built(&self, flow: &FlowConfig, redirect_uri: &str) -> Result<Arc<Built>, Error> {
        // `Debug` leaves the secret out, so the key carries a hash of it: a replaced secret builds a new grant at once
        // (else the old one would be sent until the grant's life ended).
        let secret_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            match &flow.kind {
                FlowKind::Oidc(config) => config.secret.hash(&mut hasher),
                FlowKind::Oauth(config) => config.secret.hash(&mut hasher),
            }
            hasher.finish()
        };
        let key = format!("{flow:?}|{redirect_uri}|{secret_hash:x}");
        if let Some(found) = lock(&self.grants)
            .get(&flow.auth_flow_id)
            .filter(|built| built.key == key && built.at.elapsed() < GRANT_LIFE)
        {
            return Ok(Arc::clone(found));
        }
        let transport = ReqwestTransport::new(flow.deny_internal)?;
        let built = match &flow.kind {
            FlowKind::Oidc(config) => {
                let discovery = discovery_uri(config)?;
                let response = transport
                    .send(
                        http::Request::get(discovery.as_str())
                            .header("Accept", "application/json")
                            .body(Bytes::new())
                            .map_err(|err| Error::new(format!("discovery URL {discovery}: {err}")))?,
                    )
                    .await
                    .map_err(|err| Error::new(format!("could not read {discovery}: {err}")))?;
                if !response.status.is_success() {
                    return Err(Error::new(format!("{discovery} answered {}", response.status)));
                }
                // The reason is the kind of error and where, never the parser's text, which quotes a value of the
                // body: a test of a provider (`check#Flow`) shows this message, and the URL is its administrator's.
                let metadata: AuthorizationServerMetadata = serde_json::from_slice(&response.body).map_err(|err| {
                    let what = match err.classify() {
                        serde_json::error::Category::Data => "its JSON is not that of an OpenID provider",
                        _ => "it is not JSON",
                    };
                    Error::new(format!(
                        "{discovery} is not a discovery document: {what} (line {}, column {})",
                        err.line(),
                        err.column()
                    ))
                })?;
                if !config.preferred_jws_algorithm.is_empty() && config.preferred_jws_algorithm == "ES512" {
                    return Err(Error::new(
                        "ES512 ID tokens are not supported; choose ES256, ES384, RS*, PS* or EdDSA",
                    ));
                }
                let secret = if config.client_type == "OctApple" {
                    apple_client_secret(config)?
                } else {
                    config.secret.clone()
                };
                let user_info = metadata.userinfo_endpoint.as_ref().map(|url| url.as_uri().to_string());
                let grant = AuthorizationCodeGrant::builder_from_metadata(&metadata)
                    .map_err(|err| Error::new(format!("{discovery}: {err}")))?
                    .client_id(config.client_id.clone())
                    .http_client(transport.clone())
                    .client_auth(client_auth(secret))
                    .redirect_uri(redirect_uri.to_string())
                    .oidc(true)
                    .build()
                    .await
                    .map_err(|err| Error::new(format!("SSO flow {}: {err}", flow.auth_flow_id)))?;
                Built {
                    key,
                    at: Instant::now(),
                    grant: Arc::new(grant),
                    metadata: Some(Arc::new(metadata)),
                    user_info,
                }
            }
            FlowKind::Oauth(config) => {
                if matches!(config.client_type.as_str(), "OctOauth10" | "OctTwitter") {
                    return Err(Error::new(format!(
                        "SSO flow {}: OAuth 1.0 ({}) is not supported",
                        flow.auth_flow_id, config.client_type
                    )));
                }
                let known = provider(&config.client_type);
                let pick = |own: &str, default: Option<&'static str>| {
                    if own.is_empty() {
                        default.unwrap_or("").to_string()
                    } else {
                        own.to_string()
                    }
                };
                let authorize = pick(&config.authorize_url, known.as_ref().map(|p| p.authorize));
                let token = pick(&config.token_url, known.as_ref().map(|p| p.token));
                let user_info = pick(&config.user_info_url, known.as_ref().map(|p| p.user_info));
                if authorize.is_empty() || token.is_empty() {
                    return Err(Error::new(format!(
                        "SSO flow {}: an OAuth 2 flow needs authorizeUrl and tokenUrl",
                        flow.auth_flow_id
                    )));
                }
                let grant = AuthorizationCodeGrant::builder()
                    .authorization_endpoint(endpoint(&authorize, "authorize")?)
                    .token_endpoint(endpoint(&token, "token")?)
                    .client_id(config.client_id.clone())
                    .http_client(transport.clone())
                    .client_auth(client_auth(config.secret.clone()))
                    .redirect_uri(redirect_uri.to_string())
                    .oidc(false)
                    .build()
                    .await
                    .map_err(|err| Error::new(format!("SSO flow {}: {err}", flow.auth_flow_id)))?;
                Built {
                    key,
                    at: Instant::now(),
                    grant: Arc::new(grant),
                    metadata: None,
                    user_info: (!user_info.is_empty()).then_some(user_info),
                }
            }
        };
        let built = Arc::new(built);
        lock(&self.grants).insert(flow.auth_flow_id.clone(), Arc::clone(&built));
        Ok(built)
    }

    async fn start_async(&self, flow: FlowConfig, redirect_uri: String) -> Result<SsoStart, Error> {
        let built = self.built(&flow, &redirect_uri).await?;
        let scope: Vec<String> = match &flow.kind {
            // Apple sends a form post for the name and email scopes; openid alone comes back in the query.
            FlowKind::Oidc(config) if config.client_type == "OctApple" => vec!["openid".to_string()],
            FlowKind::Oidc(_) => ["openid", "profile", "email"].map(str::to_string).to_vec(),
            FlowKind::Oauth(config) => {
                let scope = if config.scope.is_empty() {
                    provider(&config.client_type).map_or("", |p| p.scope).to_string()
                } else {
                    config.scope.clone()
                };
                scope.split_whitespace().map(str::to_string).collect()
            }
        };
        let started = built
            .grant
            .start(StartInput::scope(scope))
            .await
            .map_err(|err| Error::new(format!("SSO flow {}: {err}", flow.auth_flow_id)))?;
        let pending = serde_json::to_string(&started.pending_state)
            .map_err(|err| Error::new(format!("SSO state does not serialize: {err}")))?;
        Ok(SsoStart {
            authorize_url: started.authorization_url.to_string(),
            pending,
        })
    }

    async fn complete_async(
        &self,
        flow: FlowConfig,
        redirect_uri: String,
        pending: String,
        callback: BTreeMap<String, String>,
    ) -> Result<SsoProfile, Error> {
        let built = self.built(&flow, &redirect_uri).await?;
        let pending: PendingState =
            serde_json::from_str(&pending).map_err(|err| Error::new(format!("SSO state is not readable: {err}")))?;
        let query: String = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(callback.iter())
            .finish();
        let input: CompleteInput = query
            .parse()
            .map_err(|err| Error::new(format!("the SSO callback is not readable: {err}")))?;
        let completed = built
            .grant
            .complete(&pending, input)
            .await
            .map_err(|err| Error::new(format!("SSO flow {}: {err}", flow.auth_flow_id)))?;
        let access_token = completed
            .token_response
            .access_token()
            .token()
            .expose_secret()
            .to_string();
        let mut attributes = serde_json::Map::new();
        let mut id_token = None;
        if let Some(validated) = &completed.id_token {
            if let Json::Object(claims) = serde_json::to_value(&validated.claims).unwrap_or(Json::Null) {
                attributes.extend(claims);
            }
            if let Some(sub) = &validated.sub {
                attributes.insert("sub".to_string(), Json::String(sub.clone()));
            }
            if let Some(iss) = &validated.iss {
                attributes.insert("iss".to_string(), Json::String(iss.clone()));
            }
            id_token = completed
                .token_response
                .id_token()
                .map(|token| token.token().to_string());
        }
        let provider = match &flow.kind {
            FlowKind::Oauth(config) => provider(&config.client_type),
            FlowKind::Oidc(_) => None,
        };
        if let Some(user_info) = &built.user_info {
            let post = provider.as_ref().is_some_and(|p| p.user_info_post);
            let info = self
                .user_info(user_info, &access_token, post, flow.deny_internal)
                .await?;
            for (name, value) in info {
                attributes.entry(name).or_insert(value);
            }
        }
        if matches!(flow.kind, FlowKind::Oidc(_)) && completed.id_token.is_none() {
            return Err(Error::new(format!(
                "SSO flow {}: the provider sent no ID token",
                flow.auth_flow_id
            )));
        }
        Ok(profile(&flow, attributes, provider.as_ref(), id_token))
    }

    async fn user_info(
        &self,
        url: &str,
        access_token: &str,
        post: bool,
        deny_internal: bool,
    ) -> Result<serde_json::Map<String, Json>, Error> {
        let transport = ReqwestTransport::new(deny_internal)?;
        let request = if post {
            http::Request::post(url)
        } else {
            http::Request::get(url)
        }
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .body(Bytes::new())
        .map_err(|err| Error::new(format!("user info URL {url}: {err}")))?;
        let response = transport
            .send(request)
            .await
            .map_err(|err| Error::new(format!("could not read the user info at {url}: {err}")))?;
        if !response.status.is_success() {
            return Err(Error::new(format!(
                "the user info at {url} answered {}",
                response.status
            )));
        }
        match serde_json::from_slice::<Json>(&response.body) {
            Ok(Json::Object(map)) => Ok(map),
            _ => Err(Error::new(format!("the user info at {url} is not a JSON object"))),
        }
    }
}

fn text_of(value: Option<&Json>) -> Option<String> {
    match value? {
        Json::String(text) if !text.is_empty() => Some(text.clone()),
        Json::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// The profile: the id, the username, every attribute, and the roles (`roles`, `groups`, or Keycloak's realm roles).
fn profile(
    flow: &FlowConfig,
    attributes: serde_json::Map<String, Json>,
    provider: Option<&Provider>,
    id_token: Option<String>,
) -> SsoProfile {
    let id_key = provider.map_or("sub", |p| p.id);
    let id = text_of(attributes.get(id_key))
        .or_else(|| text_of(attributes.get("sub")))
        .or_else(|| text_of(attributes.get("id")))
        .unwrap_or_default();
    let username_keys: Vec<&str> = match (&flow.kind, provider) {
        (FlowKind::Oauth(_), Some(p)) => vec![p.username, "preferred_username", "username", "login", "email"],
        (FlowKind::Oauth(_), None) => vec!["preferred_username", "username", "login", "email"],
        (FlowKind::Oidc(_), _) => vec!["preferred_username", "email"],
    };
    // The email is a username only when `email_verified` says so, or, without that claim, when it is the known
    // provider's own username field.
    let email_verified = attributes
        .get("email_verified")
        .map(|value| matches!(value, Json::Bool(true)) || value.as_str() == Some("true"));
    let email_trusted = email_verified
        .unwrap_or_else(|| matches!((&flow.kind, provider), (FlowKind::Oauth(_), Some(p)) if p.username == "email"));
    let username = username_keys
        .iter()
        .filter(|key| **key != "email" || email_trusted)
        .find_map(|key| text_of(attributes.get(*key)));
    let mut roles = Vec::new();
    let mut add_roles = |value: Option<&Json>| {
        if let Some(Json::Array(items)) = value {
            roles.extend(items.iter().filter_map(|item| item.as_str().map(str::to_string)));
        }
    };
    add_roles(attributes.get("roles"));
    add_roles(attributes.get("groups"));
    add_roles(attributes.get("realm_access").and_then(|access| access.get("roles")));
    let json = Json::Object(attributes);
    let attributes = match moqui_lib::json::from_json(&json) {
        moqui_lib::object::Value::Map(map) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    SsoProfile {
        id,
        username,
        attributes,
        roles,
        id_token,
    }
}

impl SsoClient for HuskarlSso {
    fn start(&self, flow: &FlowConfig, redirect_uri: &str) -> Result<SsoStart, Error> {
        let this = Arc::clone(&self.inner);
        let (flow, redirect_uri) = (flow.clone(), redirect_uri.to_string());
        crate::edge::block_on(async move { this.start_async(flow, redirect_uri).await })?
    }

    fn complete(
        &self,
        flow: &FlowConfig,
        pending: &str,
        callback: &BTreeMap<String, String>,
    ) -> Result<SsoProfile, Error> {
        // The pending state names the redirect URI the grant was built with.
        let redirect_uri = serde_json::from_str::<Json>(pending)
            .ok()
            .and_then(|state| state.get("redirect_uri").and_then(Json::as_str).map(str::to_string))
            .unwrap_or_default();
        let this = Arc::clone(&self.inner);
        let (flow, pending, callback) = (flow.clone(), pending.to_string(), callback.clone());
        crate::edge::block_on(async move { this.complete_async(flow, redirect_uri, pending, callback).await })?
    }

    fn logout_url(
        &self,
        flow: &FlowConfig,
        id_token_hint: Option<&str>,
        post_logout_redirect_uri: &str,
    ) -> Result<Option<String>, Error> {
        let FlowKind::Oidc(config) = &flow.kind else {
            return Ok(None);
        };
        let built = lock(&self.inner.grants).get(&flow.auth_flow_id).cloned();
        let end_session = built.and_then(|built| built.metadata.clone()).and_then(|metadata| {
            metadata
                .end_session_endpoint
                .as_ref()
                .map(|url| url.as_uri().to_string())
        });
        let Some(end_session) = end_session else {
            return Ok(None);
        };
        let mut query = form_urlencoded::Serializer::new(String::new());
        if let Some(hint) = id_token_hint {
            query.append_pair("id_token_hint", hint);
        }
        query.append_pair("post_logout_redirect_uri", post_logout_redirect_uri);
        query.append_pair("client_id", &config.client_id);
        let separator = if end_session.contains('?') { '&' } else { '?' };
        Ok(Some(format!("{end_session}{separator}{}", query.finish())))
    }
}

#[cfg(test)]
mod tests {
    use moqui_core::sso::{FlowConfig, FlowKind, OauthConfig, OidcConfig};
    use serde_json::json;

    use super::{profile, provider};

    /// Admin P10 review: a flow that may not use the private network sends nothing to an internal address, written out
    /// or by name, before any connection.
    #[test]
    fn a_flow_denied_the_private_network_sends_nothing_to_an_internal_address() {
        let transport = super::ReqwestTransport::new(true).unwrap();
        for url in [
            "https://10.1.2.3/.well-known/openid-configuration",
            "https://localhost:9/x",
        ] {
            let request = http::Request::get(url).body(bytes::Bytes::new()).unwrap();
            let transport = transport.clone();
            let err = crate::edge::block_on(async move { transport.send(request).await })
                .unwrap()
                .unwrap_err();
            assert!(err.contains("internal address"), "{url}: {err}");
        }
    }

    fn username(kind: FlowKind, client_type: &str, claims: serde_json::Value) -> Option<String> {
        let flow = FlowConfig {
            auth_flow_id: "F".to_string(),
            kind,
            deny_internal: false,
        };
        let serde_json::Value::Object(claims) = claims else {
            unreachable!()
        };
        profile(&flow, claims, provider(client_type).as_ref(), None).username
    }

    #[test]
    fn an_email_is_a_username_only_when_verified_or_the_provider_names_it() {
        let oidc = || FlowKind::Oidc(OidcConfig::default());
        let claims =
            |verified: serde_json::Value| json!({"sub": "s1", "email": "a@example.com", "email_verified": verified});
        assert_eq!(
            username(oidc(), "", claims(json!(true))).as_deref(),
            Some("a@example.com")
        );
        assert_eq!(username(oidc(), "", claims(json!(false))), None);
        assert_eq!(
            username(oidc(), "", json!({"sub": "s1", "email": "a@example.com"})),
            None
        );
        assert_eq!(
            username(
                oidc(),
                "",
                json!({"sub": "s1", "preferred_username": "ann", "email": "a@example.com"})
            )
            .as_deref(),
            Some("ann")
        );
        let oauth = |client_type: &str| {
            FlowKind::Oauth(OauthConfig {
                client_type: client_type.to_string(),
                ..OauthConfig::default()
            })
        };
        // Facebook's username is its email, and it sends no email_verified.
        let facebook = json!({"id": "1", "email": "a@example.com"});
        assert_eq!(
            username(oauth("OctFacebook"), "OctFacebook", facebook).as_deref(),
            Some("a@example.com")
        );
        let unverified = json!({"id": "1", "email": "a@example.com", "email_verified": false});
        assert_eq!(username(oauth("OctGoogle"), "OctGoogle", unverified), None);
        assert_eq!(
            username(
                oauth("OctGitHub"),
                "OctGitHub",
                json!({"id": 1, "email": "a@example.com"})
            ),
            None
        );
    }
}
