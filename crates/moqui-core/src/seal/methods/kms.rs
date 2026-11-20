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

//! The `kms` unseal method (plans/Sealed-Secrets.md §3.3, S11): a cloud KMS key wraps the root key and unwraps it at
//! each start, with no SDK, over [`moqui_lib::http::RestClient`].
//!
//! - **AWS KMS**: `TrentService.Encrypt` and `Decrypt`, signed with SigV4 ([`moqui_lib::sigv4`]), with the encryption
//!   context `moqui-seal=root-key`; credentials from a root-owned file or the instance role (IMDSv2).
//! - **GCP Cloud KMS**: `cryptoKeys:encrypt` and `:decrypt` with the additional authenticated data `moqui-seal
//!   root-key`; a token from a service account's key file (the JWT bearer grant, [`crate::google_auth`]) or the host's
//!   metadata server.
//! - **Azure Key Vault**: `wrapkey` and `unwrapkey` with RSA-OAEP-256; a token by the client credentials grant from a
//!   root-owned file, or the host's managed identity (IMDS).
//!
//! What decides where the root key goes (the provider, the key, the region, the endpoints) is read from the conf files
//! only ([`super::CloudConf`]); `seal.json` keeps only the wrapped key, so a writer of it cannot send the server's
//! credentials elsewhere. A credential is never a conf literal: `seal@kms-credentials` names a file or a systemd
//! credential.

use std::time::Duration;

use base64::Engine as _;
use moqui_lib::http::{Method as HttpMethod, RestClient};
use moqui_lib::sigv4;
use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use super::{CloudConf, Credentials, KmsProvider, Method, MethodKind};
use crate::context::Error;
use crate::seal::{RootKey, SealConf};

/// What binds a wrapped root key to this use, where the provider takes it (AWS's encryption context, GCP's additional
/// authenticated data).
const CONTEXT: &str = "moqui-seal root-key";
/// The Azure Key Vault API version.
const AZURE_API: &str = "7.4";
const GCP_SCOPE: &str = "https://www.googleapis.com/auth/cloudkms";

/// The `kms` method.
pub struct Kms;

impl Method for Kms {
    fn kind(&self) -> MethodKind {
        MethodKind::Kms
    }

    fn wrap(&self, root: &RootKey, conf: &SealConf) -> Result<Map<String, Value>, Error> {
        let cloud = &conf.cloud;
        let provider = cloud.kms_provider()?;
        let key = cloud.kms_key()?;
        let mut metadata = Map::new();
        metadata.insert("provider".to_string(), json!(provider.name()));
        metadata.insert("key".to_string(), json!(key));
        let wrapped = match provider {
            KmsProvider::Aws => {
                metadata.insert("region".to_string(), json!(cloud.kms_region));
                let answer = aws(
                    cloud,
                    "TrentService.Encrypt",
                    &json!({
                        "KeyId": key,
                        "Plaintext": standard(root.expose()),
                        "EncryptionContext": {"moqui-seal": "root-key"},
                    }),
                )?;
                text_field(&answer, "CiphertextBlob")?
            }
            KmsProvider::Gcp => {
                let answer = gcp(
                    cloud,
                    "encrypt",
                    &json!({"plaintext": standard(root.expose()), "additionalAuthenticatedData": standard(CONTEXT.as_bytes())}),
                )?;
                text_field(&answer, "ciphertext")?
            }
            KmsProvider::Azure => {
                let answer = azure(cloud, key, "wrapkey", root.expose())?;
                if let Some(kid) = answer["kid"].as_str() {
                    metadata.insert("kid".to_string(), json!(kid));
                }
                text_field(&answer, "value")?
            }
        };
        metadata.insert("wrapped".to_string(), json!(wrapped));
        Ok(metadata)
    }

