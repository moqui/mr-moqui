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

//! `moqui check`, `moqui refs` and `moqui lsp` with `--server URL` (plans/design-dev-aide.md 13.6, F13): the checks and
//! the artifact index of a running dev instance, over its MCP tools `check` and `artifact` (`POST /mcp`, `tools/call`),
//! as the user of the login key `moqui tui --server` keeps for that server (or `MOQUI_LOGIN_KEY`). A local file is the
//! `component://` location of the nearest directory above it with a `component.xml`, and a location the file of the
//! runtime's component of that name.
//!
//! With a project (`--project DIR`, [`Remote::with_project`]), every call names it, and the server answers about that
//! Moqui project on its machine instead of its own runtime; a local file is then named by its absolute path, and a
//! location the file of that project's component.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use moqui_devkit::{Problem, Severity};
use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

/// Where an artifact is defined, as the editor opens it.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub path: PathBuf,
    pub line: u32,
    pub summary: String,
}

/// The MCP protocol version the calls name.
const PROTOCOL: &str = "2026-07-28";

/// A running instance's dev tools.
pub struct Remote {
    base: String,
    key: String,
    client: reqwest::blocking::Client,
    /// Component names to their directories here, read once.
    components: Mutex<Option<HashMap<String, PathBuf>>>,
    /// The project every call names: its directory on the server's machine.
    project: Option<String>,
}

