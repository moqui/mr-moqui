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

//! The contributor agreement in the Workspace (plans/design-dev-aide.md 21.7, P12; docs/dev/Contributions.md): for a
//! hosted repository whose agreement is on, Changes and Commit warn when the commit's author (git's `user.email` in
//! the working tree) has not signed, as the server reports it (`/rest/s1/moqui/hosting/agreement`, F18). Sign adds the
//! author's line under both lists of the repository's `AUTHORS.md` and commits that file alone, by the user. It is a
//! person's act: from a session, never a header login, and no agent tool makes it.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::review::need_hosted;
use crate::git::Mode;
use crate::tier::Tier;
use crate::vcs::{audit_git, git_root, git_write_root};
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_agreement), get_agreement)?;
    registry.service_function(moqui_core::service_location!(sign_agreement), sign_agreement)
}

/// The file a repository's agreement is signed in.
pub const AUTHORS_FILE: &str = "AUTHORS.md";
/// The headings of the two lists a signature goes under.
const LISTS: [&str; 2] = ["## Copyright Waiver", "## Grant of Patent License"];

/// Whether a legal name may go into a line of `AUTHORS.md`: two characters at least, no control character (a newline,
/// a tab, an escape, a carriage return) and no markup.
#[must_use]
pub fn legal_name_ok(name: &str) -> bool {
    name.trim().chars().count() >= 2 && !name.chars().any(|ch| ch.is_control() || matches!(ch, '<' | '>'))
}

/// The line of a signature, as `AUTHORS.md` asks for it.
#[must_use]
pub fn signature_line(years: &str, legal_name: &str, username: &str) -> String {
    format!(
        "- Written in {} by {} - {}",
        years.trim(),
        legal_name.trim(),
        username.trim()
    )
}

