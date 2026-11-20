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

//! The `kms` (AWS, GCP, Azure) and `transit` methods of S11 against a stub HTTP server in the test: each wraps a root
//! key and unwraps it again, the requests' shape and AWS's SigV4 signature are checked, a credential is never a conf
//! literal, and a refused automatic unseal leaves the server sealed with `auto-unseal failed`.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Value};

use super::kms::Kms;
use super::transit::Transit;
use super::{CloudConf, Credentials, Method};
use crate::seal::{file, Keeper, Mode, RootKey, SealConf, Start};

/// One request the stub got.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map_or("", String::as_str)
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

type Handler = dyn Fn(&Seen) -> (u16, String) + Send + Sync;

/// A stub HTTP server on a loopback port, answering each request with `handler` and keeping what it got.
struct Stub {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    fn start(handler: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let handler: Arc<Handler> = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = HashMap::new();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                let length: usize = headers.get("content-length").and_then(|n| n.parse().ok()).unwrap_or(0);
                let mut body = vec![0_u8; length];
                reader.read_exact(&mut body).unwrap();
                let request = Seen {
                    method,
                    path,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                let (status, answer) = handler(&request);
                moqui_lib::sync::lock(&kept).push(request);
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Self { base, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        moqui_lib::sync::lock(&self.seen).clone()
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "moqui-cloud-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A credential file, its owner's alone.
    fn file(&self, name: &str, text: &str) -> Credentials {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        Credentials::File(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn conf(cloud: CloudConf) -> SealConf {
    SealConf {
        cloud,
        ..SealConf::with_purpose("production")
    }
}

/// A stand-in for a KMS's sealing: base64 of the bytes reversed, behind a tag.
fn fake_seal(plain: &[u8]) -> String {
    let reversed: Vec<u8> = plain.iter().rev().copied().collect();
    format!("sealed:{}", base64::engine::general_purpose::STANDARD.encode(reversed))
}

fn fake_open(sealed: &str) -> Vec<u8> {
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(sealed.strip_prefix("sealed:").unwrap())
        .unwrap();
    bytes.reverse();
    bytes
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(text).unwrap()
}

/// Wrap a new root key with `method` and unwrap it again.
fn round_trip(method: &dyn Method, conf: &SealConf) -> serde_json::Map<String, Value> {
    let root = RootKey::random().unwrap();
    let metadata = method.wrap(&root, conf).unwrap();
    let opened = method.unwrap(&metadata, conf).unwrap();
    assert_eq!(opened.expose(), root.expose());
    metadata
}

/// The SigV4 signature the stub expects of a request, by its signed headers and AWS's documented algorithm.
fn expected_authorization(seen: &Seen, secret: &str, region: &str) -> String {
    let authorization = seen.header("authorization");
    let signed = authorization
        .split("SignedHeaders=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap();
    let headers: Vec<(String, String)> = signed
        .split(';')
        .map(|name| (name.to_string(), seen.header(name).to_string()))
        .collect();
    let access = authorization
        .split("Credential=")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap();
    moqui_lib::sigv4::authorization(
        &moqui_lib::sigv4::Request {
            method: &seen.method,
            path: &seen.path,
            query: "",
            headers: &headers,
            payload_hash: &moqui_lib::sigv4::sha256_hex(seen.body.as_bytes()),
        },
        &moqui_lib::sigv4::Credentials {
            access_key: access,
            secret_key: secret,
        },
        moqui_lib::sigv4::Scope { region, service: "kms" },
        seen.header("x-amz-date"),
    )
}

/// An AWS KMS stub: Encrypt and Decrypt with the encryption context, and IMDSv2's token, role and credentials.
fn aws_stub() -> Stub {
    Stub::start(|seen| match (seen.method.as_str(), seen.path.as_str()) {
        ("PUT", "/latest/api/token") if !seen.header("x-aws-ec2-metadata-token-ttl-seconds").is_empty() => {
            (200, "imds-session".to_string())
        }
        ("GET", "/latest/meta-data/iam/security-credentials/")
            if seen.header("x-aws-ec2-metadata-token") == "imds-session" =>
        {
            (200, "moqui-role".to_string())
        }
        ("GET", "/latest/meta-data/iam/security-credentials/moqui-role")
            if seen.header("x-aws-ec2-metadata-token") == "imds-session" =>
        {
            (
                200,
                json!({"AccessKeyId": "ASIAROLE", "SecretAccessKey": "role-secret", "Token": "role-session"})
                    .to_string(),
            )
        }
        ("POST", "/") => {
            let body = seen.json();
            if body["EncryptionContext"]["moqui-seal"] != "root-key" {
                return (400, "{}".to_string());
            }
            match seen.header("x-amz-target") {
                "TrentService.Encrypt" => (
                    200,
                    json!({"CiphertextBlob": b64(fake_seal(&unb64(body["Plaintext"].as_str().unwrap())).as_bytes())})
                        .to_string(),
                ),
                "TrentService.Decrypt" => {
                    let sealed = String::from_utf8(unb64(body["CiphertextBlob"].as_str().unwrap())).unwrap();
                    (200, json!({"Plaintext": b64(&fake_open(&sealed))}).to_string())
                }
                _ => (400, "{}".to_string()),
            }
        }
        _ => (404, "{}".to_string()),
    })
}

#[test]
fn aws_kms_wraps_and_unwraps_with_a_sigv4_signed_request() {
    let scratch = Scratch::new();
    let stub = aws_stub();
    let conf = conf(CloudConf {
        kms_provider: "aws".into(),
        kms_key: "arn:aws:kms:us-east-1:111122223333:key/seal".into(),
        kms_region: "us-east-1".into(),
        kms_endpoint: stub.base.clone(),
        kms_credentials: scratch.file(
            "aws.json",
            r#"{"access_key_id": "AKIDSEAL", "secret_access_key": "the-aws-secret"}"#,
        ),
        ..CloudConf::default()
    });
    let metadata = round_trip(&Kms, &conf);
    assert_eq!(metadata["provider"], "aws");
    let seen = stub.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert_eq!(request.header("content-type"), "application/x-amz-json-1.1");
        assert_eq!(request.json()["KeyId"], "arn:aws:kms:us-east-1:111122223333:key/seal");
        assert!(request
            .header("authorization")
            .starts_with("AWS4-HMAC-SHA256 Credential=AKIDSEAL/"));
        assert!(request.header("authorization").contains("/us-east-1/kms/aws4_request"));
        assert_eq!(
            request.header("authorization"),
            expected_authorization(request, "the-aws-secret", "us-east-1"),
            "the signature is AWS's over the request as sent"
        );
        assert!(!request.body.contains("the-aws-secret"));
    }
    assert_eq!(seen[0].header("x-amz-target"), "TrentService.Encrypt");
    assert_eq!(seen[1].header("x-amz-target"), "TrentService.Decrypt");
}

#[test]
fn aws_kms_signs_with_the_instance_roles_session_from_imdsv2() {
    let stub = aws_stub();
    let conf = conf(CloudConf {
        kms_provider: "aws".into(),
        kms_key: "alias/moqui-seal".into(),
        kms_region: "eu-west-1".into(),
        kms_endpoint: stub.base.clone(),
        kms_token_endpoint: stub.base.clone(),
        ..CloudConf::default()
    });
    round_trip(&Kms, &conf);
    let kms: Vec<Seen> = stub.seen().into_iter().filter(|seen| seen.path == "/").collect();
    assert_eq!(kms.len(), 2);
    for request in &kms {
        assert_eq!(request.header("x-amz-security-token"), "role-session");
        assert!(request.header("authorization").contains("x-amz-security-token"));
        assert_eq!(
            request.header("authorization"),
            expected_authorization(request, "role-secret", "eu-west-1")
        );
    }
}

const GCP_KEY: &str = "projects/p/locations/global/keyRings/r/cryptoKeys/seal";

fn gcp_stub() -> Stub {
    Stub::start(|seen| {
        let aad = b64(b"moqui-seal root-key");
        match (seen.method.as_str(), seen.path.as_str()) {
            ("GET", "/computeMetadata/v1/instance/service-accounts/default/token")
                if seen.header("metadata-flavor") == "Google" =>
            {
                (
                    200,
                    json!({"access_token": "gcp-metadata-token", "expires_in": 3599}).to_string(),
                )
            }
            ("POST", "/token")
                if seen
                    .body
                    .starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion=") =>
            {
                (200, json!({"access_token": "gcp-account-token"}).to_string())
            }
            ("POST", path) if path.starts_with(&format!("/v1/{GCP_KEY}:")) => {
                let body = seen.json();
                if body["additionalAuthenticatedData"] != aad.as_str() {
                    return (400, "{}".to_string());
                }
                if path.ends_with(":encrypt") {
                    let sealed = fake_seal(&unb64(body["plaintext"].as_str().unwrap()));
                    (200, json!({"ciphertext": b64(sealed.as_bytes())}).to_string())
                } else {
                    let sealed = String::from_utf8(unb64(body["ciphertext"].as_str().unwrap())).unwrap();
                    (200, json!({"plaintext": b64(&fake_open(&sealed))}).to_string())
                }
            }
            _ => (404, "{}".to_string()),
        }
    })
}

#[test]
fn gcp_cloud_kms_wraps_and_unwraps_with_the_metadata_servers_token_or_a_service_accounts() {
    let stub = gcp_stub();
    let by_metadata = conf(CloudConf {
        kms_provider: "gcp".into(),
        kms_key: GCP_KEY.into(),
        kms_endpoint: stub.base.clone(),
        kms_token_endpoint: stub.base.clone(),
        ..CloudConf::default()
    });
    round_trip(&Kms, &by_metadata);
    let calls: Vec<Seen> = stub
        .seen()
        .into_iter()
        .filter(|seen| seen.path.starts_with("/v1/"))
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(calls
        .iter()
        .all(|call| call.header("authorization") == "Bearer gcp-metadata-token"));

    let scratch = Scratch::new();
    let account = json!({
        "type": "service_account",
        "client_email": "seal@p.iam.gserviceaccount.com",
        "private_key": include_str!("../../../../moqui-lib/testdata/qz-test-key-pkcs8.pem"),
        "token_uri": format!("{}/token", stub.base),
    });
    let by_account = conf(CloudConf {
        kms_credentials: scratch.file("gcp.json", &account.to_string()),
        ..by_metadata.cloud.clone()
    });
    round_trip(&Kms, &by_account);
    let seen = stub.seen();
    let token = seen.iter().find(|seen| seen.path == "/token").unwrap();
    let jwt = token.body.split("assertion=").nth(1).unwrap();
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(jwt.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(claims["scope"], "https://www.googleapis.com/auth/cloudkms");
    assert_eq!(claims["iss"], "seal@p.iam.gserviceaccount.com");
    assert!(seen
        .iter()
        .filter(|seen| seen.path.starts_with("/v1/"))
        .skip(2)
        .all(|call| call.header("authorization") == "Bearer gcp-account-token"));
}

fn azure_stub() -> Stub {
    Stub::start(|seen| {
        let url = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
        let unurl = |text: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text).unwrap();
        match (seen.method.as_str(), seen.path.as_str()) {
            ("POST", "/tenant-1/oauth2/v2.0/token")
                if seen.body.contains("grant_type=client_credentials")
                    && seen.body.contains("client_id=app-1")
                    && seen.body.contains("client_secret=the-client-secret")
                    && seen.body.contains("scope=https%3A%2F%2Fvault.azure.net%2F.default") =>
            {
                (200, json!({"access_token": "azure-app-token"}).to_string())
            }
            ("GET", path)
                if path.starts_with("/metadata/identity/oauth2/token?api-version=")
                    && seen.header("metadata") == "true" =>
            {
                (200, json!({"access_token": "azure-identity-token"}).to_string())
            }
            ("POST", "/keys/seal/wrapkey?api-version=7.4") if seen.json()["alg"] == "RSA-OAEP-256" => {
                let sealed = fake_seal(&unurl(seen.json()["value"].as_str().unwrap()));
                (
                    200,
                    json!({"kid": "VAULT/keys/seal/v7", "value": url(sealed.as_bytes())}).to_string(),
                )
            }
            ("POST", "/keys/seal/v7/unwrapkey?api-version=7.4") if seen.json()["alg"] == "RSA-OAEP-256" => {
                let sealed = String::from_utf8(unurl(seen.json()["value"].as_str().unwrap())).unwrap();
                (200, json!({"value": url(&fake_open(&sealed))}).to_string())
            }
            _ => (404, "{}".to_string()),
        }
    })
}

#[test]
fn azure_key_vault_wraps_and_unwraps_by_client_credentials_or_the_managed_identity() {
    let scratch = Scratch::new();
    let stub = azure_stub();
    let key = format!("{}/keys/seal", stub.base);
    let by_app = conf(CloudConf {
        kms_provider: "azure".into(),
        kms_key: key.clone(),
        kms_token_endpoint: stub.base.clone(),
        kms_credentials: scratch.file(
            "azure.json",
            r#"{"tenant_id": "tenant-1", "client_id": "app-1", "client_secret": "the-client-secret"}"#,
        ),
        ..CloudConf::default()
    });
    // the stub's kid names its own base; the unwrap takes a kid only under the conf's key
    let root = RootKey::random().unwrap();
    let mut metadata = Kms.wrap(&root, &by_app).unwrap();
    assert_eq!(metadata["kid"], "VAULT/keys/seal/v7");
    metadata.insert("kid".into(), json!(format!("{key}/v7")));
    assert_eq!(Kms.unwrap(&metadata, &by_app).unwrap().expose(), root.expose());
    let calls: Vec<Seen> = stub
        .seen()
        .into_iter()
        .filter(|seen| seen.path.starts_with("/keys/"))
        .collect();
    assert!(calls
        .iter()
        .all(|call| call.header("authorization") == "Bearer azure-app-token"));

    let by_identity = conf(CloudConf {
        kms_credentials: Credentials::Metadata,
        ..by_app.cloud.clone()
    });
    assert_eq!(Kms.unwrap(&metadata, &by_identity).unwrap().expose(), root.expose());
    let last = stub.seen().into_iter().last().unwrap();
    assert_eq!(last.header("authorization"), "Bearer azure-identity-token");
}

fn transit_stub(refuse: Arc<std::sync::atomic::AtomicBool>) -> Stub {
    Stub::start(move |seen| {
        if refuse.load(Ordering::SeqCst) || seen.header("x-vault-token") != "the-vault-token" {
            return (403, json!({"errors": ["permission denied"]}).to_string());
        }
        match seen.path.as_str() {
            "/v1/transit/encrypt/moqui-root" => {
                let sealed = fake_seal(&unb64(seen.json()["plaintext"].as_str().unwrap()));
                (
                    200,
                    json!({"data": {"ciphertext": format!("vault:v1:{sealed}")}}).to_string(),
                )
            }
            "/v1/transit/decrypt/moqui-root" => {
                let sealed = seen.json()["ciphertext"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("vault:v1:")
                    .to_string();
                (
                    200,
                    json!({"data": {"plaintext": b64(&fake_open(&sealed))}}).to_string(),
                )
            }
            _ => (404, "{}".to_string()),
        }
    })
}

#[test]
fn transit_wraps_and_unwraps_with_the_vault_token_and_a_refused_unseal_stays_sealed() {
    let scratch = Scratch::new();
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stub = transit_stub(Arc::clone(&refuse));
    let cloud = CloudConf {
        transit_url: stub.base.clone(),
        transit_mount: "transit".into(),
        transit_key: "moqui-root".into(),
        transit_namespace: "ops".into(),
        transit_token: scratch.file("vault-token", "the-vault-token\n"),
        ..CloudConf::default()
    };
    let metadata = round_trip(&Transit, &conf(cloud.clone()));
    assert_eq!(metadata["backend"], "openbao");
    assert!(stub
        .seen()
        .iter()
        .all(|seen| seen.header("x-vault-namespace") == "ops" && !seen.body.contains("the-vault-token")));

    // a production first start with transit makes its seal by itself; a start the Vault refuses waits sealed
    let seal_dir = scratch.0.join("runtime");
    let with_method = SealConf {
        method: "transit".into(),
        ..conf(cloud)
    };
    let keeper = Keeper::open(&file::path_in(&seal_dir), with_method.clone()).unwrap();
    assert_eq!(keeper.start(Mode::Operator).unwrap(), Start::Unsealed);
    assert!(keeper.status().shares_waiting);
    refuse.store(true, Ordering::SeqCst);
    let restarted = Keeper::open(&file::path_in(&seal_dir), with_method).unwrap();
    match restarted.start(Mode::Operator).unwrap() {
        Start::Waiting(reason) => assert!(reason.starts_with("auto-unseal failed"), "{reason}"),
        Start::Unsealed => panic!("a refused transit unsealed"),
    }
    assert!(!restarted.is_unsealed());
    assert!(restarted.status().reason.contains("403"));
}

#[test]
fn a_credential_is_a_file_or_a_systemd_credential_never_a_conf_literal() {
    let scratch = Scratch::new();
    assert_eq!(
        Credentials::parse("seal@kms-credentials", "").unwrap(),
        Credentials::Metadata
    );
    assert!(Credentials::parse("seal@transit-token", "hvs.the-token-itself").is_err());
    assert!(Credentials::parse("seal@transit-token", "relative/path").is_err());
    assert!(Credentials::parse("seal@transit-token", "systemd:../x").is_err());
    let open = scratch.file("open-token", "x");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let Credentials::File(path) = &open else { unreachable!() };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(
            open.read("seal@transit-token").is_err(),
            "a file others may write is refused"
        );
    }
    // a seal.json made with another provider than the conf's names it, and no call goes out
    let cloud = CloudConf {
        kms_provider: "gcp".into(),
        kms_key: GCP_KEY.into(),
        ..CloudConf::default()
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert("provider".into(), json!("aws"));
    metadata.insert("wrapped".into(), json!("x"));
    let err = Kms.unwrap(&metadata, &conf(cloud)).unwrap_err();
    assert!(err.to_string().contains("seal@kms-provider"), "{err}");
}

/// The settings that say where the root key goes are read from the conf files only: an environment name's is ignored.
#[test]
fn the_cloud_settings_are_read_from_the_conf_files_only() {
    use crate::context::conf_env::{Outcome, Override, Source};
    let text =
        r#"<moqui-conf><seal method="transit" transit-url="https://attacker.example" transit-key="k"/></moqui-conf>"#;
    let conf = moqui_lib::node::MNode::parse_text("moqui-conf", text).unwrap();
    let from_env = Override {
        name: "moqui_conf__seal__transit_url".into(),
        source: Source::Environment,
        selector: Some("seal@transit-url".into()),
        secret: false,
        outcome: Outcome::Set,
    };
    let read = SealConf::from_gate(&conf, &[from_env]).unwrap();
    assert_eq!(read.cloud.transit_url, "");
    assert_eq!(read.cloud.transit_key, "k");
    assert_eq!(read.cloud.transit_mount, "transit");
}
