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
//! preview: each file of the root (what git tracks and the untracked files it does not ignore, or a walk of a plain
//! root) that the query matches, with Rust's regular expressions alone so the preview and the replace read the query
//! one way; its matches with their lines before and after, its hash, and why it may not be written when the user may
//! not (an upstream file while the lock is on, a read-only root: Save's rule). A file over `max-edit-bytes` is not
//! read, as an editor does not open it to edit. With `files` (each path, the hash the preview read and the matches
//! chosen) it replaces those matches of each file through the save pipeline (`workspace::save_text_with`: confined,
//! audited, reloaded), its line endings as they were, each file on its own: one whose text changed since the preview
//! (its hash), that may not be written, or whose result would be over `max-edit-bytes`, is refused and named, and the
//! others are still written. Open tabs are the client's: the Workspace leaves out a file with unsaved changes in a
//! tab and reads the others again after.

use std::collections::BTreeSet;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::roots::Root;
use crate::tier::Tier;
use crate::workspace::{self, decode, flag, json_value, sha256, text, Context, Endings};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(replace_files), replace_files)
}

/// The most files a preview lists, and replaces at once.
const MAX_FILES: usize = 200;
/// The most matches a preview lists in all, and in one file.
const MAX_MATCHES: usize = 2000;
const MAX_FILE_MATCHES: usize = 200;
/// The most files of a root a preview looks in.
const MAX_SCANNED: usize = 20_000;
/// The longest query and replacement taken, in bytes.
pub(crate) const MAX_QUERY_BYTES: usize = 4 * 1024;
pub(crate) const MAX_REPLACEMENT_BYTES: usize = 64 * 1024;
/// The most characters of a line a preview shows, before and after.
pub(crate) const SHOWN_CHARS: usize = 400;

/// What to find and what to put in its place.
#[derive(Debug)]
pub(crate) struct Replacer {
    regex: Regex,
    /// A regular expression's replacement expands `$1` and `${name}`; a text's is taken as it is.
    expand: bool,
    replacement: String,
    /// The group references of an expanding replacement (`${1}`, `$name`): each puts at most its match's length in.
    group_refs: usize,
}

/// The group references of a replacement as the regex crate expands it: each `$` that is not `$$`.
pub(crate) fn group_refs(replacement: &str) -> usize {
    let mut count = 0;
    let mut chars = replacement.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
            }
            Some(next) if *next == '{' || *next == '_' || next.is_alphanumeric() => count += 1,
            _ => {}
        }
    }
    count
}

/// A replacement's `$1` as `${1}`, so the group's number ends at its last digit (`$1a` is group 1, then `a`) where
/// the regex crate reads `$1a` as a group named `1a`; `$$` and `${...}` are left as they are.
pub(crate) fn numbered_groups(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                out.push_str("$$");
                chars.next();
            }
            Some(digit) if digit.is_ascii_digit() => {
                out.push_str("${");
                while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
                    out.push(digit);
                    chars.next();
                }
                out.push('}');
            }
            _ => out.push('$'),
        }
    }
    out
}

/// The regular expression a query is, as Replace and Search in files read it: the text itself, or with `regex` a
/// regular expression over the whole text (`^` and `$` at each line, a CRLF line's too, `\n` across lines), ignoring
/// case unless `case_sensitive`.
pub(crate) fn compile(query: &str, regex: bool, case_sensitive: bool) -> Result<Regex, String> {
    let pattern = if regex { query.to_string() } else { regex::escape(query) };
    RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .multi_line(true)
        .crlf(true)
        .build()
        .map_err(|err| format!("the regular expression: {err}"))
}

/// The lines of `text` a match starts on, each once and in order, at most `limit`: its number from 1 and the line
/// (cut at [`SHOWN_CHARS`]); an empty match is passed over, as Replace passes it over.
pub(crate) fn matched_lines(regex: &Regex, text: &str, limit: usize) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let (mut counted_to, mut line) = (0, 1);
    for found in regex.find_iter(text).filter(|found| !found.is_empty()) {
        line += text[counted_to..found.start()].matches('\n').count();
        counted_to = found.start();
        if out.last().is_some_and(|(last, _)| *last == line) {
            continue;
        }
        if out.len() >= limit {
            break;
        }
        let start = text[..found.start()].rfind('\n').map_or(0, |at| at + 1);
        let end = text[found.start()..]
            .find('\n')
            .map_or(text.len(), |at| found.start() + at);
        out.push((line, shown(text[start..end].trim_end_matches('\r'), SHOWN_CHARS)));
    }
    out
}