impl Remote {
    /// The instance at `url` with its stored login key.
    ///
    /// # Errors
    ///
    /// Returns an error when no login key is stored for it, or the HTTP client cannot be made.
    pub fn connect(url: &str) -> Result<Self, Error> {
        let base = url.trim().trim_end_matches('/').to_string();
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(Error::new(format!("--server {url}: an http or https URL")));
        }
        // the login key goes in a header: in the clear only to this machine
        if base.starts_with("http://") && !loopback(&base) {
            return Err(Error::new(format!(
                "--server {url}: the login key is sent with each call, so a server not on this machine is https"
            )));
        }
        let key = std::env::var("MOQUI_LOGIN_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .or_else(|| {
                crate::tui_remote::key_file(&base)
                    .ok()
                    .and_then(|path| crate::tui_remote::stored_key(&path))
            })
            .ok_or_else(|| {
                Error::new(format!(
                    "no login key for {base}: log in once with moqui tui --server {base}, or set MOQUI_LOGIN_KEY"
                ))
            })?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .user_agent(concat!("moqui-devtools/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| Error::new(format!("HTTP client: {err}")))?;
        Ok(Self {
            base,
            key,
            client,
            components: Mutex::new(None),
            project: None,
        })
    }

    /// Ask about the Moqui project at `dir` (a directory on the server's machine) instead of the server's own runtime.
    /// When `dir` is a project here too, its canonical root is sent, and its components are the ones a location the
    /// server answers is a file of.
    #[must_use]
    pub fn with_project(mut self, dir: &Path, java: bool) -> Self {
        let found = if java {
            moqui_devkit::project::detect_java(dir)
        } else {
            moqui_devkit::project::detect(dir)
        };
        let named = match found {
            Ok(project) => {
                let mut dirs = component_dirs_in(&project.runtime);
                if project.layout == moqui_devkit::project::Layout::Java {
                    dirs.insert("framework".to_string(), project.root.join("framework"));
                }
                *moqui_lib::sync::lock(&self.components) = Some(dirs);
                project.root
            }
            // not here: the server's machine has it, or answers why not
            Err(_) => std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()),
        };
        self.project = Some(named.display().to_string());
        self
    }

    /// `arguments` with the project, when one is named.
    fn with_project_arg(&self, mut arguments: Json) -> Json {
        if let (Some(project), Some(map)) = (&self.project, arguments.as_object_mut()) {
            map.insert("project".to_string(), json!(project));
        }
        arguments
    }

    /// How a call names the local file `path`: its absolute path in a project, else its `component://` location.
    fn file_arg(&self, path: &Path) -> Option<String> {
        if self.project.is_some() {
            return Some(
                std::fs::canonicalize(path)
                    .unwrap_or_else(|_| path.to_path_buf())
                    .display()
                    .to_string(),
            );
        }
        location_of(path)
    }

    /// A tool's answer: the JSON of its first text content.
    ///
    /// # Errors
    ///
    /// Returns the server's error, or why the answer did not come.
    pub fn call(&self, tool: &str, arguments: Json) -> Result<Json, Error> {
        // a stateless call: each names the protocol and the client, as the MCP server asks
        let meta = json!({"io.modelcontextprotocol/protocolVersion": PROTOCOL, "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "moqui-devtools", "version": env!("CARGO_PKG_VERSION")}});
        let arguments = self.with_project_arg(arguments);
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments, "_meta": meta}});
        let response = self
            .client
            .post(format!("{}/mcp", self.base))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", tool)
            .header("Authorization", format!("Bearer {}", self.key))
            .body(body.to_string())
            .send()
            .map_err(|err| Error::new(format!("{}: {err}", self.base)))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(Error::new(format!(
                "{} answered {status}: {}",
                self.base,
                text.chars().take(300).collect::<String>()
            )));
        }
        // a JSON answer, or one event of a stream (the transport lets the server answer either way)
        let body = if text.trim_start().starts_with('{') {
            text.as_str()
        } else {
            text.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .rfind(|data| data.starts_with('{'))
                .unwrap_or(text.as_str())
        };
        let answer: Json = serde_json::from_str(body).map_err(|err| Error::new(format!("{}: {err}", self.base)))?;
        if let Some(error) = answer.get("error") {
            return Err(Error::new(format!(
                "{tool}: {}",
                error["message"].as_str().unwrap_or("failed")
            )));
        }
        let content = answer["result"]["content"][0]["text"].as_str().unwrap_or_default();
        if answer["result"]["isError"].as_bool() == Some(true) {
            return Err(Error::new(format!("{tool}: {content}")));
        }
        Ok(serde_json::from_str(content).unwrap_or_else(|_| json!({ "text": content })))
    }

    /// The problems of `text` as the file at `path`.
    pub fn check(&self, path: &Path, text: &str) -> Result<Vec<Problem>, Error> {
        let location = self.file_arg(path).ok_or_else(|| {
            Error::new(format!(
                "{} is in no component (no component.xml above it)",
                path.display()
            ))
        })?;
        let answer = self.call("check", json!({ "file": location, "text": text }))?;
        Ok(answer["problems"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|problem| Problem {
                path: path.display().to_string(),
                line: u32::try_from(problem["line"].as_u64().unwrap_or(0)).unwrap_or(0),
                column: u32::try_from(problem["column"].as_u64().unwrap_or(0)).unwrap_or(0),
                severity: match problem["severity"].as_str() {
                    Some("warning") => Severity::Warning,
                    Some("info") => Severity::Info,
                    _ => Severity::Error,
                },
                rule: problem["rule"].as_str().unwrap_or_default().to_string(),
                message: problem["message"].as_str().unwrap_or_default().to_string(),
                skill: problem["skill"].as_str().map(str::to_string),
            })
            .collect())
    }

    /// The file here of a location the server names (`component://...`), else the text as a path.
    fn local(&self, file: &str) -> PathBuf {
        let Some((name, rest)) = file.strip_prefix("component://").and_then(|rest| rest.split_once('/')) else {
            return PathBuf::from(file);
        };
        let mut known = moqui_lib::sync::lock(&self.components);
        let map = known.get_or_insert_with(component_dirs);
        map.get(name).map_or_else(|| PathBuf::from(file), |dir| dir.join(rest))
    }

    fn target(&self, artifact: &Json) -> Option<Target> {
        let id = artifact["id"].as_str()?;
        Some(Target {
            id: id.to_string(),
            kind: artifact["kind"].as_str().unwrap_or_default().to_string(),
            name: artifact["name"].as_str().unwrap_or_default().to_string(),
            path: self.local(artifact["file"].as_str().unwrap_or_default()),
            line: u32::try_from(artifact["line"].as_u64().unwrap_or(1)).unwrap_or(1),
            summary: artifact["summary"].as_str().unwrap_or_default().to_string(),
        })
    }

    pub fn at(&self, path: &Path, line: u32, word: &str) -> Option<Target> {
        let location = self.file_arg(path)?;
        let answer = self
            .call(
                "artifact",
                json!({ "action": "at", "file": location, "line": line, "word": word }),
            )
            .ok()?;
        self.target(&answer["artifact"])
    }

    pub fn uses(&self, id: &str) -> Vec<(PathBuf, u32)> {
        self.refs(id, "in")
            .map(|answer| {
                answer["refs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|row| {
                        (
                            self.local(row["file"].as_str().unwrap_or_default()),
                            u32::try_from(row["line"].as_u64().unwrap_or(1)).unwrap_or(1),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `refs` answer of the artifact tool: `{artifact, direction, count, refs: [{how, file, line, artifact | name}]}`.
    pub fn refs(&self, id: &str, direction: &str) -> Result<Json, Error> {
        self.call(
            "artifact",
            json!({ "action": "refs", "id": id, "direction": direction }),
        )
    }

    /// [`Remote::refs`] of every artifact the local file `path` defines.
    ///
    /// # Errors
    ///
    /// Returns the server's error, or that the file is in no component.
    pub fn file_refs(&self, path: &Path, direction: &str) -> Result<Json, Error> {
        let file = self.file_arg(path).ok_or_else(|| {
            Error::new(format!(
                "{} is in no component (no component.xml above it)",
                path.display()
            ))
        })?;
        self.call(
            "artifact",
            json!({ "action": "refs", "file": file, "direction": direction }),
        )
    }

    pub fn find(&self, text: &str, kinds: &[String], limit: usize) -> Vec<Target> {
        let answer = self
            .call(
                "artifact",
                json!({ "action": "find", "text": text, "kinds": kinds.join(",") }),
            )
            .unwrap_or(Json::Null);
        answer["artifacts"]
            .as_array()
            .into_iter()
            .flatten()
            .take(limit)
            .filter_map(|artifact| self.target(artifact))
            .collect()
    }
}

/// The name of the component whose directory is `dir`: its `component.xml`'s name, else the directory's.
fn component_name(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("component.xml"))
        .ok()
        .and_then(|text| {
            let root = &text[text.find("<component")?..];
            let tag = &root[..root.find('>')?];
            // name= after any white space: a space, or the attributes one to a line
            let at = tag
                .match_indices("name=\"")
                .find(|(at, _)| tag[..*at].ends_with(char::is_whitespace))?
                .0
                + 6;
            Some(tag[at..at + tag[at..].find('"')?].to_string())
        })
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

/// The `component://` location of a file: the nearest directory above it with a `component.xml`.
pub(crate) fn location_of(path: &Path) -> Option<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut dir = path.parent();
    while let Some(here) = dir {
        if here.join("component.xml").is_file() {
            let rest = path.strip_prefix(here).ok()?.to_string_lossy().replace('\\', "/");
            return Some(format!("component://{}/{rest}", component_name(here)));
        }
        dir = here.parent();
    }
    None
}

/// The components of this runtime by name: the directories of `base-component`, `component` and `dev-component`.
fn component_dirs() -> HashMap<String, PathBuf> {
    component_dirs_in(&crate::runtime_framework_dirs().0)
}

/// The components of the runtime `runtime` by name.
fn component_dirs_in(runtime: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    for kind in ["base-component", "component", "dev-component"] {
        let Ok(entries) = std::fs::read_dir(runtime.join(kind)) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.join("component.xml").is_file() {
                out.insert(component_name(&dir), dir);
            }
        }
    }
    out
}

/// Whether an `http://` URL's host is this machine.
fn loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = authority
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']').map(|(host, _)| host))
        .unwrap_or_else(|| authority.split(':').next().unwrap_or(authority));
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_is_the_location_of_its_component() {
        let dir = std::env::temp_dir().join(format!("moqui-remote-{}", std::process::id()));
        let screen = dir.join("my-site/screen/MySite");
        std::fs::create_dir_all(&screen).unwrap();
        std::fs::write(
            dir.join("my-site/component.xml"),
            "<component name=\"site-x\" version=\"1\"/>",
        )
        .unwrap();
        std::fs::write(screen.join("A.xml"), "<screen/>").unwrap();
        assert_eq!(
            super::location_of(&screen.join("A.xml")).as_deref(),
            Some("component://site-x/screen/MySite/A.xml")
        );
        assert_eq!(super::location_of(&std::env::temp_dir().join("nowhere.xml")), None);
        // the attributes one to a line; a name= inside another attribute's name is not it
        std::fs::write(
            dir.join("my-site/component.xml"),
            "<component xmlns:xsi=\"x\"\n        name=\"site-y\" version=\"1\"/>",
        )
        .unwrap();
        assert_eq!(super::component_name(&dir.join("my-site")), "site-y");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_login_key_goes_in_the_clear_to_this_machine_only() {
        assert!(super::loopback("http://localhost:8080"));
        assert!(super::loopback("http://127.0.0.1:8080/x"));
        assert!(super::loopback("http://[::1]:8080"));
        assert!(!super::loopback("http://dev.example.com"));
        assert!(super::Remote::connect("http://dev.example.com").is_err());
    }
}
