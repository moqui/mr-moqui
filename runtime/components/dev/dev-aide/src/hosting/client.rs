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

//! A connection to a group server (plans/design-dev-aide.md 21.2): its `DevAideServer` row, the user's token there (a
//! `DevAideCredential` of its `serverId`, sent only to the server's own address), and its REST as that user: the
//! framework's (`/rest/s1/moqui/git`, `/rest/s1/moqui/hosting`) and, where the server offers one, the application's
//! (Smithy's, under the offer's `application.restBase`). After an app sign-in the token is a key for git alone, the
//! task panel's reads go with a key for tasks alone, and an application's REST with the installed app's key, which is
//! all it takes (question 18); a pasted token goes everywhere, as before.

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::Value as Json;

pub(crate) const SERVER: &str = "moqui.dev.DevAideServer";
pub(crate) const CREDENTIAL: &str = "moqui.dev.DevAideCredential";
/// Where the framework's REST of what a server offers is, before the offer says it.
pub(crate) const OFFER_PATH: &str = "/rest/s1/moqui/hosting/offer";

pub(crate) fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// A server's base URL as kept: `https://host[:port][/path]` with no `/` at its end, or `http` for a loopback IP
/// literal only (`crate::vcs::web_url`), so a token never crosses a network in the clear.
pub(crate) fn normalize_base(url: &str) -> Result<String, LibError> {
    let url = url.trim().trim_end_matches('/');
    let parsed = crate::vcs::checked_url(url)?;
    if parsed.query().is_some() || parsed.fragment().is_some() || url.contains(['?', '#']) {
        return Err(LibError::new(format!(
            "{url} is not a server's address (no user, query or page part)"
        )));
    }
    let prefix = crate::vcs::url_prefix(url)
        .ok_or_else(|| LibError::new(format!("{url} is not an https address (http only for 127.0.0.1)")))?;
    let path = parsed.path().trim_matches('/');
    Ok(if path.is_empty() {
        prefix
    } else {
        format!("{prefix}/{path}")
    })
}

/// A URL the server handed back (a fork's remote, its addons template, its gateway) checked to be on the server's own
/// origin, https (or a loopback IP literal), with no user part and no `\`.
pub(crate) fn on_server(server: &Server, url: &str) -> Result<String, LibError> {
    let parsed = crate::vcs::checked_url(url)?;
    server.url(parsed.as_str())?;
    Ok(url.trim().to_string())
}

/// `value` encoded for a URL's query.
pub(crate) fn query_text(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The host and port of an SSH remote (`ssh://user@host:2222/a.git`, `user@host:a.git`), port 22 when it names none.
pub(crate) fn ssh_host_port(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || host.starts_with('[') => (host, port),
            _ => (host_port, "22"),
        };
        return Some((host.trim_matches(['[', ']']).to_ascii_lowercase(), port.to_string()));
    }
    if url.contains("://") {
        return None;
    }
    let (authority, _) = url.split_once(':')?;
    if authority.contains('/') || authority.is_empty() {
        return None;
    }
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    Some((host.to_ascii_lowercase(), "22".into()))
}

/// What an application on the server offers in place of the framework's REST for the repositories it keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Application {
    pub name: String,
    pub rest_base: String,
}

/// A group server as the current user reaches it.
#[derive(Clone, Debug)]
pub(crate) struct Server {
    pub id: String,
    pub label: String,
    pub base: String,
    /// Whose connection it is; empty for the instance's.
    pub user_id: String,
    pub username: String,
    pub offer: Json,
    /// The user's token: after an app sign-in a key for git alone (git, the framework's git and hosting REST).
    pub token: Option<String>,
    /// The key for tasks alone (`DevAideServer.tasksKey`): the task source's reads and an issue's.
    pub tasks_token: Option<String>,
    /// The installed app's key (`DevAideServer.appKey`), for the application's REST alone, where the offer names one.
    pub app_token: Option<String>,
}

/// A key field of a server's row, when it is set.
fn key_field(row: &EntityValue, name: &str) -> Option<String> {
    Some(field(row, name)).filter(|key| !key.is_empty())
}