/// At most `most` characters of `text`, with an ellipsis when it had more.
fn shown(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

impl Replacer {
    /// `query` as text or, with `regex`, a regular expression, ignoring case unless `case_sensitive`. Refused when it
    /// or the replacement is too long, it does not parse, or it matches an empty text (it would put the replacement
    /// between every two characters); an empty match it has anyway (at `\b`) is passed over.
    pub(crate) fn new(query: &str, replacement: &str, regex: bool, case_sensitive: bool) -> Result<Self, String> {
        if query.is_empty() {
            return Err("nothing to replace".into());
        }
        if query.len() > MAX_QUERY_BYTES {
            return Err(format!("the text to find is over {MAX_QUERY_BYTES} bytes"));
        }
        if replacement.len() > MAX_REPLACEMENT_BYTES {
            return Err(format!("the replacement is over {MAX_REPLACEMENT_BYTES} bytes"));
        }
        let compiled = compile(query, regex, case_sensitive)?;
        if compiled.is_match("") {
            return Err(
                "the regular expression matches an empty text, so it would replace between every character".into(),
            );
        }
        let replacement = if regex {
            numbered_groups(replacement)
        } else {
            replacement.to_string()
        };
        Ok(Self {
            regex: compiled,
            expand: regex,
            group_refs: if regex { group_refs(&replacement) } else { 0 },
            replacement,
        })
    }

    /// Whether the query matches somewhere in `text` (an empty match is no match).
    pub(crate) fn finds(&self, text: &str) -> bool {
        self.matches(text).next().is_some()
    }

    /// The matches of `text` in order, empty ones passed over: what a preview's indexes count.
    fn matches<'t>(&'t self, text: &'t str) -> impl Iterator<Item = regex::Captures<'t>> + 't {
        self.regex
            .captures_iter(text)
            .filter(|captures| captures.get(0).is_some_and(|found| !found.is_empty()))
    }

    /// The most bytes the replacement of a match of `len` bytes can be: the replacement's own and, for each of its
    /// group references, the match again (a group is at most its match).
    fn replacement_bound(&self, len: usize) -> usize {
        self.replacement
            .len()
            .saturating_add(self.group_refs.saturating_mul(len))
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

    /// The matches of `text`, in order, at most `limit`, and whether there were more; each line shown is at most
    /// [`SHOWN_CHARS`] characters.
    pub(crate) fn hits(&self, text: &str, limit: usize) -> (Vec<Hit>, bool) {
        let mut hits = Vec::new();
        // the line of the last match, counted on from there
        let (mut counted_to, mut line) = (0, 1);
        for captures in self.matches(text) {
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
            let before = &text[line_start..found.start()];
            // a replacement that could be longer than a shown line is shown as written, not expanded
            let replaced = if self.replacement_bound(found.len()) > SHOWN_CHARS * 4 {
                self.replacement.clone()
            } else {
                self.replacement_of(&captures)
            };
            hits.push(Hit {
                index: hits.len(),
                line,
                column: before.chars().count() + 1,
                text: shown(text[line_start..line_end].trim_end_matches('\r'), SHOWN_CHARS),
                after: shown(
                    &format!("{before}{replaced}{}", tail.trim_end_matches('\r')),
                    SHOWN_CHARS,
                ),
            });
        }
        (hits, false)
    }

    /// `text` with the matches whose index is in `chosen` replaced, and how many were; refused once the text would be
    /// over `max_bytes`.
    pub(crate) fn replace(
        &self,
        text: &str,
        chosen: &BTreeSet<usize>,
        max_bytes: u64,
    ) -> Result<(String, usize), String> {
        let too_long = || format!("the text would be over {max_bytes} bytes");
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut count = 0;
        for (index, captures) in self.matches(text).enumerate() {
            let Some(found) = captures.get(0) else { continue };
            if !chosen.contains(&index) {
                continue;
            }
            out.push_str(&text[last..found.start()]);
            // refused before it is expanded when it could take the text past the bound
            if out.len().saturating_add(self.replacement_bound(found.len())) as u64 > max_bytes {
                return Err(too_long());
            }
            out.push_str(&self.replacement_of(&captures));
            last = found.end();
            count += 1;
            if out.len() as u64 > max_bytes {
                return Err(too_long());
            }
        }
        out.push_str(&text[last..]);
        if out.len() as u64 > max_bytes {
            return Err(too_long());
        }
        Ok((out, count))
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

/// The most bytes, and the longest time, one search or preview reads files for.
pub(crate) const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_SCAN_TIME: std::time::Duration = std::time::Duration::from_secs(10);

/// A file a scan read: its path in the root and its text.
pub(crate) struct Scanned<'a> {
    pub relative: &'a str,
    pub hash: String,
    pub body: &'a str,
}

/// Each file of `root` whose path has `filter` (the filter before the cap of [`MAX_SCANNED`]), as Search in files
/// and Replace read them: what git tracks and the untracked files it does not ignore, or a walk of a plain root;
/// confined as an open is; text up to `max-edit-bytes`, a file an editor would not open to edit passed over; at most
/// [`MAX_READ_BYTES`] in all and for [`MAX_SCAN_TIME`]. `each` answers whether to go on. Whether the scan stopped
/// short: the cap, the budget, or `each`.
pub(crate) fn scan_files(
    context: &Context,
    root: &Root,
    filter: &str,
    each: &mut dyn FnMut(&Scanned<'_>) -> bool,
) -> bool {
    let filter = filter.trim_matches('*');
    let (paths, capped) = workspace::root_files_where(context, root, MAX_SCANNED, &|path| {
        filter.is_empty() || path.contains(filter)
    });
    let started = std::time::Instant::now();
    let mut read: u64 = 0;
    for path in paths {
        if read > MAX_READ_BYTES || started.elapsed() > MAX_SCAN_TIME {
            return true;
        }
        // what an open would refuse is not read: the runtime's denied files, its conf below the full tier
        let Ok(resolved) = context.resolve(root, &path) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&resolved.path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > context.settings.max_edit_bytes {
            continue;
        }
        let Ok(bytes) = std::fs::read(&resolved.path) else {
            continue;
        };
        read += bytes.len() as u64;
        let Some((body, _)) = decode(&bytes) else {
            continue;
        };
        let file = Scanned {
            relative: &resolved.relative,
            hash: sha256(&bytes),
            body: &body,
        };
        if !each(&file) {
            return true;
        }
    }
    capped
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
    if root.hosted_branch.is_some() {
        // a hosted root has no working tree to replace across (crate::hosted): its files are edited one Save at a time
        return Err(LibError::new(format!(
            "{} is a hosted repository: Replace across files is not offered there",
            root.label
        )));
    }
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
        None => Ok(preview(&context, root, &replacer, &text(parameters, "pathFilter"))),
        Some(files) => apply(&context, root, &replacer, &query, regex, &files),
    }
}

fn preview(context: &Context, root: &Root, replacer: &Replacer, filter: &str) -> Parameters {
    let mut files = Vec::new();
    let mut total = 0;
    let stopped = scan_files(context, root, filter, &mut |file| {
        if files.len() >= MAX_FILES || total >= MAX_MATCHES {
            return false;
        }
        if !replacer.finds(file.body) {
            return true;
        }
        let (hits, more) = replacer.hits(file.body, MAX_FILE_MATCHES.min(MAX_MATCHES - total));
        if hits.is_empty() {
            return true;
        }
        total += hits.len();
        let refused = context
            .may_write(root, file.relative)
            .err()
            .map(|err| err.message().to_string());
        files.push(json!({
            "path": file.relative,
            "hash": file.hash,
            "matches": hits,
            "more": more,
            "refused": refused,
        }));
        true
    });
    let truncated = stopped || files.len() >= MAX_FILES || total >= MAX_MATCHES;
    let mut out = Parameters::new();
    out.insert("files".into(), json_value(&files));
    out.insert(
        "matchCount".into(),
        Value::I64(i64::try_from(total).unwrap_or(i64::MAX)),
    );
    out.insert("truncated".into(), Value::Bool(truncated));
    out
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
    let (text, count) = replacer.replace(&body, &chosen, context.settings.max_edit_bytes)?;
    if count == 0 {
        return Ok(None);
    }
    let detail = json!({ "replace": { "query": query, "regex": regex, "count": count } });
    // the line endings as the file has them, mixed ones too: the replace changes only what it matched
    let out = workspace::save_text_with(
        context,
        root,
        &resolved,
        &text,
        &file.hash,
        Some(detail),
        Endings::AsGiven,
    )
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