    fn unwrap(&self, metadata: &Map<String, Value>, conf: &SealConf) -> Result<RootKey, Error> {
        let cloud = &conf.cloud;
        let provider = cloud.kms_provider()?;
        let made_with = metadata.get("provider").and_then(Value::as_str).unwrap_or_default();
        if made_with != provider.name() {
            return Err(Error::new(format!(
                "seal.json's key was wrapped by {made_with:?}, and seal@kms-provider is {:?}",
                provider.name()
            )));
        }
        let key = cloud.kms_key()?;
        let wrapped = metadata
            .get("wrapped")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("seal.json's kms method has no wrapped key"))?;
        let opened = match provider {
            KmsProvider::Aws => {
                let answer = aws(
                    cloud,
                    "TrentService.Decrypt",
                    &json!({
                        "KeyId": key,
                        "CiphertextBlob": wrapped,
                        "EncryptionContext": {"moqui-seal": "root-key"},
                    }),
                )?;
                decode_standard(&text_field(&answer, "Plaintext")?)?
            }
            KmsProvider::Gcp => {
                let answer = gcp(
                    cloud,
                    "decrypt",
                    &json!({"ciphertext": wrapped, "additionalAuthenticatedData": standard(CONTEXT.as_bytes())}),
                )?;
                decode_standard(&text_field(&answer, "plaintext")?)?
            }
            KmsProvider::Azure => {
                // the version the key was wrapped under, only when it is a version of the conf's own key
                let kid = metadata.get("kid").and_then(Value::as_str).unwrap_or_default();
                let url = if kid.starts_with(&format!("{}/", key.trim_end_matches('/'))) {
                    kid
                } else {
                    key
                };
                let blob = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(wrapped.trim_end_matches('='))
                    .map_err(|err| Error::new(format!("seal.json's wrapped key is not base64url: {err}")))?;
                let answer = azure(cloud, url, "unwrapkey", &blob)?;
                Zeroizing::new(
                    base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(text_field(&answer, "value")?.trim_end_matches('='))
                        .map_err(|err| Error::new(format!("Azure's unwrapped key is not base64url: {err}")))?,
                )
            }
        };
        RootKey::from_slice(&opened)
    }
}

fn standard(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_standard(text: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map(Zeroizing::new)
        .map_err(|err| Error::new(format!("the KMS answer is not base64: {err}")))
}

fn text_field(answer: &Value, name: &str) -> Result<String, Error> {
    answer[name]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::new(format!("the KMS answer has no {name}")))
}

/// The client of every call: no redirect to another host, and a bounded wait.
pub(super) fn client() -> RestClient {
    RestClient::new()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirects(0, false)
}

/// An answer's JSON, or the call's error with the status (never the body of a refusal, which may echo the request).
pub(super) fn answer_json(what: &str, response: &moqui_lib::http::RestResponse) -> Result<Value, Error> {
    if !(200..300).contains(&response.status()) {
        return Err(Error::new(format!("{what} answered {}", response.status())));
    }
    serde_json::from_str(response.body()).map_err(|err| Error::new(format!("{what} answered no JSON: {err}")))
}

fn send(what: &str, call: moqui_lib::http::Call<'_>) -> Result<Value, Error> {
    let response = call.send(false).map_err(|err| Error::new(format!("{what}: {err}")))?;
    answer_json(what, &response)
}

/// AWS credentials: an access key, its secret and, for a role's, the session token.
struct AwsKeys {
    access_key: String,
    secret_key: Zeroizing<String>,
    session_token: Option<Zeroizing<String>>,
}

