//! Generic external executor bridge. Only server configuration selects an executable;
//! request data is JSON on stdin, never shell syntax. One invocation, no retries.
use crate::entity::EntityFacade;
use moqui_lib::{
    error::Error,
    object::{to_plain_string, Value},
};
use serde_json::{json, Value as Json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
pub(super) fn register(map: &mut BTreeMap<&'static str, super::rust_services::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, request)?;
    Ok(())
}
fn request(params: &BTreeMap<String, Value>, _: &EntityFacade) -> Result<BTreeMap<String, Value>, Error> {
    let result = (|| {
        let config = std::env::var("MOQUI_EXTERNAL_AGENT_COMMAND").map_err(|_| {
            Error::new("External agent is not configured: set MOQUI_EXTERNAL_AGENT_COMMAND to a JSON argv array")
        })?;
        let argv: Vec<String> =
            serde_json::from_str(&config).map_err(|_| Error::new("Invalid external agent command configuration"))?;
        let input: BTreeMap<_, _> = params.iter().map(|(k, v)| (k.clone(), to_plain_string(v))).collect();
        invoke(&argv, &json!(input), Duration::from_secs(90))
    })();
    // Execution failure is data so callers can persist it without transaction rollback.
    Ok(match result {
        Ok(value) => BTreeMap::from([("resultJson".into(), Value::String(value.to_string()))]),
        Err(error) => BTreeMap::from([("failure".into(), Value::String(error.to_string()))]),
    })
}
fn invoke(argv: &[String], input: &Json, timeout: Duration) -> Result<Json, Error> {
    let Some((executable, args)) = argv.split_first().filter(|(executable, _)| !executable.is_empty()) else {
        return Err(Error::new("External agent command is empty"));
    };
    let bytes = serde_json::to_vec(input).map_err(|_| Error::new("Invalid external agent request"))?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::new("External agent request too large"));
    }
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::new("Cannot start configured external agent adapter"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped above");
    let stdout = child.stdout.take().expect("stdout is piped above");
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = stdin.write_all(&bytes);
        drop(stdin);
        let _ = send.send(result);
    });
    let (send_output, receive_output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = send_output.send(result);
    });
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|_| Error::new("Cannot read external agent adapter status"))?
            {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    "External agent adapter timed out; execution will not be retried",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !status.success() {
            return Err(Error::new(
                "External agent adapter failed; execution will not be retried",
            ));
        }
        receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| Error::new("External agent input timed out"))?
            .map_err(|_| Error::new("Cannot write external agent request"))?;
        let bytes = receive_output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| Error::new("External agent output timed out"))?
            .map_err(|_| Error::new("Cannot read external agent response"))?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::new("External agent response too large"));
        }
        let response: Json =
            serde_json::from_slice(&bytes).map_err(|_| Error::new("Invalid external agent response JSON"))?;
        if let Some(failure) = response["failure"].as_str().filter(|s| !s.is_empty()) {
            return Err(Error::new(failure));
        }
        response
            .get("result")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| Error::new("External agent response needs a result object"))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn json_is_data_and_failures_are_not_retried() {
        let command = vec![
            "sh".into(),
            "-c".into(),
            "printf '{\"result\":'; cat; printf '}'".into(),
        ];
        let input = json!({"instruction": "$(exit 99); `exit 98`\nquoted \"text\"", "operation":"launch", "providerInstanceId":"codex", "model":"gpt-6-luna", "reasoningEffort":"low"});
        assert_eq!(invoke(&command, &input, Duration::from_secs(2)).unwrap(), input);
        let fail = vec![
            "sh".into(),
            "-c".into(),
            "cat >/dev/null; printf '{\"failure\":\"fixture failure\"}'".into(),
        ];
        assert!(invoke(&fail, &json!({}), Duration::from_secs(2))
            .unwrap_err()
            .to_string()
            .contains("fixture failure"));
    }
    #[test]
    fn invalid_output_and_hung_adapters_fail() {
        let invalid = vec!["sh".into(), "-c".into(), "cat >/dev/null; printf '{}'".into()];
        assert!(invoke(&invalid, &json!({}), Duration::from_secs(2)).is_err());
        let hung = vec!["sh".into(), "-c".into(), "exec sleep 5".into()];
        let start = Instant::now();
        assert!(invoke(&hung, &json!({}), Duration::from_millis(50))
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
