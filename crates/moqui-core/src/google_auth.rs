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

//! A Google service account's sign-in (the OAuth 2 JWT bearer grant, RFC 7523) and the metadata server's token of a
//! Google Cloud host, shared by FCM push (`service::fcm`) and the seal's GCP Cloud KMS method
//! (`seal::methods::kms`, plans/Sealed-Secrets.md S11). Making the request is the caller's: each sends it its own way.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// The token endpoint of a key file that names none.
pub const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
/// `urn:ietf:params:oauth:grant-type:jwt-bearer`, form-encoded.
pub const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer";
/// The metadata server of a Google Cloud host.
pub const METADATA_BASE: &str = "http://metadata.google.internal";
/// The metadata server's path of the default service account's access token; asked with `Metadata-Flavor: Google`.
pub const METADATA_TOKEN_PATH: &str = "/computeMetadata/v1/instance/service-accounts/default/token";

/// The fields of a service account's JSON key file a sign-in needs. Its `Debug` leaves out the private key.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceAccount {
    pub client_email: String,
    pub private_key: String,
    pub token_uri: String,
    pub project_id: String,
}

impl std::fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("client_email", &self.client_email)
            .field("token_uri", &self.token_uri)
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl ServiceAccount {
    /// From the JSON key file Google gives for a service account.
    ///
    /// # Errors
    ///
    /// Text that is not JSON, or a key file with no `client_email` or `private_key`.
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|err| format!("the service account is not JSON: {err}"))?;
        let field = |name: &str| value[name].as_str().unwrap_or_default().trim().to_string();
        let account = Self {
            client_email: field("client_email"),
            private_key: field("private_key"),
            token_uri: Some(field("token_uri"))
                .filter(|uri| !uri.is_empty())
                .unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string()),
            project_id: field("project_id"),
        };
        if account.client_email.is_empty() || account.private_key.is_empty() {
            return Err("the service account has no client_email or private_key".to_string());
        }
        Ok(account)
    }
}

/// The signed RS256 JWT of the JWT bearer grant for `account`, asking for `scope`, at `now` (Unix seconds), good for
/// `seconds`.
///
/// # Errors
///
/// The key file's private key does not sign.
pub fn jwt(account: &ServiceAccount, scope: &str, now: i64, seconds: i64) -> Result<String, String> {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iss": account.client_email,
        "scope": scope,
        "aud": account.token_uri,
        "iat": now,
        "exp": now + seconds,
    });
    let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signing_input = format!("{header}.{claims}");
    // the key file's private_key is PKCS#8 PEM with its line breaks as \n, which the JSON parse turned into newlines
    let signer = moqui_lib::signer::SimpleSigner::from_pem_with(
        "the service account",
        &account.private_key,
        None,
        Some("SHA256withRSA"),
    );
    let signature = signer.sign(&signing_input).map_err(|err| err.message().to_string())?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|err| format!("the signature is not base64: {err}"))?;
    Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

/// The form body that exchanges `jwt` for an access token at the account's `token_uri`.
#[must_use]
pub fn token_form(jwt: &str) -> String {
    format!("grant_type={GRANT_TYPE}&assertion={jwt}")
}

/// The access token of a token endpoint's or the metadata server's JSON answer, and how many seconds it is good for.
///
/// # Errors
///
/// An answer with no `access_token`.
pub fn access_token(answer: &str) -> Result<(String, i64), String> {
    let value: serde_json::Value =
        serde_json::from_str(answer).map_err(|err| format!("the token answer is not JSON: {err}"))?;
    let token = value["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or("the token answer has no access_token")?;
    Ok((token.to_string(), value["expires_in"].as_i64().unwrap_or(3600)))
}
