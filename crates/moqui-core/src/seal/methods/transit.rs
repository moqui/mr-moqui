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

//! The `transit` unseal method (plans/Sealed-Secrets.md §3.3, S11): an OpenBao or HashiCorp Vault Transit key wraps
//! the root key (`POST /v1/<mount>/encrypt/<key>`) and unwraps it at each start (`/decrypt/<key>`), with the token in
//! `X-Vault-Token`. The Vault's address, mount, key and namespace come from the conf files only, the token from a file
//! or a systemd credential `seal@transit-token` names ([`super::CloudConf`]); `seal.json` keeps only the ciphertext and
//! `backend: openbao`.
//!
//! A managed instance's manager (Vault-Instances M6) supplies a transit of its own over the manager link: the host
//! installs it with [`super::register`], which takes this method's place, and it writes its own `backend` so each tells
//! a seal of the other's.

use moqui_lib::http::RestClient;
use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use super::{CloudConf, Method, MethodKind};
use crate::context::Error;
use crate::seal::{RootKey, SealConf};

/// What this method writes as `backend`, so a manager's transit tells it from its own.
pub const BACKEND: &str = "openbao";

/// The `transit` method of an OpenBao or Vault server.
pub struct Transit;

impl Method for Transit {
    fn kind(&self) -> MethodKind {
        MethodKind::Transit
    }

    fn wrap(&self, root: &RootKey, conf: &SealConf) -> Result<Map<String, Value>, Error> {
        use base64::Engine as _;
        let plaintext = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(root.expose()));
        let answer = call(&conf.cloud, "encrypt", &json!({"plaintext": plaintext.as_str()}))?;
        let ciphertext = answer["data"]["ciphertext"]
            .as_str()
            .filter(|text| text.starts_with("vault:"))
            .ok_or_else(|| Error::new("the transit encrypt answer has no ciphertext"))?;
        let mut metadata = Map::new();
        metadata.insert("backend".to_string(), json!(BACKEND));
        metadata.insert("key".to_string(), json!(conf.cloud.transit_key));
        metadata.insert("ciphertext".to_string(), json!(ciphertext));
        Ok(metadata)
    }

    fn unwrap(&self, metadata: &Map<String, Value>, conf: &SealConf) -> Result<RootKey, Error> {
        use base64::Engine as _;
        let backend = metadata.get("backend").and_then(Value::as_str).unwrap_or(BACKEND);
        if backend != BACKEND {
            return Err(Error::new(format!(
                "seal.json's transit is {backend:?}, which this server's transit does not open (a manager's link \
                 installs its own)"
            )));
        }
        let ciphertext = metadata
            .get("ciphertext")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("seal.json's transit method has no ciphertext"))?;
        let answer = call(&conf.cloud, "decrypt", &json!({"ciphertext": ciphertext}))?;
        let plaintext = answer["data"]["plaintext"]
            .as_str()
            .ok_or_else(|| Error::new("the transit decrypt answer has no plaintext"))?;
        let opened = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(plaintext.trim())
                .map_err(|err| Error::new(format!("the transit plaintext is not base64: {err}")))?,
        );
        RootKey::from_slice(&opened)
    }
}

/// One Transit call on the conf's key: `encrypt` or `decrypt`.
fn call(cloud: &CloudConf, verb: &str, body: &Value) -> Result<Value, Error> {
    if cloud.transit_url.is_empty() || cloud.transit_key.is_empty() {
        return Err(Error::new(
            "seal@transit-url and seal@transit-key name the Vault and its Transit key",
        ));
    }
    let token = cloud.transit_token.read("seal@transit-token")?;
    let mut http: RestClient = super::kms::client();
    if !cloud.transit_ca_file.is_empty() {
        let pem = std::fs::read(&cloud.transit_ca_file)
            .map_err(|err| Error::new(format!("seal@transit-ca-file {}: {err}", cloud.transit_ca_file)))?;
        http = http.root_certificates_pem(pem);
    }
    let url = format!(
        "{}/v1/{}/{verb}/{}",
        cloud.transit_url.trim_end_matches('/'),
        cloud.transit_mount.trim_matches('/'),
        moqui_lib::sigv4::uri_encode(&cloud.transit_key, true)
    );
    let mut request = http.post(url).header("X-Vault-Token", token.as_str());
    if !cloud.transit_namespace.is_empty() {
        request = request.header("X-Vault-Namespace", cloud.transit_namespace.as_str());
    }
    let response = request
        .json(body)
        .send(false)
        .map_err(|err| Error::new(format!("transit {verb}: {err}")))?;
    super::kms::answer_json(&format!("transit {verb}"), &response)
}
