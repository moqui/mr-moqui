//! Synchronous protocol-v1 HTTP provider. No retries: a lost response is an ambiguous operation.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moqui_core::deterministic::{self, Provider};
use moqui_lib::error::Error;
use serde_json::{Value, json};

struct Hook {
    client: reqwest::blocking::Client,
    url: reqwest::Url,
    run: String,
    log: Mutex<File>,
}

fn required(name: &str) -> Result<String, Error> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::new(format!("deterministic mode requires {name}")))
}

/// Initialize once per process; factory restarts preserve provider, sequence and append log.
pub fn offline() -> bool {
    deterministic::enabled() && std::env::var("MOQUI_DETERMINISTIC_OFFLINE").as_deref() == Ok("true")
}

pub fn init() -> Result<(), Error> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    if !deterministic::requested() {
        return Ok(());
    }
    INIT.get_or_init(|| install().map_err(|err| err.to_string()))
        .clone()
        .map_err(Error::new)
}

fn install() -> Result<(), Error> {
    let url = reqwest::Url::parse(&required("MOQUI_DETERMINISTIC_HOOK")?)
        .map_err(|_| Error::new("invalid deterministic hook URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/transaction"
    {
        return Err(Error::new(
            "deterministic hook must be an HTTP(S) /transaction URL without credentials, query or fragment",
        ));
    }
    if required("MOQUI_DETERMINISTIC_ENGINE")? != "rust" {
        return Err(Error::new("MOQUI_DETERMINISTIC_ENGINE must be rust"));
    }
    let run = required("MOQUI_DETERMINISTIC_RUN")?;
    let path = required("MOQUI_DETERMINISTIC_LOG")?;
    if !Path::new(&path).is_absolute() {
        return Err(Error::new("deterministic log path must be absolute"));
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options
        .open(path)
        .map_err(|_| Error::new("could not open deterministic append log"))?;
    let timeout = std::env::var("MOQUI_DETERMINISTIC_TIMEOUT_SECONDS")
        .unwrap_or_else(|_| "120".to_string())
        .parse::<u64>()
        .ok()
        .filter(|seconds| (1..=3600).contains(seconds))
        .ok_or_else(|| Error::new("MOQUI_DETERMINISTIC_TIMEOUT_SECONDS must be 1..3600"))?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(timeout))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| Error::new("could not initialize deterministic HTTP client"))?;
    tracing::warn!(
        "Deterministic mode: application work is serial, async services drain inline; autonomous scheduled jobs are unsupported"
    );
    deterministic::install(Arc::new(Hook {
        client,
        url,
        run,
        log: Mutex::new(log),
    }))?;
    if offline() {
        tracing::warn!("Explicit deterministic offline mode: search, cluster and SSH disabled");
    }
    Ok(())
}

pub fn bootstrap() -> Result<Option<deterministic::Operation>, Error> {
    init()?;
    deterministic::bootstrap()
}

impl Hook {
    fn envelope(&self, mut value: Value) -> Value {
        value["version"] = json!(1);
        value["run_id"] = json!(self.run);
        value["engine"] = json!("rust");
        value
    }

    fn post(&self, request: &Value) -> Result<Value, Error> {
        let response = self
            .client
            .post(self.url.clone())
            .json(request)
            .send()
            .map_err(|_| Error::new("deterministic hook transport failed"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::new("deterministic hook returned non-200 status"));
        }
        let mut bytes = Vec::new();
        response
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::new("deterministic hook response read failed"))?;
        if bytes.len() > 65536 {
            return Err(Error::new("deterministic hook response too large"));
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::new("deterministic hook returned invalid JSON"))
    }

    fn append(&self, value: &Value) -> Result<(), Error> {
        let mut log = self.log.lock().unwrap();
        let mut bytes = serde_json::to_vec(value).map_err(|_| Error::new("deterministic event encoding failed"))?;
        bytes.push(b'\n');
        log.write_all(&bytes)
            .and_then(|()| log.flush())
            .map_err(|_| Error::new("deterministic log write/flush failed"))
    }
}

impl Provider for Hook {
    fn random(&self, domain: &str, length: usize) -> Result<Vec<u8>, Error> {
        let request = self.envelope(json!({"event":"random", "domain":domain, "length":length}));
        self.append(&request)?;
        let response = self.post(&request)?;
        let hex = response
            .get("bytes")
            .and_then(Value::as_str)
            .filter(|hex| {
                hex.len() == length * 2
                    && hex
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| Error::new("invalid deterministic random response"))?;
        if response.as_object().map(|value| value.len()) != Some(1) {
            return Err(Error::new("invalid deterministic random fields"));
        }
        (0..hex.len())
            .step_by(2)
            .map(|index| {
                u8::from_str_radix(&hex[index..index + 2], 16)
                    .map_err(|_| Error::new("invalid deterministic random hex"))
            })
            .collect()
    }

    fn begin(&self, request: Value) -> Result<Value, Error> {
        let mut request = self.envelope(request);
        moqui_core::deterministic::trace(
            "coordinator",
            "begin",
            request["artifact"]["name"].as_str().unwrap_or(""),
            "requested",
        )?;
        let response = self.post(&request)?;
        let id = response
            .get("operation_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let millis = response.get("time_epoch_ms").and_then(Value::as_i64);
        let (Some(id), Some(millis)) = (id, millis) else {
            return Err(Error::new("invalid deterministic begin response"));
        };
        if id.len() > 256 || id.chars().any(char::is_control) || jiff::Timestamp::from_millisecond(millis).is_err() {
            return Err(Error::new("invalid deterministic assignment"));
        }
        request["operation_id"] = json!(id);
        request["time_epoch_ms"] = json!(millis);
        self.append(&request)?;
        Ok(response)
    }

    fn trace(&self, request: Value) -> Result<(), Error> {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        // Sequence allocation and append must be serialized together.
        static SERIAL: Mutex<()> = Mutex::new(());
        let _guard = SERIAL.lock().unwrap();
        let mut request = self.envelope(request);
        request["trace_sequence"] = json!(SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        self.append(&request)?;
        let kind = request["kind"].as_str().unwrap_or("");
        let action = request["action"].as_str().unwrap_or("");
        let outcome = request["outcome"].as_str().unwrap_or("");
        if std::env::var("MOQUI_DETERMINISTIC_OPERATIONS").as_deref() == Ok("true")
            && ((kind.starts_with("AT_") && outcome == "exit")
                || (kind == "jdbc" && matches!(outcome, "succeeded" | "failed"))
                || (kind == "data" && outcome != "requested"))
        {
            let operation = self.envelope(json!({"event":"operation", "sequence":request["trace_sequence"],
                "kind":kind, "action":action, "name":request["name"], "outcome":outcome,
                "transaction_sequence":request["parent_sequence"]}));
            if self.post(&operation)?.get("ok") != Some(&Value::Bool(true)) {
                return Err(Error::new("operation database comparison rejected"));
            }
        }
        Ok(())
    }

    fn complete(&self, request: Value) -> Result<(), Error> {
        let request = self.envelope(request);
        self.append(&request)?;
        let response = self.post(&request)?;
        if response.get("ok") != Some(&Value::Bool(true)) {
            return Err(Error::new("invalid deterministic completion response"));
        }
        Ok(())
    }
}