/// `text` (an `AUTHORS.md`) with `line` after the last signature of each of its two lists (or at the end of a list
/// with none). An error when the file has not both lists, or the line is in both already.
///
/// # Errors
///
/// When either list's heading is missing, or the signature is there already.
pub fn with_signature(text: &str, line: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut insert_at = Vec::new();
    for heading in LISTS {
        let Some(start) = lines.iter().position(|have| have.trim() == heading) else {
            return Err(format!("{AUTHORS_FILE} has no {heading} list to sign"));
        };
        let end = lines[start + 1..]
            .iter()
            .position(|have| have.starts_with("## "))
            .map_or(lines.len(), |at| start + 1 + at);
        let section = &lines[start + 1..end];
        if section.iter().any(|have| have.trim() == line.trim()) {
            insert_at.push(None);
            continue;
        }
        let last = section
            .iter()
            .rposition(|have| have.trim_start().starts_with("- Written"));
        let at = match last {
            Some(last) => start + 1 + last + 1,
            // no signature yet: before the section's trailing blank lines
            None => {
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                at
            }
        };
        insert_at.push(Some(at));
    }
    if insert_at.iter().all(Option::is_none) {
        return Err("the signature is in both lists already".into());
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    for (index, have) in lines.iter().enumerate() {
        if insert_at.contains(&Some(index)) {
            out.push(line.to_string());
        }
        out.push((*have).to_string());
    }
    if insert_at.contains(&Some(lines.len())) {
        out.push(line.to_string());
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

/// git's `user.email` (and name) in the working tree: the author of the next commit.
fn author(context: &Context, root: &crate::roots::Root) -> (String, String) {
    let read = |key: &str| {
        context
            .git()
            .read(&root.path, &["config", "--get", key])
            .map(|out| out.trim().to_string())
            .unwrap_or_default()
    };
    (read("user.email"), read("user.name"))
}

/// Whether `email` has signed the hosted repository's agreement, as the server answers it (`known`, `required`,
/// `signed`, `message`, `signUrl`, `application`).
pub(crate) fn author_signed(hosted: &super::review::Hosted, email: &str) -> Result<Json, LibError> {
    let path = Some(hosted.server.offered("agreement.path"))
        .filter(|path| path.starts_with('/'))
        .unwrap_or_else(|| format!("{}/agreement", hosted.server.rest_path("hosting")));
    let id = hosted.id();
    hosted
        .server
        .get(&path, &[("repositoryId", id.as_str()), ("authorEmail", email)])
}

/// The authors of `emails` who have not signed the hosted repository's agreement where the server decides it
/// (`known` and `required`): an agent is not given the diff of a merge request of theirs (plans/design-dev-aide.md
/// 21.8, Dogfood §10). A server that does not decide the agreement (`known` false) has none to keep.
pub(crate) fn unsigned_authors(hosted: &super::review::Hosted, emails: &[String]) -> Result<Vec<String>, LibError> {
    let mut unsigned = Vec::new();
    for email in emails {
        let answer = author_signed(hosted, email)?;
        if unsigned_in(&answer) {
            unsigned.push(email.clone());
        }
    }
    Ok(unsigned)
}

/// Whether an agreement answer says the author has not signed where signing is required.
#[must_use]
pub fn unsigned_in(answer: &Json) -> bool {
    answer["known"].as_bool() == Some(true)
        && answer["required"].as_bool() == Some(true)
        && answer["signed"].as_bool() != Some(true)
}

/// `get#Agreement`: whether the next commit's author (git's `user.email`, or `authorEmail`) has signed the hosted
/// repository's agreement, as the server reports it: `known` (the server decides it), `required`, `signed`,
/// `signUrl`, and what Sign would add (`line` with the user's account on the server).
fn get_agreement(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let mut out = Parameters::new();
    let Some(hosted) = super::review::hosted(&context, root) else {
        out.insert("known".into(), Value::Bool(false));
        return Ok(out);
    };
    let (email, name) = author(&context, root);
    let email = Some(text(parameters, "authorEmail"))
        .filter(|email| !email.is_empty())
        .unwrap_or(email);
    let answer = author_signed(&hosted, &email)?;
    let year = moqui_core::user::now_value();
    let year = moqui_lib::object::to_plain_string(&year)
        .chars()
        .take(4)
        .collect::<String>();
    out.insert("known".into(), Value::Bool(answer["known"].as_bool() == Some(true)));
    out.insert(
        "required".into(),
        Value::Bool(answer["required"].as_bool() == Some(true)),
    );
    out.insert("signed".into(), Value::Bool(answer["signed"].as_bool() == Some(true)));
    out.insert("authorEmail".into(), Value::string(&email));
    out.insert(
        "signUrl".into(),
        json_value(&super::review::page_url(&hosted.server, &answer["signUrl"])),
    );
    out.insert("message".into(), json_value(&answer["message"]));
    out.insert("application".into(), json_value(&answer["application"]));
    out.insert(
        "hasAuthorsFile".into(),
        Value::Bool(root.path.join(AUTHORS_FILE).is_file()),
    );
    out.insert(
        "line".into(),
        Value::string(signature_line(&year, &name, &hosted.server.username)),
    );
    Ok(out)
}

/// `sign#Agreement`: the user's signature (`legalName`, `years`, their account on the server) added under both lists
/// of the working tree's `AUTHORS.md`, committed alone, by the user. Refused while `AUTHORS.md` has changes of its
/// own, and from a request logged in by a header.
fn sign_agreement(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    // a person signs: never under an agent's fence or in a sim (try_service), where it would still be real
    crate::workspace::unfenced(moqui_core::entity::fence::Effect::Process, "sign#Agreement")?;
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "A person signs, from a session: not a request logged in by a header",
        ));
    }
    let _authz = ec.disable_authz();
    let root = git_write_root(&context, parameters, "signs the agreement")?;
    let hosted = need_hosted(&context, root)?;
    let legal_name = text(parameters, "legalName");
    if !legal_name_ok(&legal_name) {
        return Err(LibError::new("your legal name is required"));
    }
    let username = Some(text(parameters, "username"))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| hosted.server.username.clone());
    if username.trim().is_empty() || username.contains(char::is_whitespace) {
        return Err(LibError::new("your account on the server is required"));
    }
    let years = Some(text(parameters, "years"))
        .filter(|years| !years.trim().is_empty())
        .unwrap_or_else(|| {
            moqui_lib::object::to_plain_string(&moqui_core::user::now_value())
                .chars()
                .take(4)
                .collect()
        });
    if !years
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '-' | ' '))
    {
        return Err(LibError::new("years are numbers, such as 2026 or 2025-2026"));
    }
    let file = root.path.join(AUTHORS_FILE);
    let before = std::fs::read_to_string(&file)
        .map_err(|_| LibError::new(format!("{} has no {AUTHORS_FILE} to sign", root.label)))?;
    let changed = context
        .git()
        .read(&root.path, &["status", "--porcelain", "--", AUTHORS_FILE])
        .unwrap_or_default();
    if !changed.trim().is_empty() {
        return Err(LibError::new(format!(
            "{AUTHORS_FILE} has changes of yours: commit or discard them first, so the signature is committed alone"
        )));
    }
    let line = signature_line(&years, &legal_name, &username);
    let after = with_signature(&before, &line).map_err(LibError::new)?;
    crate::workspace::write_atomic(&file, after.as_bytes())?;
    // only the file, whatever else is staged
    let committed = context.git().run(
        &root.path,
        Mode::Write,
        &["commit", "-m", "Sign AUTHORS.md", "--", AUTHORS_FILE],
        &[],
        None,
    )?;
    if !committed.success() {
        let _ = std::fs::write(&file, before.as_bytes());
        return Err(LibError::new(format!(
            "git commit: {}{}",
            committed.stdout.trim(),
            committed.stderr.trim()
        )));
    }
    let head = context
        .git()
        .read(&root.path, &["rev-parse", "HEAD"])
        .map(|out| out.trim().to_string())
        .unwrap_or_default();
    audit_git(
        &context,
        root,
        "sign",
        json!({ "repository": hosted.id(), "commit": head }),
    );
    let mut out = Parameters::new();
    out.insert("commitId".into(), Value::string(head));
    out.insert("line".into(), Value::string(line));
    out.insert("signed".into(), json_value(&Json::Bool(true)));
    Ok(out)
}
