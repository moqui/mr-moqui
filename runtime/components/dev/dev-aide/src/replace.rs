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

//! Replace across files (plans/design-dev-aide.md 4.3): `replace#Files` of the Workspace. With no `files` it is the
//! preview: each file of the root a search finds (`git grep` in a repository, as Search in files; a walk of a plain
//! root), its matches with their lines before and after, its hash, and why it may not be written when the user may not
//! (an upstream file while the lock is on, a read-only root: Save's rule). With `files` (each path, the hash the
//! preview read and the matches chosen) it replaces those matches of each file through the save pipeline
//! (`workspace::save_text`: confined, audited, reloaded), each file on its own: one whose text changed since the preview
//! (its hash), or that may not be written, is refused and named, and the others are still written. Open tabs are the
//! client's: the Workspace leaves out a file with unsaved changes in a tab and reads the others again after.

use std::collections::BTreeSet;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::git;
use crate::roots::{Root, RootKind};
use crate::tier::Tier;
use crate::workspace::{self, decode, flag, json_value, sha256, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(replace_files), replace_files)
}

/// The most files a preview lists, and replaces at once.
const MAX_FILES: usize = 200;
/// The most matches a preview lists in all, and in one file.
const MAX_MATCHES: usize = 2000;
const MAX_FILE_MATCHES: usize = 200;

/// What to find and what to put in its place.
#[derive(Debug)]
pub(crate) struct Replacer {
    regex: Regex,
    /// A regular expression's replacement expands `$1` and `${name}`; a text's is taken as it is.
    expand: bool,
    replacement: String,
}

impl Replacer {
    /// `query` as text or, with `regex`, a regular expression, ignoring case unless `case_sensitive`. Refused when it
    /// does not parse or matches an empty text (it would put the replacement between every two characters).
    pub(crate) fn new(query: &str, replacement: &str, regex: bool, case_sensitive: bool) -> Result<Self, String> {
        if query.is_empty() {
            return Err("nothing to replace".into());
        }
        let pattern = if regex { query.to_string() } else { regex::escape(query) };
        let compiled = RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .map_err(|err| format!("the regular expression: {err}"))?;
        if compiled.is_match("") {
            return Err(
                "the regular expression matches an empty text, so it would replace between every character".into(),
            );
        }
        Ok(Self {
            regex: compiled,
            expand: regex,
            replacement: replacement.to_string(),
        })
    }

    /// The replacement of one match.
    fn replacement_of(&self, captures: &regex::Captures<'_>) -> String {
        if self.expand {
            let mut out = String::new();
            captures.expand(&self.replacement, &mut out);
            out
        } else {
            self.replacement.clone()
        }
    }

    /// The matches of `text`, in order, at most `limit`, and whether there were more.
    pub(crate) fn hits(&self, text: &str, limit: usize) -> (Vec<Hit>, bool) {
        let mut hits = Vec::new();
        // the line of the last match, counted on from there
        let (mut counted_to, mut line) = (0, 1);
        for captures in self.regex.captures_iter(text) {
            if hits.len() >= limit {
                return (hits, true);
            }
            let Some(found) = captures.get(0) else { continue };
            line += text[counted_to..found.start()].matches('\n').count();
            counted_to = found.start();
            let line_start = text[..found.start()].rfind('\n').map_or(0, |at| at + 1);
            let line_end = text[found.start()..]
                .find('\n')
                .map_or(text.len(), |at| found.start() + at);
            // the line after this one match is replaced; a match past its line ends the line there
            let tail = if found.end() <= line_end {
                &text[found.end()..line_end]
            } else {
                ""
            };
            hits.push(Hit {
                index: hits.len(),
                line,
                column: text[line_start..found.start()].chars().count() + 1,
                text: text[line_start..line_end].trim_end_matches('\r').to_string(),
                after: format!(
                    "{}{}{}",
                    &text[line_start..found.start()],
                    self.replacement_of(&captures),
                    tail.trim_end_matches('\r')
                ),
            });
        }
        (hits, false)
    }

    /// `text` with the matches whose index is in `chosen` replaced, and how many were.
    pub(crate) fn replace(&self, text: &str, chosen: &BTreeSet<usize>) -> (String, usize) {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut count = 0;
        for (index, captures) in self.regex.captures_iter(text).enumerate() {
            let Some(found) = captures.get(0) else { continue };
            if !chosen.contains(&index) {
                continue;
            }
            out.push_str(&text[last..found.start()]);
            out.push_str(&self.replacement_of(&captures));
            last = found.end();
            count += 1;
        }
        out.push_str(&text[last..]);
        (out, count)
    }
}

/// One match of a preview: its index in the file, its line and column (from 1), and its line before and after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Hit {
    pub index: usize,
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub after: String,
}

/// A file to replace in, as the client sends it back from the preview.
#[derive(Debug, Deserialize)]
struct Chosen {
    path: String,
    hash: String,
    matches: Vec<usize>,
}