impl Server {
    pub(crate) fn from_row(facade: &EntityFacade, row: &EntityValue) -> Self {
        let id = field(row, "serverId");
        let user_id = field(row, "userId");
        Self {
            token: token_of(facade, &id, &user_id),
            tasks_token: key_field(row, "tasksKey"),
            app_token: key_field(row, "appKey"),
            offer: serde_json::from_str(&field(row, "offerJson")).unwrap_or(Json::Null),
            label: field(row, "label"),
            base: field(row, "baseUrl"),
            username: field(row, "username"),
            user_id,
            id,
        }
    }

    /// Server `id` when it is the user's own or the instance's.
    pub(crate) fn load(facade: &EntityFacade, id: &str, user: &str) -> Result<Self, LibError> {
        facade
            .find(SERVER)?
            .eq("serverId", Value::string(id))
            .one()?
            .filter(|row| {
                let owner = field(row, "userId");
                owner.is_empty() || owner == user
            })
            .map(|row| Self::from_row(facade, &row))
            .ok_or_else(|| LibError::new(format!("no group server {id} of yours")))
    }

    /// The user's own servers, then the instance's.
    pub(crate) fn all(facade: &EntityFacade, user: &str) -> Vec<Self> {
        let rows = facade
            .find(SERVER)
            .and_then(|find| find.order_by("label", false).list())
            .map(|rows| rows.into_values())
            .unwrap_or_default();
        let mut servers: Vec<Self> = rows
            .iter()
            .filter(|row| field(row, "userId") == user)
            .chain(rows.iter().filter(|row| field(row, "userId").is_empty()))
            .map(|row| Self::from_row(facade, row))
            .collect();
        servers.dedup_by(|a, b| a.id == b.id);
        servers
    }

    /// A path on the server, or a URL at its own address: the only places its token goes.
    pub(crate) fn url(&self, path_or_url: &str) -> Result<String, LibError> {
        if path_or_url.starts_with('/') {
            let origin = crate::vcs::url_prefix(&self.base).unwrap_or_else(|| self.base.clone());
            return Ok(format!("{origin}{path_or_url}"));
        }
        let same = crate::vcs::url_prefix(path_or_url)
            .is_some_and(|prefix| Some(prefix) == crate::vcs::url_prefix(&self.base));
        if same {
            Ok(path_or_url.to_string())
        } else {
            Err(LibError::new(format!("{path_or_url} is not on {}", self.base)))
        }
    }

    /// The offer's text at a path of keys (`git.httpsUrlTemplate`), empty when absent.
    pub(crate) fn offered(&self, path: &str) -> String {
        let mut at = &self.offer;
        for key in path.split('.') {
            at = &at[key];
        }
        match at {
            Json::String(text) => text.clone(),
            Json::Bool(flag) => flag.to_string(),
            Json::Number(number) => number.to_string(),
            _ => String::new(),
        }
    }

    /// The application the server offers for the repositories it keeps (`application.name`, `.restBase`).
    pub(crate) fn application(&self) -> Option<Application> {
        let name = self.offered("application.name");
        let rest_base = self.offered("application.restBase");
        (!name.is_empty() && !rest_base.is_empty()).then(|| Application {
            name,
            rest_base: rest_base.trim_end_matches('/').to_string(),
        })
    }

