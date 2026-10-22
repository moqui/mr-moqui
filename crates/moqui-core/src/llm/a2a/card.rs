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

//! The agent cards (A2A 1.0 `AgentCard`). The public card at `/.well-known/agent-card.json` is minimal: the two
//! interfaces (JSON-RPC and HTTP+JSON, both at `<base>/a2a`), the security schemes, the capabilities and one generic
//! skill; it is 404 while the server is off. The extended card (`GetExtendedAgentCard`, authenticated) lists the
//! skills of the agents the caller may use: those with `exposeA2a` that serve `a2a` (from `a2aSkillsJson`, else one
//! skill per agent). The advertised base URL is the conf's `public-url`, else the request's scheme and host, with a
//! proxy's `X-Forwarded-Proto` and `X-Forwarded-Host` taken only when `trust-forwarded-headers` is set and the peer is
//! a proxy the `client-ip-header` conf believes; a value that is not a plain origin is never advertised. The card lists
//! no `openIdConnect` scheme until JWTs are accepted.

use serde_json::{json, Value as Json};

use super::types::VERSION;
use super::{Caller, Origin, Reply};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::llm::prompt::Surface;
use crate::llm::run::AgentDef;
use crate::llm::{flag_of, text_of};

/// The path of the public card.
pub const WELL_KNOWN: &str = "/.well-known/agent-card.json";
/// The most skills an extended card lists.
const MAX_SKILLS: usize = 100;

/// An absolute http(s) origin with an optional path: no user info, query, fragment, `//` in the path, spaces or
/// control characters; trailing slashes removed. `None` when it is not one.
pub fn valid_base_url(value: &str) -> Option<String> {
    let url = value.trim().trim_end_matches('/');
    if url.is_empty() || url.chars().any(|c| c <= ' ' || c == '\u{7f}') {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if url.contains(['?', '#', '@', '\\']) || rest.contains("//") || valid_host(authority).is_none() {
        return None;
    }
    Some(url.to_string())
}

/// `host` or `host:port`, or a bracketed IPv6 address with an optional port.
fn valid_host(value: &str) -> Option<&str> {
    let host = value.trim();
    let (name, port) = if let Some(rest) = host.strip_prefix('[') {
        let (address, after) = rest.split_once(']')?;
        if address.is_empty() || !address.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return None;
        }
        (address, after.strip_prefix(':'))
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        }
    };
    let name_ok = host.starts_with('[')
        || (!name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~')));
    let port_ok =
        port.is_none_or(|port| !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit()));
    (name_ok && port_ok).then_some(host)
}