fn aws_keys(cloud: &CloudConf) -> Result<AwsKeys, Error> {
    let value = match &cloud.kms_credentials {
        Credentials::File(_) => {
            let text = cloud.kms_credentials.read("seal@kms-credentials")?;
            let value: Value = serde_json::from_str(&text)
                .map_err(|_| Error::new("seal@kms-credentials: the AWS credentials file is not JSON"))?;
            let field = |names: &[&str]| {
                names
                    .iter()
                    .find_map(|name| value[*name].as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            json!({
                "AccessKeyId": field(&["access_key_id", "AccessKeyId"]),
                "SecretAccessKey": field(&["secret_access_key", "SecretAccessKey"]),
                "Token": field(&["session_token", "SessionToken", "Token"]),
            })
        }
        Credentials::Metadata => {
            // IMDSv2: a session token first, then the instance role's credentials
            let base = cloud.token_base("http://169.254.169.254");
            let http = client();
            let token = http
                .request(HttpMethod::Put, format!("{base}/latest/api/token"))
                .header("X-aws-ec2-metadata-token-ttl-seconds", "300")
                .send(false)
                .map_err(|err| Error::new(format!("the instance metadata token: {err}")))?;
            if token.status() != 200 {
                return Err(Error::new(format!(
                    "the instance metadata token answered {}",
                    token.status()
                )));
            }
            let token = Zeroizing::new(token.body().trim().to_string());
            let roles = http
                .get(format!("{base}/latest/meta-data/iam/security-credentials/"))
                .header("X-aws-ec2-metadata-token", token.as_str())
                .send(false)
                .map_err(|err| Error::new(format!("the instance role: {err}")))?;
            let role = roles.body().lines().next().unwrap_or_default().trim().to_string();
            if roles.status() != 200 || role.is_empty() {
                return Err(Error::new("this instance has no role to sign the KMS call with"));
            }
            send(
                "the instance role's credentials",
                http.get(format!("{base}/latest/meta-data/iam/security-credentials/{role}"))
                    .header("X-aws-ec2-metadata-token", token.as_str()),
            )?
        }
    };
    let access_key = value["AccessKeyId"].as_str().unwrap_or_default().to_string();
    let secret_key = Zeroizing::new(value["SecretAccessKey"].as_str().unwrap_or_default().to_string());
    if access_key.is_empty() || secret_key.is_empty() {
        return Err(Error::new("the AWS credentials have no access key id or secret"));
    }
    let session_token = value["Token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .map(|token| Zeroizing::new(token.to_string()));
    Ok(AwsKeys {
        access_key,
        secret_key,
        session_token,
    })
}

/// One AWS KMS call, `target` its `X-Amz-Target`, signed with SigV4.
fn aws(cloud: &CloudConf, target: &str, body: &Value) -> Result<Value, Error> {
    if cloud.kms_region.is_empty() {
        return Err(Error::new("seal@kms-region is not set (the AWS region of the key)"));
    }
    let endpoint = if cloud.kms_endpoint.is_empty() {
        format!("https://kms.{}.amazonaws.com", cloud.kms_region)
    } else {
        cloud.kms_endpoint.trim_end_matches('/').to_string()
    };
    let host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let keys = aws_keys(cloud)?;
    let body = Zeroizing::new(body.to_string());
    let amz_date = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ").to_string();
    let mut headers = vec![
        ("content-type".to_string(), "application/x-amz-json-1.1".to_string()),
        ("host".to_string(), host),
        ("x-amz-date".to_string(), amz_date.clone()),
        ("x-amz-target".to_string(), target.to_string()),
    ];
    if let Some(token) = &keys.session_token {
        headers.push(("x-amz-security-token".to_string(), token.to_string()));
    }
    let payload_hash = sigv4::sha256_hex(body.as_bytes());
    let authorization = sigv4::authorization(
        &sigv4::Request {
            method: "POST",
            path: "/",
            query: "",
            headers: &headers,
            payload_hash: &payload_hash,
        },
        &sigv4::Credentials {
            access_key: &keys.access_key,
            secret_key: &keys.secret_key,
        },
        sigv4::Scope {
            region: &cloud.kms_region,
            service: "kms",
        },
        &amz_date,
    );
    let http = client();
    let mut call = http.post(format!("{endpoint}/"));
    for (name, value) in headers
        .iter()
        .filter(|(name, _)| name != "host" && name != "content-type")
    {
        call = call.header(name.as_str(), value.as_str());
    }
    call = call
        .header("authorization", authorization)
        .text("application/x-amz-json-1.1", body.as_str());
    send(&format!("AWS KMS {target}"), call)
}

/// A Google access token for Cloud KMS: from the service account's key file, else the host's metadata server.
fn gcp_token(cloud: &CloudConf) -> Result<Zeroizing<String>, Error> {
    let http = client();
    let answer = match &cloud.kms_credentials {
        Credentials::File(_) => {
            let text = cloud.kms_credentials.read("seal@kms-credentials")?;
            let account = crate::google_auth::ServiceAccount::parse(&text)
                .map_err(|err| Error::new(format!("seal@kms-credentials: {err}")))?;
            let now = jiff::Timestamp::now().as_second();
            let jwt = crate::google_auth::jwt(&account, GCP_SCOPE, now, 3600).map_err(Error::new)?;
            http.post(account.token_uri.clone())
                .text(
                    "application/x-www-form-urlencoded",
                    crate::google_auth::token_form(&jwt),
                )
                .send(false)
        }
        Credentials::Metadata => http
            .get(format!(
                "{}{}",
                cloud.token_base(crate::google_auth::METADATA_BASE),
                crate::google_auth::METADATA_TOKEN_PATH
            ))
            .header("Metadata-Flavor", "Google")
            .send(false),
    }
    .map_err(|err| Error::new(format!("the Google token: {err}")))?;
    if answer.status() != 200 {
        return Err(Error::new(format!("the Google token answered {}", answer.status())));
    }
    crate::google_auth::access_token(answer.body())
        .map(|(token, _)| Zeroizing::new(token))
        .map_err(Error::new)
}

/// One Cloud KMS call on the conf's key: `encrypt` or `decrypt`.
fn gcp(cloud: &CloudConf, verb: &str, body: &Value) -> Result<Value, Error> {
    let token = gcp_token(cloud)?;
    let endpoint = if cloud.kms_endpoint.is_empty() {
        "https://cloudkms.googleapis.com"
    } else {
        cloud.kms_endpoint.trim_end_matches('/')
    };
    let key = cloud.kms_key()?;
    let http = client();
    send(
        &format!("Cloud KMS {verb}"),
        http.post(format!("{endpoint}/v1/{key}:{verb}"))
            .header("authorization", format!("Bearer {}", token.as_str()))
            .json(body),
    )
}

/// An Azure access token for Key Vault: the client credentials grant from a root-owned file (`tenant_id`, `client_id`,
/// `client_secret`), else the host's managed identity.
fn azure_token(cloud: &CloudConf) -> Result<Zeroizing<String>, Error> {
    let http = client();
    let answer = match &cloud.kms_credentials {
        Credentials::File(_) => {
            let text = cloud.kms_credentials.read("seal@kms-credentials")?;
            let value: Value = serde_json::from_str(&text)
                .map_err(|_| Error::new("seal@kms-credentials: the Azure credentials file is not JSON"))?;
            let field = |name: &str| value[name].as_str().unwrap_or_default().to_string();
            let (tenant, client_id) = (field("tenant_id"), field("client_id"));
            let secret = Zeroizing::new(field("client_secret"));
            if tenant.is_empty() || client_id.is_empty() || secret.is_empty() {
                return Err(Error::new(
                    "seal@kms-credentials: the Azure credentials need tenant_id, client_id and client_secret",
                ));
            }
            let form = Zeroizing::new(format!(
                "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
                sigv4::uri_encode(&client_id, true),
                sigv4::uri_encode(&secret, true),
                sigv4::uri_encode("https://vault.azure.net/.default", true)
            ));
            http.post(format!(
                "{}/{}/oauth2/v2.0/token",
                cloud.token_base("https://login.microsoftonline.com"),
                sigv4::uri_encode(&tenant, true)
            ))
            .text("application/x-www-form-urlencoded", form.as_str())
            .send(false)
        }
        Credentials::Metadata => http
            .get(format!(
                "{}/metadata/identity/oauth2/token?api-version=2018-02-01&resource={}",
                cloud.token_base("http://169.254.169.254"),
                sigv4::uri_encode("https://vault.azure.net", true)
            ))
            .header("Metadata", "true")
            .send(false),
    }
    .map_err(|err| Error::new(format!("the Azure token: {err}")))?;
    if answer.status() != 200 {
        return Err(Error::new(format!("the Azure token answered {}", answer.status())));
    }
    crate::google_auth::access_token(answer.body())
        .map(|(token, _)| Zeroizing::new(token))
        .map_err(Error::new)
}

/// `wrapkey` or `unwrapkey` of `value` on the key at `url` (the conf's key, or a version of it).
fn azure(cloud: &CloudConf, url: &str, verb: &str, value: &[u8]) -> Result<Value, Error> {
    let token = azure_token(cloud)?;
    let http = client();
    let body = json!({
        "alg": "RSA-OAEP-256",
        "value": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value),
    });
    send(
        &format!("Azure Key Vault {verb}"),
        http.post(format!("{}/{verb}?api-version={AZURE_API}", url.trim_end_matches('/')))
            .header("authorization", format!("Bearer {}", token.as_str()))
            .json(&body),
    )
}