    /// The framework REST path of `name` (`repositories`, `hosting`), as the offer has it.
    pub(crate) fn rest_path(&self, name: &str) -> String {
        Some(self.offered(&format!("rest.{name}")))
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| format!("/rest/s1/moqui/{}", if name == "repositories" { "git" } else { name }))
    }

    /// Whether a remote URL is this server's: its HTTPS address, or its SSH host and port as offered.
    pub(crate) fn matches_remote(&self, remote: &str) -> bool {
        if let (Some(remote), Some(base)) = (crate::vcs::url_prefix(remote), crate::vcs::url_prefix(&self.base)) {
            return remote == base;
        }
        let host = self.offered("git.sshHost").to_ascii_lowercase();
        match ssh_host_port(remote) {
            Some((remote_host, remote_port)) if !host.is_empty() => {
                let port = Some(self.offered("git.sshPort"))
                    .filter(|port| !port.is_empty())
                    .unwrap_or_else(|| "22".into());
                remote_host == host && remote_port == port
            }
            _ => false,
        }
    }

    /// This server as `token` alone: a call made with one key, such as a key ending itself.
    pub(crate) fn with_token(&self, token: &str) -> Self {
        Self {
            token: Some(token.to_string()),
            tasks_token: None,
            app_token: None,
            ..self.clone()
        }
    }

    /// Whether `url` (on this server) is under the application's REST.
    fn on_application(&self, url: &str) -> bool {
        let Some(application) = self.application() else {
            return false;
        };
        let origin = crate::vcs::url_prefix(&self.base).unwrap_or_else(|| self.base.clone());
        let path = url.strip_prefix(&origin).unwrap_or(url);
        let path = path.split(['?', '#']).next().unwrap_or_default();
        path == application.rest_base || path.starts_with(&format!("{}/", application.rest_base))
    }

    /// The key a call of `url` goes with: the installed app's under the application's REST, where it is kept, else the
    /// user's token.
    fn key_for(&self, url: &str) -> Option<&String> {
        if self.on_application(url) {
            if let Some(app) = &self.app_token {
                return Some(app);
            }
        }
        self.token.as_ref()
    }

    /// A call of the server's REST as the user (with the token when there is one): its JSON answer, or the error the
    /// server said.
    pub(crate) fn request(
        &self,
        method: moqui_core::outbound::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Json>,
    ) -> Result<Json, LibError> {
        let url = self.url(path)?;
        let token = self.key_for(&url).cloned();
        self.request_as(token.as_deref(), method, url, query, body)
    }

    /// A read of the task panel's (the user's tasks, a repository's issues, one issue): with the key for tasks alone,
    /// or for the issues of a repository an application keeps (`kept_by_application`) the installed app's, which is
    /// all the application takes; else the user's token.
    pub(crate) fn tasks_get(&self, path: &str, kept_by_application: bool) -> Result<Json, LibError> {
        let url = self.url(path)?;
        let token = if kept_by_application {
            self.app_token.as_ref().or(self.tasks_token.as_ref())
        } else {
            self.tasks_token.as_ref()
        }
        .or(self.token.as_ref())
        .cloned();
        self.request_as(token.as_deref(), moqui_core::outbound::Method::Get, url, &[], None)
    }

    fn request_as(
        &self,
        token: Option<&str>,
        method: moqui_core::outbound::Method,
        mut url: String,
        query: &[(&str, &str)],
        body: Option<&Json>,
    ) -> Result<Json, LibError> {
        let query: Vec<String> = query
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{key}={}", query_text(value)))
            .collect();
        if !query.is_empty() {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&query.join("&"));
        }
        let mut headers = vec![("accept".to_string(), "application/json".to_string())];
        if let Some(token) = token {
            headers.push(("api_key".into(), token.to_string()));
        }
        if body.is_some() {
            headers.push(("content-type".into(), "application/json".into()));
        }
        send(url, method, headers, body.map(Json::to_string), &self.base)
    }

    pub(crate) fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Json, LibError> {
        self.request(moqui_core::outbound::Method::Get, path, query, None)
    }

    pub(crate) fn post(&self, path: &str, body: &Json) -> Result<Json, LibError> {
        self.request(moqui_core::outbound::Method::Post, path, &[], Some(body))
    }
}

