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

//! The SSO relying party of the `moqui-sso` component (`plans/archive/Sso.md`): OpenID Connect and OAuth 2 authorization code
//! logins configured by `moqui.security.sso.AuthFlow` rows. Core has no HTTP or JOSE stack; the host installs an
//! [`SsoClient`] (`huskarl`) that builds the authorization URL, completes the code exchange and validates the ID token,
//! and reads the user info. The account sync, the group sync, and the login are core
//! (`crate::service::sso`). SAML flows are defined and report that SAML is not available.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::InstallSlot;

/// What a caller sees when SSO is used and the host installed no client.
pub const SSO_UNAVAILABLE: &str = "SSO is not available in this build";

/// One `OidcFlow` row. `Debug` leaves out the secret.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OidcConfig {
    /// `OctApple`, `OctAzureAd`, `OctGoogle`, `OctKeycloak`, or `OctOther`.
    pub client_type: String,
    pub client_id: String,
    pub secret: String,
    pub discovery_uri: String,
    /// Keycloak: with `base_uri`, the discovery URI when `discovery_uri` is empty.
    pub realm: String,
    pub base_uri: String,
    /// The `optionValue` of `preferredJwsAlgorithmEnumId`, such as `RS256`.
    pub preferred_jws_algorithm: String,
    pub use_nonce: bool,
    /// Apple: the client secret is a JWT signed with the private key in `secret`.
    pub apple_team_id: String,
    pub apple_key_id: String,
}

/// One `OauthFlow` row. `Debug` leaves out the secret.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OauthConfig {
    /// `OctGitHub`, `OctGoogle`, ..., or `OctOauth20` with the URLs set.
    pub client_type: String,
    pub client_id: String,
    pub secret: String,
    pub authorize_url: String,
    pub token_url: String,
    pub user_info_url: String,
    pub scope: String,
}

impl fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcConfig")
            .field("client_type", &self.client_type)
            .field("client_id", &self.client_id)
            .field("secret", &redacted(&self.secret))
            .field("discovery_uri", &self.discovery_uri)
            .field("realm", &self.realm)
            .field("base_uri", &self.base_uri)
            .field("preferred_jws_algorithm", &self.preferred_jws_algorithm)
            .field("use_nonce", &self.use_nonce)
            .field("apple_team_id", &self.apple_team_id)
            .field("apple_key_id", &self.apple_key_id)
            .finish()
    }
}

impl fmt::Debug for OauthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OauthConfig")
            .field("client_type", &self.client_type)
            .field("client_id", &self.client_id)
            .field("secret", &redacted(&self.secret))
            .field("authorize_url", &self.authorize_url)
            .field("token_url", &self.token_url)
            .field("user_info_url", &self.user_info_url)
            .field("scope", &self.scope)
            .finish()
    }
}

/// What `Debug` shows for a secret: whether one is set, never its value.
fn redacted(secret: &str) -> &'static str {
    if secret.is_empty() {
        ""
    } else {
        "<redacted>"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowKind {
    Oidc(OidcConfig),
    Oauth(OauthConfig),
}

/// An `AuthFlow` with its protocol row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowConfig {
    pub auth_flow_id: String,
    pub kind: FlowKind,
    /// `AuthFlow.allowPrivateNetwork` is `N`: no request of the flow (discovery, token, keys, user info) goes to a
    /// loopback, private or link-local address.
    pub deny_internal: bool,
}

/// The redirect to the identity provider, and the state to keep in the session until the callback.
#[derive(Clone, Debug)]
pub struct SsoStart {
    pub authorize_url: String,
    /// Opaque to core: the host's pending state (PKCE verifier, state, nonce), serialized.
    pub pending: String,
}

/// Who the identity provider says logged in.
#[derive(Clone, Debug, Default)]
pub struct SsoProfile {
    /// The provider's subject (`sub`, or the user info `id`): `UserAccount.externalUserId`.
    pub id: String,
    /// The Moqui username, when the provider gave one.
    pub username: Option<String>,
    /// Every claim of the ID token and the user info, for `AuthFlowFieldMap`.
    pub attributes: BTreeMap<String, Value>,
    /// The provider's roles or groups, for `AuthFlowRoleMap`.
    pub roles: Vec<String>,
    /// The raw ID token, for the `id_token_hint` of a logout at the provider.
    pub id_token: Option<String>,
}

/// The protocol work, installed by the host.
pub trait SsoClient: Send + Sync {
    /// Build the authorization request.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery or the request fails.
    fn start(&self, flow: &FlowConfig, redirect_uri: &str) -> Result<SsoStart, Error>;

    /// Complete the callback: check the state, exchange the code, validate the ID token, read the user info.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider refused, the state does not match, or a token does not validate.
    fn complete(
        &self,
        flow: &FlowConfig,
        pending: &str,
        callback: &BTreeMap<String, String>,
    ) -> Result<SsoProfile, Error>;

    /// The provider's end-session URL, when it has one.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery fails.
    fn logout_url(
        &self,
        flow: &FlowConfig,
        id_token_hint: Option<&str>,
        post_logout_redirect_uri: &str,
    ) -> Result<Option<String>, Error>;
}

static CLIENT: InstallSlot<dyn SsoClient> = InstallSlot::new();

/// Use `client` for every SSO flow in this process.
pub fn install_sso_client(client: Arc<dyn SsoClient>) {
    CLIENT.install(client);
}

/// Remove the installed client.
pub fn clear_sso_client() {
    CLIENT.clear();
}

pub(crate) fn client() -> Result<Arc<dyn SsoClient>, Error> {
    CLIENT.get().ok_or_else(|| Error::new(SSO_UNAVAILABLE))
}

#[cfg(test)]
mod tests {
    use super::{OauthConfig, OidcConfig};

    #[test]
    fn debug_leaves_out_the_client_secret() {
        let oidc = OidcConfig {
            client_id: "app".into(),
            secret: "oidc-secret-value".into(),
            ..OidcConfig::default()
        };
        let oauth = OauthConfig {
            client_id: "app".into(),
            secret: "oauth-secret-value".into(),
            ..OauthConfig::default()
        };
        let shown = format!("{oidc:?} {oauth:?}");
        assert!(shown.contains("client_id: \"app\"") && shown.contains("<redacted>"));
        assert!(!shown.contains("secret-value"));
    }
}