fn first_value(header: Option<&str>) -> Option<&str> {
    header
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// The base URL the server is reached at: `public-url`, else the request's origin (forwarded headers only when
/// trusted), else the webroot's configured host.
pub fn base_url(factory: &ExecutionContextFactory, origin: &Origin) -> String {
    base_url_of(&crate::llm::facade(factory).conf().a2a, origin, || {
        factory.webapp_root_url_for("webroot", Some(origin.https))
    })
}

/// [`base_url`] of an `a2a` conf, with `root` the webroot's configured URL.
pub fn base_url_of(conf: &crate::llm::conf::A2aConf, origin: &Origin, root: impl FnOnce() -> String) -> String {
    if let Some(configured) = conf.public_url.as_deref().and_then(valid_base_url) {
        return configured;
    }
    let (mut scheme, mut host) = (None, None);
    // a proxy's headers only from a proxy the client-ip-header conf believes, and only when the a2a conf says so
    if conf.trust_forwarded_headers && origin.believed {
        scheme = first_value(origin.forwarded_proto.as_deref())
            .map(str::to_ascii_lowercase)
            .filter(|scheme| scheme == "http" || scheme == "https");
        host = first_value(origin.forwarded_host.as_deref())
            .and_then(valid_host)
            .map(str::to_string);
    }
    let scheme = scheme.unwrap_or_else(|| if origin.https { "https" } else { "http" }.to_string());
    match host.or_else(|| origin.host.as_deref().and_then(valid_host).map(str::to_string)) {
        Some(host) => format!("{scheme}://{host}"),
        None => valid_base_url(&root()).unwrap_or_else(|| "http://localhost".to_string()),
    }
}

fn interfaces(base: &str) -> Json {
    let url = format!("{base}/a2a");
    json!([
        {"url": url, "protocolBinding": "JSONRPC", "protocolVersion": VERSION},
        {"url": url, "protocolBinding": "HTTP+JSON", "protocolVersion": VERSION},
    ])
}

fn security(factory: &ExecutionContextFactory) -> (Json, Json) {
    let llm = crate::llm::facade(factory);
    let conf = &llm.conf().a2a.security;
    let mut schemes = serde_json::Map::new();
    let mut requirements = Vec::new();
    if conf.bearer {
        schemes.insert(
            "bearer".to_string(),
            json!({"httpAuthSecurityScheme": {"scheme": "Bearer", "description": "A Moqui login key"}}),
        );
        requirements.push(json!({"schemes": {"bearer": {"list": []}}}));
    }
    schemes.insert(
        "basic".to_string(),
        json!({"httpAuthSecurityScheme": {"scheme": "Basic", "description": "A Moqui username and app password"}}),
    );
    requirements.push(json!({"schemes": {"basic": {"list": []}}}));
    if conf.api_key {
        schemes.insert(
            "apiKey".to_string(),
            json!({"apiKeySecurityScheme": {"location": "header", "name": "api_key", "description": "A Moqui login key"}}),
        );
        requirements.push(json!({"schemes": {"apiKey": {"list": []}}}));
    }
    // no openIdConnect scheme while JWTs are not accepted (plan 11, answered question 10): `oidc-issuer` waits for it
    (Json::Object(schemes), Json::Array(requirements))
}

fn card(factory: &ExecutionContextFactory, base: &str, skills: Vec<Json>) -> Json {
    let property = |name: &str, default: &str| {
        Some(moqui_lib::env::get_prop_or_env(name).trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    let (schemes, requirements) = security(factory);
    json!({
        "name": property("a2a_agent_name", "Moqui"),
        "description": property("a2a_agent_description", "The agents of this Moqui server: they work its screens and data for you"),
        "supportedInterfaces": interfaces(base),
        "version": property("a2a_agent_version", "1.0.0"),
        "capabilities": {"streaming": true, "pushNotifications": true, "extendedAgentCard": true},
        "securitySchemes": schemes,
        "securityRequirements": requirements,
        "defaultInputModes": ["text/plain", "application/json"],
        "defaultOutputModes": ["text/plain", "application/json"],
        "skills": skills,
    })
}

fn generic_skill() -> Json {
    json!({"id": "moqui", "name": "Moqui", "description": "Answers from this server's apps and data, and proposes changes its users approve",
        "tags": ["moqui"]})
}

/// The public card, for `GET /.well-known/agent-card.json`: 404 while the server is off, else cacheable for five
/// minutes, varying by host.
pub fn public(factory: &ExecutionContextFactory, origin: &Origin) -> Reply {
    if !super::enabled(factory) {
        return Reply::json(
            404,
            &json!({"error": {"code": 404, "status": "NOT_FOUND", "message": "the A2A server is off"}}),
        );
    }
    let body = card(factory, &base_url(factory, origin), vec![generic_skill()]);
    let mut reply = Reply::json(200, &body);
    reply.headers.retain(|(name, _)| name != "cache-control");
    reply
        .with_header("cache-control", "public, max-age=300")
        .with_header("vary", "Host")
}

/// The agents a user may call over A2A: not disabled, `exposeA2a`, serving `a2a`, in id order. The caller's
/// `AgentUse` was checked when it was authenticated.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn exposed_agents(
    facade: &EntityFacade,
) -> Result<Vec<(AgentDef, crate::entity::EntityValue)>, moqui_lib::error::Error> {
    let rows = crate::llm::run::quietly(|| {
        facade
            .find("moqui.agent.AgentDefinition")?
            .order_by("agentId", false)
            .use_cache(true)
            .disable_authz()
            .list()
    })?;
    Ok(rows
        .values()
        .iter()
        .filter(|row| flag_of(row, "exposeA2a") == Some(true))
        .map(|row| (AgentDef::of(row), row.clone()))
        .filter(|(agent, _)| !agent.disabled && agent.serves(Surface::A2a))
        .collect())
}

/// The skills an agent's card entry lists: its `a2aSkillsJson` (an array of `AgentSkill`, or `{skills: [...]}`),
/// each tagged with the agent, else one skill for the agent itself. A skill entry that is not an object with an id is
/// left out and logged.
pub fn agent_skills(agent: &AgentDef, row: &crate::entity::EntityValue) -> Vec<Json> {
    let description = text_of(row, "description").unwrap_or_else(|| format!("The {} agent", agent.agent_id));
    let name = if agent.name.is_empty() {
        agent.agent_id.clone()
    } else {
        agent.name.clone()
    };
    let listed = text_of(row, "a2aSkillsJson").and_then(|text| match serde_json::from_str::<Json>(&text) {
        Ok(Json::Array(skills)) => Some(skills),
        Ok(Json::Object(mut object)) => object.remove("skills").and_then(|skills| skills.as_array().cloned()),
        Ok(_) | Err(_) => {
            tracing::error!(
                "the a2aSkillsJson of agent {} is not a JSON array of skills; it is left out",
                agent.agent_id
            );
            None
        }
    });
    let mut skills = Vec::new();
    for skill in listed.unwrap_or_default() {
        let Some(id) = skill
            .get("id")
            .and_then(Json::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            tracing::error!(
                "a skill in the a2aSkillsJson of agent {} has no id; it is left out",
                agent.agent_id
            );
            continue;
        };
        let text = |field: &str| skill.get(field).and_then(Json::as_str).map(str::to_string);
        let strings = |field: &str| -> Vec<String> {
            skill
                .get(field)
                .and_then(Json::as_array)
                .map(|items| items.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        let mut tags = strings("tags");
        tags.push(format!("agent:{}", agent.agent_id));
        skills.push(json!({"id": id, "name": text("name").unwrap_or_else(|| id.to_string()),
            "description": text("description").unwrap_or_else(|| description.clone()),
            "tags": tags, "examples": strings("examples")}));
    }
    if skills.is_empty() {
        skills.push(json!({"id": agent.agent_id, "name": name, "description": description,
            "tags": [format!("agent:{}", agent.agent_id)]}));
    }
    skills
}

/// The extended card of `caller`: the skills of the agents they may use (a generic one when there are none).
///
/// # Errors
///
/// Returns an error when the agents cannot be read.
pub fn extended(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    caller: &Caller,
) -> Result<Json, moqui_lib::error::Error> {
    let mut skills: Vec<Json> = exposed_agents(facade)?
        .iter()
        .flat_map(|(agent, row)| agent_skills(agent, row))
        .take(MAX_SKILLS)
        .collect();
    if skills.is_empty() {
        skills.push(generic_skill());
    }
    Ok(card(factory, &caller.base_url, skills))
}
