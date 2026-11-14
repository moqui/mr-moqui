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

//! The runner's transport over HTTP: the runner REST at `<server>/ci/runner/jobs/<job run>/`, and the checkout with the
//! system `git` over the server's smart HTTP, both with the job token as a bearer token.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use moqui_lib::http::{Method, OpenRequest, RestClient};

use super::Transport;

const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// A call that fails to connect or is answered 5xx is tried this many times, a few seconds apart.
const ATTEMPTS: u32 = 4;

/// Percent-encode a query value.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The runner REST of one job over HTTP.
pub struct HttpTransport {
    base: String,
    token: String,
    client: RestClient,
}

impl HttpTransport {
    /// For the job run `job_run_id` on `server_url`, with its job token.
    pub fn new(server_url: &str, job_run_id: &str, token: &str) -> Self {
        Self {
            base: format!("{}/ci/runner/jobs/{job_run_id}/", server_url.trim_end_matches('/')),
            token: token.to_string(),
            client: RestClient::new().timeout(TRANSFER_TIMEOUT),
        }
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }

    fn answer(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap_or(serde_json::Value::Null)
    }
}

/// Call the runner REST outside a job: `register` with a pool token, `claim` with a runner token. The status and the
/// answer.
///
/// # Errors
///
/// When the server is not reached.
pub fn call(
    server_url: &str,
    path: &str,
    token: &str,
    body: &serde_json::Value,
) -> Result<(u16, serde_json::Value), String> {
    let response = RestClient::new()
        .timeout(CALL_TIMEOUT)
        .post(format!("{}/ci/runner/{path}", server_url.trim_end_matches('/')))
        .header("Authorization", format!("Bearer {token}"))
        .json(body)
        .allow_in_sim(true)
        .send(false)
        .map_err(|err| err.to_string())?;
    Ok((response.status(), HttpTransport::answer(response.body())))
}

impl Transport for HttpTransport {
    fn json(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), String> {
        let method = Method::parse(method).ok_or_else(|| format!("{method} is not an HTTP method"))?;
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(u64::from(attempt) * 2));
            }
            let mut call = self
                .client
                .request(method, format!("{}{path}", self.base))
                .header("Authorization", self.bearer())
                .timeout(CALL_TIMEOUT)
                .allow_in_sim(true);
            if let Some(body) = body {
                call = call.json(body);
            }
            match call.send(false) {
                Ok(response) if response.status() < 500 => {
                    return Ok((response.status(), Self::answer(response.body())));
                }
                Ok(response) => last = format!("the server answered {}", response.status()),
                Err(err) => last = err.to_string(),
            }
        }
        Err(last)
    }

    fn upload(&self, path_and_query: &str, file: &Path) -> Result<(u16, serde_json::Value), String> {
        let opened = std::fs::File::open(file).map_err(|err| format!("{}: {err}", file.display()))?;
        let length = opened.metadata().map(|meta| meta.len()).ok();
        let request = OpenRequest {
            method: Method::Put,
            url: format!("{}{path_and_query}", self.base),
            headers: vec![
                ("Authorization".to_string(), self.bearer()),
                ("Content-Type".to_string(), "application/octet-stream".to_string()),
            ],
            body: None,
            timeout: Some(TRANSFER_TIMEOUT),
        };
        let mut response = self
            .client
            .upload(request, Box::new(opened), length)
            .map_err(|err| err.to_string())?;
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::Read::take(&mut response.reader, 64 * 1024), &mut text);
        Ok((response.status, Self::answer(&text)))
    }

    fn download(&self, path_and_query: &str, to: &Path) -> Result<u16, String> {
        let request = OpenRequest {
            method: Method::Get,
            url: format!("{}{path_and_query}", self.base),
            headers: vec![("Authorization".to_string(), self.bearer())],
            body: None,
            timeout: Some(TRANSFER_TIMEOUT),
        };
        let mut response = self.client.open(request).map_err(|err| err.to_string())?;
        if response.status == 200 {
            let mut file = std::fs::File::create(to).map_err(|err| format!("{}: {err}", to.display()))?;
            std::io::copy(&mut response.reader, &mut file).map_err(|err| format!("{}: {err}", to.display()))?;
        }
        Ok(response.status)
    }

    fn checkout(&self, job: &serde_json::Value, workspace: &Path) -> Result<String, String> {
        let url = job["cloneUrl"].as_str().ok_or("the job has no cloneUrl")?;
        let commit = job["commitId"].as_str().ok_or("the job has no commitId")?;
        let depth = job["spec"]["fetchDepth"].as_u64().unwrap_or(1).max(1);
        checkout(url, commit, depth, &self.token, workspace)
    }
}

/// Check `commit` of the repository at `url` out into the empty directory `workspace` with the system `git`, a
/// shallow fetch of that commit alone. The token goes to git in its environment configuration, not its arguments.
///
/// # Errors
///
/// When `git` is not there or a command fails, with what it printed.
pub fn checkout(url: &str, commit: &str, depth: u64, token: &str, workspace: &Path) -> Result<String, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "false")
            .env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", "http.extraHeader")
            .env("GIT_CONFIG_VALUE_0", format!("Authorization: Bearer {token}"))
            .env("GIT_CONFIG_KEY_1", "credential.helper")
            .env("GIT_CONFIG_VALUE_1", "")
            .output()
            .map_err(|err| format!("git did not run: {err}"))?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            Ok(text)
        } else {
            Err(format!("git {} failed: {}", args.first().unwrap_or(&""), text.trim()))
        }
    };
    let depth = format!("--depth={depth}");
    let mut text = run(&["init", "-q", "."])?;
    text.push_str(&run(&["fetch", "-q", &depth, "--no-tags", url, commit])?);
    text.push_str(&run(&[
        "-c",
        "advice.detachedHead=false",
        "checkout",
        "-q",
        "--detach",
        "FETCH_HEAD",
    ])?);
    Ok(text)
}