/// The files of `root` a search for `query` finds, as Search in files finds them: `git grep -l` in a repository, every
/// file whose path has `path_filter` in a plain root.
fn candidates(
    context: &Context,
    root: &Root,
    query: &str,
    regex: bool,
    case: bool,
    filter: &str,
) -> Result<Vec<String>, LibError> {
    if root.kind != RootKind::Git {
        let filter = filter.trim_matches('*');
        return Ok(workspace::root_files(context, root, 20_000)
            .into_iter()
            .filter(|path| filter.is_empty() || path.contains(filter))
            .collect());
    }
    let mut args = vec!["grep", "-l", "-I", "--no-color", "-z", "--untracked"];
    args.push(if regex { "-E" } else { "-F" });
    if !case {
        args.push("-i");
    }
    args.extend(["-e", query, "--"]);
    if !filter.is_empty() {
        args.push(filter);
    }
    let output = context.git().run(&root.path, git::Mode::Read, &args, &[], None)?;
    // exit 1 is "no match"
    if output.code != Some(0) && output.code != Some(1) {
        return Err(LibError::new(format!("git grep: {}", output.stderr.trim())));
    }
    Ok(output
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// `replace#Files`: the preview of a replace across a root's files, or with `files` the replace of the matches chosen
/// in each, through the save pipeline. The extend tier, as Save.
pub(crate) fn replace_files(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let query = text(parameters, "query");
    let regex = flag(parameters, "regex");
    let case = flag(parameters, "caseSensitive");
    let replacer = Replacer::new(&query, &text(parameters, "replacement"), regex, case).map_err(LibError::new)?;
    let chosen: Option<Vec<Chosen>> = match parameters.get("files") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw.trim().is_empty() => None,
        Some(Value::String(raw)) => {
            Some(serde_json::from_str(raw).map_err(|err| LibError::new(format!("files: {err}")))?)
        }
        Some(value) => Some(
            serde_json::from_value(moqui_lib::json::to_json(value))
                .map_err(|err| LibError::new(format!("files: {err}")))?,
        ),
    };
    match chosen {
        None => preview(
            &context,
            root,
            &replacer,
            &query,
            regex,
            case,
            &text(parameters, "pathFilter"),
        ),
        Some(files) => apply(&context, root, &replacer, &query, regex, &files),
    }
}

fn preview(
    context: &Context,
    root: &Root,
    replacer: &Replacer,
    query: &str,
    regex: bool,
    case: bool,
    filter: &str,
) -> Result<Parameters, LibError> {
    let mut files = Vec::new();
    let mut total = 0;
    let mut truncated = false;
    for path in candidates(context, root, query, regex, case, filter)? {
        if files.len() >= MAX_FILES || total >= MAX_MATCHES {
            truncated = true;
            break;
        }
        // what an open would refuse is not shown: the runtime's denied files, its conf below the full tier
        let Ok(resolved) = context.resolve(root, &path) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&resolved.path) else {
            continue;
        };
        let Some((body, _)) = decode(&bytes) else {
            continue;
        };
        let (hits, more) = replacer.hits(&body, MAX_FILE_MATCHES.min(MAX_MATCHES - total));
        if hits.is_empty() {
            continue;
        }
        total += hits.len();
        let refused = context
            .may_write(root, &resolved.relative)
            .err()
            .map(|err| err.message().to_string());
        files.push(json!({
            "path": resolved.relative,
            "hash": sha256(&bytes),
            "matches": hits,
            "more": more,
            "refused": refused,
        }));
    }
    let mut out = Parameters::new();
    out.insert("files".into(), json_value(&files));
    out.insert(
        "matchCount".into(),
        Value::I64(i64::try_from(total).unwrap_or(i64::MAX)),
    );
    out.insert("truncated".into(), Value::Bool(truncated));
    Ok(out)
}

fn apply(
    context: &Context,
    root: &Root,
    replacer: &Replacer,
    query: &str,
    regex: bool,
    files: &[Chosen],
) -> Result<Parameters, LibError> {
    if files.len() > MAX_FILES {
        return Err(LibError::new(format!("at most {MAX_FILES} files are replaced at once")));
    }
    let mut replaced = Vec::new();
    let mut refused = Vec::new();
    for file in files {
        match apply_one(context, root, replacer, query, regex, file) {
            Ok(Some(done)) => replaced.push(done),
            Ok(None) => {}
            Err(reason) => {
                tracing::warn!(root = %root.id, path = %file.path, "replace across files left a file: {reason}");
                refused.push(json!({ "path": file.path, "reason": reason }));
            }
        }
    }
    let mut out = Parameters::new();
    out.insert("replaced".into(), json_value(&replaced));
    out.insert("refused".into(), json_value(&refused));
    Ok(out)
}

/// One file's replace: what was written, `None` when none of its matches were chosen, or why it was not.
fn apply_one(
    context: &Context,
    root: &Root,
    replacer: &Replacer,
    query: &str,
    regex: bool,
    file: &Chosen,
) -> Result<Option<serde_json::Value>, String> {
    let chosen: BTreeSet<usize> = file.matches.iter().copied().collect();
    if chosen.is_empty() {
        return Ok(None);
    }
    let resolved = context
        .resolve(root, &file.path)
        .map_err(|err| err.message().to_string())?;
    context
        .may_write(root, &resolved.relative)
        .map_err(|err| err.message().to_string())?;
    let bytes = std::fs::read(&resolved.path).map_err(|err| format!("{}: {err}", file.path))?;
    if sha256(&bytes) != file.hash {
        return Err("it changed since the preview: preview again".into());
    }
    let (body, _) = decode(&bytes).ok_or("a binary file is not replaced in")?;
    let (text, count) = replacer.replace(&body, &chosen);
    if count == 0 {
        return Ok(None);
    }
    let detail = json!({ "replace": { "query": query, "regex": regex, "count": count } });
    let out = workspace::save_text(context, root, &resolved, &text, &file.hash, Some(detail))
        .map_err(|err| err.message().to_string())?;
    if matches!(out.get("conflict"), Some(Value::Bool(true))) {
        return Err("it changed since the preview: preview again".into());
    }
    let problems = match out.get("problems") {
        Some(Value::List(list)) => list.len(),
        _ => 0,
    };
    Ok(Some(json!({
        "path": resolved.relative,
        "count": count,
        "hash": out.get("hash").map(moqui_lib::object::to_plain_string),
        "problems": problems,
    })))
}

#[cfg(test)]
#[path = "replace_tests.rs"]
mod tests;