/// Send a request to `base`'s server and read its JSON answer; an HTTP error is the server's `errors` text.
pub(crate) fn send(
    url: String,
    method: moqui_core::outbound::Method,
    headers: Vec<(String, String)>,
    body: Option<String>,
    base: &str,
) -> Result<Json, LibError> {
    // a call of a group server is the user's act there (a comment, a key, a re-run): never under a fence or in a sim
    if method != moqui_core::outbound::Method::Get {
        crate::workspace::unfenced(moqui_core::entity::fence::Effect::RemoteCall, base)?;
    }
    // a read of the server under an agent's read fence (the hosting tool, 21.8): only with the tool's permit for this
    // server's origin above the fence, never in a sim, and only for this GET; every write stays refused
    let _read = if method == moqui_core::outbound::Method::Get && moqui_core::entity::fence::active() {
        let origin = crate::vcs::url_prefix(base).unwrap_or_else(|| base.to_string());
        moqui_core::entity::fence::check_remote(&origin, &format!("GET {base}"))
            .map_err(|err| LibError::new(err.to_string()))?;
        Some(moqui_core::entity::fence::lift())
    } else {
        None
    };
    let request = moqui_core::outbound::Request {
        method,
        url,
        headers,
        body,
        timeout: Some(std::time::Duration::from_secs(30)),
        // as a task source: the view tier, which runs no code of its own here, reaches no internal address through a
        // server it stored; a team's server is often on the local network
        deny_internal: crate::tier::current().is_none_or(|tier| tier < crate::tier::Tier::Extend),
    };
    let response = moqui_core::outbound::send(request)?;
    let answer: Json = if response.body.trim().is_empty() {
        Json::Null
    } else {
        serde_json::from_str(&response.body).unwrap_or(Json::Null)
    };
    if !(200..300).contains(&response.status) {
        let said = said(&answer);
        return Err(LibError::new(format!(
            "{base} answered {} {}{}",
            response.status,
            moqui_core::outbound::reason_phrase(response.status),
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        )));
    }
    if answer.is_null() && !response.body.trim().is_empty() {
        return Err(LibError::new(format!("{base} answered no JSON")));
    }
    Ok(answer)
}

/// The errors of a REST answer (`errors` as text or a list), at most 300 characters.
pub(crate) fn said(answer: &Json) -> String {
    let text = match &answer["errors"] {
        Json::String(text) => text.clone(),
        Json::Array(items) => items.iter().filter_map(Json::as_str).collect::<Vec<_>>().join("; "),
        _ => answer["message"].as_str().unwrap_or("").to_string(),
    };
    text.chars().take(300).collect()
}

/// The user's token for server `id` (their own, else the instance's when the connection is the instance's).
fn token_of(facade: &EntityFacade, id: &str, user_id: &str) -> Option<String> {
    let rows = facade
        .find(CREDENTIAL)
        .ok()?
        .eq("serverId", Value::string(id))
        .list()
        .ok()?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let pick = |owner: &str| {
        rows.iter()
            .find(|row| field(row, "userId") == owner)
            .map(|row| field(row, "token"))
            .filter(|token| !token.is_empty())
    };
    pick(&user).or_else(|| user_id.is_empty().then(|| pick("")).flatten())
}

/// A JSON object of `pairs` whose values are not empty.
pub(crate) fn body(pairs: &[(&str, Json)]) -> Json {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        let empty = matches!(value, Json::Null) || value.as_str().is_some_and(str::is_empty);
        if !empty {
            map.insert((*key).to_string(), value.clone());
        }
    }
    Json::Object(map)
}

/// An id for a create the application takes once (Smithy's `operationId`, §15.1 of its design).
pub(crate) fn operation_id() -> String {
    moqui_core::user::random_string(24).unwrap_or_else(|_| format!("op{}", moqui_core::user::now_millis()))
}

/// The first list in an answer: the framework's named lists, or an application's `result` (itself a list, or holding
/// one).
pub(crate) fn first_list(answer: &Json) -> Vec<Json> {
    let pick = |value: &Json| -> Option<Vec<Json>> {
        match value {
            Json::Array(items) => Some(items.clone()),
            Json::Object(map) => map.values().find_map(|value| value.as_array().cloned()),
            _ => None,
        }
    };
    for key in ["mergeRequestList", "mergeRequests", "items", "result", "list"] {
        if let Some(items) = answer.get(key).and_then(pick) {
            return items;
        }
    }
    pick(answer).unwrap_or_default()
}

/// `answer`'s `result` when it has one (an application's operations answer so), else the answer.
pub(crate) fn result_of(answer: &Json) -> &Json {
    match answer.get("result") {
        Some(result) if !result.is_null() => result,
        _ => answer,
    }
}
