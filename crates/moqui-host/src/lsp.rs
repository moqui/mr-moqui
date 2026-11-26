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

//! `moqui lsp` (plans/design-dev-aide.md 13.6, F13): a Language Server Protocol server over stdio for editors and
//! coding agents, answering from what `moqui check` and `moqui refs` use: diagnostics (the checks of
//! `moqui_devkit::check`, on each open file's text as it changes), go to definition and references (the artifact
//! index, read into memory on a model-only boot and kept current with the open buffers), hover (an artifact's summary,
//! or an element's or attribute's documentation from the XSD digest), completion (the elements and attributes the
//! digest gives, and entity and service names from the index in their attributes), document and workspace symbols, and
//! on the local index rename (with `prepareRename`, `lsp_rename.rs`) and the check rules' quick fixes (`lsp_edit.rs`):
//! the nearest known names, and "Extend in my-site" for an in-place edit of an upstream component (`lsp_extend.rs`,
//! which also gives its `upstream.edit` diagnostics).
//! With `--server URL` the checks and the index are a running dev instance's, over its MCP tools `check` and
//! `artifact`, with the login key `moqui tui --server` keeps (or `MOQUI_LOGIN_KEY`); rename is refused there, and the
//! quick fixes find names with the instance's `artifact` tool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types as lsp;
use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::index::{self, Direction, MemoryIndex};
use moqui_devkit::{check, schema, Problem, Severity};
use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use crate::devtools_remote::{Remote, Target};

#[path = "lsp_edit.rs"]
mod edit;
#[path = "lsp_extend.rs"]
mod extend;
#[path = "lsp_rename.rs"]
mod rename;

/// What answers the server: this checkout's model and index, or a running instance's.
pub enum Backend {
    Local {
        factory: Arc<ExecutionContextFactory>,
        index: Box<MemoryIndex>,
    },
    Remote(Remote),
}

impl Backend {
    fn problems(&mut self, path: &Path, text: &str) -> Vec<Problem> {
        match self {
            Self::Local { factory, index } => {
                if let Some(located) = index::locate(factory, path) {
                    index.update_text(factory, &located, text);
                }
                let mut problems = check::check_text(factory, path, text);
                problems.extend(extend::problems(factory, path, text));
                problems
            }
            Self::Remote(remote) => remote
                .check(path, text)
                .unwrap_or_else(|err| vec![problem_of(path, &format!("the server's check failed: {err}"))]),
        }
    }

    /// The artifact a name at `line` of `path` refers to (or the one defined there).
    fn at(&self, path: &Path, line: u32, word: &str) -> Option<Target> {
        match self {
            Self::Local { index, .. } => {
                let (_, artifact) = index.at(&path.display().to_string(), line, word)?;
                artifact.map(|artifact| target_of(index, &artifact))
            }
            Self::Remote(remote) => remote.at(path, line, word),
        }
    }

    /// Where the artifact `id` is used.
    fn uses(&self, id: &str) -> Vec<(PathBuf, u32)> {
        match self {
            Self::Local { index, .. } => index
                .refs(id, Direction::In)
                .into_iter()
                .map(|entry| (PathBuf::from(entry.path), entry.line))
                .collect(),
            Self::Remote(remote) => remote.uses(id),
        }
    }

    fn find(&self, text: &str, kinds: &[String], limit: usize) -> Vec<Target> {
        match self {
            Self::Local { index, .. } => index
                .find(text, kinds, limit)
                .iter()
                .map(|artifact| target_of(index, artifact))
                .collect(),
            Self::Remote(remote) => remote.find(text, kinds, limit),
        }
    }

    /// What a rename at the 0-based `line` and UTF-16 `character` of `path` (its text `text`) would rename, with the
    /// span of its name, or why none: the local index's artifact the attribute or string under the cursor names.
    fn rename_target(
        &self,
        path: &Path,
        text: &str,
        line: u32,
        character: u32,
    ) -> Result<(rename::RenameTarget, rename::Span), String> {
        let Self::Local { index, .. } = self else {
            return Err("moqui lsp renames on the local index only, not with --server".to_string());
        };
        rename::target_at(index, &path.display().to_string(), text, line, character)
    }

    /// The edits of renaming `target` to `new_name`, or why the rename is refused.
    fn rename(
        &self,
        target: &rename::RenameTarget,
        new_name: &str,
        text_of: &dyn Fn(&Path) -> Option<String>,
        open: &[PathBuf],
    ) -> Result<std::collections::BTreeMap<PathBuf, Vec<edit::Edit>>, String> {
        match self {
            Self::Local { factory, index } => rename::rename_edits(factory, index, target, new_name, text_of, open),
            Self::Remote(_) => Err("moqui lsp renames on the local index only, not with --server".to_string()),
        }
    }

    /// The known names nearest a name a check says is unknown (at most three).
    fn nearest(&self, unknown: &edit::UnknownName) -> Vec<String> {
        match self {
            Self::Local { index, .. } => edit::nearest(unknown, index),
            Self::Remote(remote) => {
                // the server's find by the name's last segment, ranked here
                let (kinds, name): (&[&str], &str) = match unknown {
                    edit::UnknownName::Entity(name) => (&["entity", "view-entity"], name),
                    edit::UnknownName::Service(name) => (&["service"], name),
                    _ => return Vec::new(),
                };
                let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_string()).collect();
                let segment = name.rsplit(['.', '#']).next().unwrap_or(name);
                let short = !name.contains('.');
                let found = remote
                    .find(segment, &kinds, 50)
                    .into_iter()
                    .map(|target| {
                        if short && target.kind != "service" {
                            target.name.rsplit('.').next().unwrap_or(&target.name).to_string()
                        } else {
                            target.name
                        }
                    })
                    .collect();
                edit::rank_nearest(name, found)
            }
        }
    }

    /// The "Extend in my-site" rewrite of an upstream file's in-place edit, when it is mechanical: the committed text
    /// it goes back to, what it added, the site's files with the extension, and the site component's name. The local
    /// index only, as the site component comes from this boot's conf.
    fn extension(&self, path: &Path, text: &str, text_of: &dyn Fn(&Path) -> Option<String>) -> Option<Extension> {
        let Self::Local { factory, .. } = self else {
            return None;
        };
        let upstream = extend::upstream_of(factory, path)?;
        let head = extend::committed_text(path)?;
        let found = extend::diff(&upstream, text, &head)?;
        let site_edits = extend::site_edits(&found, text_of)?;
        Some(Extension {
            reverted: extend::reverted(text, &head),
            added: found.added,
            site_edits,
            site: upstream.site,
        })
    }

    fn digest(&self, root: &str) -> Option<Arc<schema::Digest>> {
        match self {
            Self::Local { factory, .. } => schema::digest(&check::xsd_dir(factory), root),
            // the XSDs of this checkout, when it is one
            Self::Remote(_) => {
                let (_, framework) = crate::runtime_framework_dirs();
                schema::digest(&framework.join("xsd"), root)
            }
        }
    }
}

/// An upstream file's in-place edit as the site component's extension (`lsp_extend.rs`).
struct Extension {
    /// The file's committed text, in its line endings.
    reverted: String,
    added: Vec<extend::Added>,
    site_edits: Vec<extend::SiteEdit>,
    site: String,
}

/// The target of `artifact`; a derived one (an entity-auto service or its parameter, no file's) is at its entity.
fn target_of(index: &MemoryIndex, artifact: &index::ArtifactRow) -> Target {
    let mut place = std::borrow::Cow::Borrowed(artifact);
    while place.origin == index::extract::ORIGIN_DERIVED {
        match place.parent.as_deref().and_then(|parent| index.describe(parent)) {
            Some(parent) => place = std::borrow::Cow::Owned(parent),
            None => break,
        }
    }
    Target {
        id: artifact.id.clone(),
        kind: artifact.kind.clone(),
        name: artifact.name.clone(),
        path: PathBuf::from(&place.path),
        line: place.line,
        summary: artifact.summary.clone(),
    }
}

fn problem_of(path: &Path, message: &str) -> Problem {
    Problem {
        path: path.display().to_string(),
        line: 0,
        column: 0,
        severity: Severity::Error,
        rule: "lsp.server".into(),
        message: message.into(),
        skill: None,
    }
}

// ---- positions ----

/// The path of a `file://` URI.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        // the two hex digits by bytes: a `%` before a character of several bytes is not a slice of it
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = |byte: u8| char::from(byte).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[at + 1]), hex(bytes[at + 2])) {
                out.push(u8::try_from(high * 16 + low).unwrap_or(b'%'));
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    let path = String::from_utf8(out).ok()?;
    // file:///C:/x on Windows: the character before the drive letter dropped
    let path = match path.split_at_checked(1) {
        Some((_, drive)) if drive.as_bytes().get(1) == Some(&b':') => drive.to_string(),
        _ => path,
    };
    Some(PathBuf::from(path))
}

/// The `file://` URI of a path.
pub fn uri_of(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    let encoded: String = text
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

fn uri(text: &str) -> lsp::Uri {
    text.parse().unwrap_or_else(|_| "file:///".parse().expect("a uri"))
}

/// The word under a position (0-based line and UTF-16 character): an attribute value's or a name's characters.
pub(crate) fn word_at(text: &str, line: u32, character: u32) -> String {
    word_span(text, line, character).2
}

/// [`word_at`] with where the word is on its line: its start and end UTF-16 characters.
pub(crate) fn word_span(text: &str, line: u32, character: u32) -> (u32, u32, String) {
    let Some(line) = text.lines().nth(line as usize) else {
        return (character, character, String::new());
    };
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0usize;
    let mut units = 0u32;
    while at < chars.len() && units < character {
        units += u32::try_from(chars[at].len_utf16()).unwrap_or(1);
        at += 1;
    }
    let part = |ch: char| ch.is_alphanumeric() || matches!(ch, '.' | '#' | '_' | '-' | ':' | '/' | '$');
    let mut start = at;
    while start > 0 && part(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < chars.len() && part(chars[end]) {
        end += 1;
    }
    let units_of = |part: &[char]| {
        part.iter()
            .map(|ch| u32::try_from(ch.len_utf16()).unwrap_or(1))
            .sum::<u32>()
    };
    (
        units_of(&chars[..start]),
        units_of(&chars[..end]),
        chars[start..end].iter().collect(),
    )
}

/// The tag the cursor is in: its element's name, and where in the tag the cursor is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TagContext {
    pub(crate) element: String,
    pub(crate) place: TagPlace,
}

/// Where in a tag the cursor is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TagPlace {
    /// In the element's name.
    Name,
    /// Among the attributes, outside a value.
    Attributes,
    /// In the value of this attribute.
    Value(String),
}

/// Where the cursor is in a tag: the element's name, and the attribute whose value it is in, if any.
pub(crate) fn tag_context(text: &str, line: u32, character: u32) -> Option<TagContext> {
    let offset = text
        .split_inclusive('\n')
        .take(line as usize)
        .map(str::len)
        .sum::<usize>()
        + text.lines().nth(line as usize).map_or(0, |line_text| {
            let mut units = 0u32;
            let mut bytes = 0usize;
            for ch in line_text.chars() {
                if units >= character {
                    break;
                }
                units += u32::try_from(ch.len_utf16()).unwrap_or(1);
                bytes += ch.len_utf8();
            }
            bytes
        });
    let before = &text[..offset.min(text.len())];
    let open = before.rfind('<')?;
    if before[open..].contains('>') {
        return None;
    }
    let tag = &before[open + 1..];
    let element: String = tag.chars().take_while(|ch| !ch.is_whitespace()).collect();
    if element.starts_with('/') || element.starts_with('!') || element.starts_with('?') {
        return None;
    }
    // inside a value: an odd number of quotes since the last `=`
    let quotes = tag.matches('"').count();
    if quotes % 2 == 1 {
        let open_quote = tag.rfind('"')?;
        let attribute: String = tag[..open_quote]
            .trim_end_matches('=')
            .rsplit(|ch: char| ch.is_whitespace())
            .next()
            .unwrap_or("")
            .to_string();
        return Some(TagContext {
            element,
            place: TagPlace::Value(attribute),
        });
    }
    let place = if tag.contains(char::is_whitespace) {
        TagPlace::Attributes
    } else {
        TagPlace::Name
    };
    Some(TagContext { element, place })
}

fn range(line: u32, column: u32, length: u32) -> lsp::Range {
    let line = line.saturating_sub(1);
    let column = column.saturating_sub(1);
    lsp::Range::new(
        lsp::Position::new(line, column),
        lsp::Position::new(line, column.saturating_add(length.max(1))),
    )
}

fn diagnostic(problem: &Problem, text: &str) -> lsp::Diagnostic {
    let length = text
        .lines()
        .nth(problem.line.saturating_sub(1) as usize)
        .map_or(1, |line| {
            let from = problem.column.saturating_sub(1) as usize;
            let rest: String = line.chars().skip(from).collect();
            u32::try_from(rest.chars().take_while(|ch| !ch.is_whitespace() && *ch != '>').count()).unwrap_or(1)
        });
    let severity = match problem.severity {
        Severity::Error => lsp::DiagnosticSeverity::ERROR,
        Severity::Warning => lsp::DiagnosticSeverity::WARNING,
        Severity::Info => lsp::DiagnosticSeverity::INFORMATION,
    };
    let message = match &problem.skill {
        Some(skill) => format!("{} [{}, {skill}]", problem.message, problem.rule),
        None => format!("{} [{}]", problem.message, problem.rule),
    };
    lsp::Diagnostic {
        range: range(problem.line.max(1), problem.column.max(1), length),
        severity: Some(severity),
        code: Some(lsp::NumberOrString::String(problem.rule.clone())),
        source: Some("moqui".into()),
        message,
        ..Default::default()
    }
}

fn location(target_path: &Path, line: u32) -> lsp::Location {
    lsp::Location::new(uri(&uri_of(target_path)), range(line.max(1), 1, 0))
}

/// The kinds of artifact an attribute's value names, for completion from the index.
fn names_kind(element: &str, attribute: &str) -> Option<&'static [&'static str]> {
    match (element, attribute) {
        (_, "entity-name" | "related" | "related-entity-name") => Some(&["entity", "view-entity"]),
        ("service-call", "name") | (_, "service-name" | "service") => Some(&["service"]),
        _ => None,
    }
}

// ---- the server ----

/// The open documents and what answers.
pub struct Server {
    backend: Backend,
    documents: HashMap<String, String>,
    /// Each open document's version, as the client numbers it.
    versions: HashMap<String, i64>,
    /// Whether the client applies a `WorkspaceEdit`'s `documentChanges` with a `create` operation (its `initialize`
    /// capabilities), which Extend in my-site needs.
    edits_create_files: bool,
}

/// A request's document and its 0-based line and UTF-16 character.
struct TextPosition {
    uri: String,
    line: u32,
    character: u32,
}

impl Server {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            documents: HashMap::new(),
            versions: HashMap::new(),
            edits_create_files: false,
        }
    }

    /// Take the client's `initialize` parameters: what its `WorkspaceEdit`s may hold.
    pub fn set_client(&mut self, initialize: &Json) {
        let edit = &initialize["capabilities"]["workspace"]["workspaceEdit"];
        self.edits_create_files = edit["documentChanges"].as_bool() == Some(true)
            && edit["resourceOperations"]
                .as_array()
                .is_some_and(|operations| operations.iter().any(|operation| operation == "create"));
    }

    /// Keep the version a notification gives its document.
    fn keep_version(&mut self, uri_text: &str, params: &Json) {
        if let Some(version) = params["textDocument"]["version"].as_i64() {
            self.versions.insert(uri_text.to_string(), version);
        }
    }

    pub fn capabilities() -> lsp::ServerCapabilities {
        lsp::ServerCapabilities {
            text_document_sync: Some(lsp::TextDocumentSyncCapability::Kind(lsp::TextDocumentSyncKind::FULL)),
            hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
            completion_provider: Some(lsp::CompletionOptions {
                trigger_characters: Some(vec!["<".into(), " ".into(), "\"".into()]),
                ..Default::default()
            }),
            definition_provider: Some(lsp::OneOf::Left(true)),
            references_provider: Some(lsp::OneOf::Left(true)),
            document_symbol_provider: Some(lsp::OneOf::Left(true)),
            workspace_symbol_provider: Some(lsp::OneOf::Left(true)),
            rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
            })),
            code_action_provider: Some(lsp::CodeActionProviderCapability::Options(lsp::CodeActionOptions {
                code_action_kinds: Some(vec![lsp::CodeActionKind::QUICKFIX]),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    /// The diagnostics of a document, as a `textDocument/publishDiagnostics` notification.
    fn publish(&mut self, uri_text: &str) -> Option<Notification> {
        let text = self.documents.get(uri_text)?.clone();
        let path = path_of(uri_text)?;
        let is_moqui = path.extension().is_some_and(|ext| ext == "xml");
        let problems = if is_moqui {
            self.backend.problems(&path, &text)
        } else {
            Vec::new()
        };
        let params = lsp::PublishDiagnosticsParams {
            uri: uri(uri_text),
            diagnostics: problems.iter().map(|problem| diagnostic(problem, &text)).collect(),
            version: None,
        };
        Some(Notification::new("textDocument/publishDiagnostics".into(), params))
    }

    /// A notification: open, change, save and close keep the documents and answer their diagnostics.
    pub fn notify(&mut self, note: &Notification) -> Vec<Notification> {
        let params = &note.params;
        let uri_text = params["textDocument"]["uri"].as_str().unwrap_or_default().to_string();
        match note.method.as_str() {
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str().unwrap_or_default().to_string();
                self.documents.insert(uri_text.clone(), text);
                self.keep_version(&uri_text, params);
                self.publish(&uri_text).into_iter().collect()
            }
            "textDocument/didChange" => {
                if let Some(text) = params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.last())
                    .and_then(|change| change["text"].as_str())
                {
                    self.documents.insert(uri_text.clone(), text.to_string());
                }
                self.keep_version(&uri_text, params);
                self.publish(&uri_text).into_iter().collect()
            }
            "textDocument/didSave" => {
                if let Some(text) = params["text"].as_str() {
                    self.documents.insert(uri_text.clone(), text.to_string());
                }
                self.publish(&uri_text).into_iter().collect()
            }
            "textDocument/didClose" => {
                self.documents.remove(&uri_text);
                self.versions.remove(&uri_text);
                vec![Notification::new(
                    "textDocument/publishDiagnostics".into(),
                    lsp::PublishDiagnosticsParams {
                        uri: uri(&uri_text),
                        diagnostics: Vec::new(),
                        version: None,
                    },
                )]
            }
            _ => Vec::new(),
        }
    }

    fn position(params: &Json) -> TextPosition {
        TextPosition {
            uri: params["textDocument"]["uri"].as_str().unwrap_or_default().to_string(),
            line: u32::try_from(params["position"]["line"].as_u64().unwrap_or(0)).unwrap_or(0),
            character: u32::try_from(params["position"]["character"].as_u64().unwrap_or(0)).unwrap_or(0),
        }
    }

    fn text_of(&self, uri_text: &str) -> String {
        self.documents
            .get(uri_text)
            .cloned()
            .or_else(|| path_of(uri_text).and_then(|path| std::fs::read_to_string(path).ok()))
            .unwrap_or_default()
    }

    /// The artifact at a position.
    fn target_at(&self, params: &Json) -> Option<Target> {
        let TextPosition {
            uri: uri_text,
            line,
            character,
        } = Self::position(params);
        let text = self.text_of(&uri_text);
        let word = word_at(&text, line, character);
        self.backend.at(&path_of(&uri_text)?, line + 1, &word)
    }

    /// A request's answer.
    pub fn answer(&mut self, request: &Request) -> Response {
        let params = &request.params;
        let result = match request.method.as_str() {
            "textDocument/definition" => match self.target_at(params) {
                Some(target) => json!(location(&target.path, target.line)),
                None => Json::Null,
            },
            "textDocument/references" => match self.target_at(params) {
                Some(target) => {
                    let mut found: Vec<lsp::Location> = self
                        .backend
                        .uses(&target.id)
                        .iter()
                        .map(|(path, line)| location(path, *line))
                        .collect();
                    if params["context"]["includeDeclaration"].as_bool().unwrap_or(false) {
                        found.insert(0, location(&target.path, target.line));
                    }
                    json!(found)
                }
                None => json!([]),
            },
            "textDocument/prepareRename" | "textDocument/rename" => match self.rename(request) {
                Ok(result) => result,
                Err(message) => {
                    return Response::new_err(request.id.clone(), lsp_server::ErrorCode::RequestFailed as i32, message)
                }
            },
            "textDocument/codeAction" => self.code_actions(params),
            "textDocument/hover" => self.hover(params),
            "textDocument/completion" => self.completion(params),
            "textDocument/documentSymbol" => self.symbols(params),
            "workspace/symbol" => {
                let query = params["query"].as_str().unwrap_or_default();
                let found: Vec<Json> = self
                    .backend
                    .find(query, &[], 100)
                    .iter()
                    .map(|target| {
                        json!({ "name": target.name, "kind": symbol_kind(&target.kind), "containerName": target.kind,
                            "location": location(&target.path, target.line) })
                    })
                    .collect();
                json!(found)
            }
            "shutdown" => Json::Null,
            _ => {
                return Response::new_err(
                    request.id.clone(),
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("moqui lsp does not answer {}", request.method),
                )
            }
        };
        Response::new_ok(request.id.clone(), result)
    }

    /// `prepareRename` (the name's range and text) or `rename` (a `WorkspaceEdit` across the files), or why not.
    fn rename(&self, request: &Request) -> Result<Json, String> {
        let params = &request.params;
        let TextPosition {
            uri: uri_text,
            line,
            character,
        } = Self::position(params);
        let path = path_of(&uri_text).ok_or_else(|| format!("{uri_text} is no file"))?;
        let text = self.text_of(&uri_text);
        let (target, span) = self.backend.rename_target(&path, &text, line, character)?;
        if request.method == "textDocument/prepareRename" {
            let range = lsp::Range::new(
                lsp::Position::new(span.line, span.start),
                lsp::Position::new(span.line, span.end),
            );
            return Ok(json!({ "range": range, "placeholder": target.name }));
        }
        let new_name = params["newName"].as_str().unwrap_or_default();
        let documents: HashMap<PathBuf, &String> = self
            .documents
            .iter()
            .filter_map(|(uri_text, text)| Some((path_of(uri_text)?, text)))
            .collect();
        let text_of = |path: &Path| {
            documents
                .get(path)
                .map(|text| (*text).clone())
                .or_else(|| std::fs::read_to_string(path).ok())
        };
        let open: Vec<PathBuf> = documents.keys().cloned().collect();
        let edits = self.backend.rename(&target, new_name, &text_of, &open)?;
        Ok(json!({ "changes": workspace_changes(&edits) }))
    }

    /// The quick fixes of the diagnostics the client sends: for an unknown entity, service, field or in-parameter, the
    /// nearest known names, each an edit of the name's value; and for an in-place edit of an upstream component,
    /// "Extend in my-site" where the range is on what it added or an `upstream.edit` diagnostic is sent.
    fn code_actions(&self, params: &Json) -> Json {
        let uri_text = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let text = self.text_of(uri_text);
        let mut actions = Vec::new();
        actions.extend(self.extend_action(uri_text, &text, params));
        for diagnostic in params["context"]["diagnostics"].as_array().into_iter().flatten() {
            let rule = diagnostic["code"].as_str().unwrap_or_default();
            let Some(unknown) = edit::unknown_name(rule, diagnostic["message"].as_str().unwrap_or_default()) else {
                continue;
            };
            let line = u32::try_from(diagnostic["range"]["start"]["line"].as_u64().unwrap_or(0)).unwrap_or(0);
            let character = u32::try_from(diagnostic["range"]["start"]["character"].as_u64().unwrap_or(0)).unwrap_or(0);
            for (at, fix) in self.backend.nearest(&unknown).into_iter().enumerate() {
                let Some(found) = edit::value_edit(&text, line, character, unknown.name(), &fix) else {
                    continue;
                };
                actions.push(json!({
                    "title": format!("Change to {fix}"),
                    "kind": lsp::CodeActionKind::QUICKFIX,
                    "diagnostics": [diagnostic],
                    "isPreferred": at == 0,
                    "edit": { "changes": { uri_text: [text_edit(&found)] } },
                }));
            }
        }
        json!(actions)
    }

    /// "Extend in my-site": a `WorkspaceEdit` taking the upstream file back to its committed text and writing what it
    /// added as the site component's extension (a new file made where the site has none), when the edit is mechanical
    /// and the request's range is on an added element or carries an `upstream.edit` diagnostic. Offered only to a
    /// client that applies `documentChanges` with a `create` (its `initialize` capabilities).
    fn extend_action(&self, uri_text: &str, text: &str, params: &Json) -> Option<Json> {
        if !self.edits_create_files {
            return None;
        }
        let path = path_of(uri_text)?;
        let documents: HashMap<PathBuf, &String> = self
            .documents
            .iter()
            .filter_map(|(uri_text, text)| Some((path_of(uri_text)?, text)))
            .collect();
        let text_of = |path: &Path| {
            documents
                .get(path)
                .map(|text| (*text).clone())
                .or_else(|| std::fs::read_to_string(path).ok())
        };
        let extension = self.backend.extension(&path, text, &text_of)?;
        let line_of = |key: &str| {
            params["range"][key]["line"]
                .as_u64()
                .and_then(|line| u32::try_from(line).ok())
        };
        // the added elements' lines are 1-based, the range's 0-based
        let on_added = match (line_of("start"), line_of("end")) {
            (Some(start), Some(end)) => extension
                .added
                .iter()
                .any(|added| start < added.end_line && end + 1 >= added.line),
            _ => false,
        };
        let diagnostics: Vec<&Json> = params["context"]["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|diagnostic| diagnostic["code"].as_str() == Some(extend::UPSTREAM_RULE))
            .collect();
        if !on_added && diagnostics.is_empty() {
            return None;
        }
        // an open document's edit names the version it was made on, so the client refuses it on a later text
        let version = |uri: &str| self.versions.get(uri).map_or(Json::Null, |version| json!(version));
        let mut changes = vec![
            json!({ "textDocument": { "uri": uri_text, "version": version(uri_text) },
            "edits": [span_edit(text, &extension.reverted)] }),
        ];
        for site_edit in &extension.site_edits {
            let uri = uri_of(&site_edit.path);
            let before = match &site_edit.before {
                Some(before) => before.as_str(),
                None => {
                    // a file made since is not written over: the client fails the edit instead
                    changes.push(json!({ "kind": "create", "uri": uri,
                        "options": { "overwrite": false, "ignoreIfExists": false } }));
                    ""
                }
            };
            let version = if site_edit.before.is_some() {
                version(&uri)
            } else {
                Json::Null
            };
            changes.push(json!({ "textDocument": { "uri": uri, "version": version },
                "edits": [span_edit(before, &site_edit.after)] }));
        }
        let what: Vec<&str> = extension.added.iter().map(|added| added.what.as_str()).collect();
        Some(json!({
            "title": format!("Extend in {}: {}", extension.site, what.join(", ")),
            "kind": lsp::CodeActionKind::QUICKFIX,
            "diagnostics": diagnostics,
            "isPreferred": true,
            "edit": { "documentChanges": changes },
        }))
    }

    fn hover(&self, params: &Json) -> Json {
        let TextPosition {
            uri: uri_text,
            line,
            character,
        } = Self::position(params);
        let text = self.text_of(&uri_text);
        let mut parts = Vec::new();
        if let Some(path) = path_of(&uri_text) {
            let word = word_at(&text, line, character);
            if let Some(target) = self.backend.at(&path, line + 1, &word) {
                parts.push(format!(
                    "**{} {}**  \n{}:{}{}",
                    target.kind,
                    target.name,
                    target.path.display(),
                    target.line,
                    if target.summary.is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}", target.summary)
                    }
                ));
            }
        }
        if let (Some(root), Some(TagContext { element, place })) = (
            schema::root_of(&text),
            tag_context(&text, line, character.saturating_add(1)),
        ) {
            if let Some(spec) = self
                .backend
                .digest(root)
                .and_then(|digest| digest.element(&element).cloned())
            {
                let attribute = match &place {
                    TagPlace::Value(name) => spec.attribute(name),
                    TagPlace::Name | TagPlace::Attributes => None,
                };
                let doc = match attribute {
                    Some(attribute) => format!("`{}` of `{}`: {}", attribute.name, spec.name, attribute.doc),
                    None => format!("`<{}>`: {}", spec.name, spec.doc),
                };
                if !doc.trim_end().ends_with(':') {
                    parts.push(doc);
                }
            }
        }
        if parts.is_empty() {
            return Json::Null;
        }
        json!({ "contents": { "kind": "markdown", "value": parts.join("\n\n---\n\n") } })
    }

    fn completion(&self, params: &Json) -> Json {
        let TextPosition {
            uri: uri_text,
            line,
            character,
        } = Self::position(params);
        let text = self.text_of(&uri_text);
        let Some(root) = schema::root_of(&text).or_else(|| {
            path_of(&uri_text)
                .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
                .map(|_| "screen")
        }) else {
            return json!([]);
        };
        let digest = self.backend.digest(root);
        let item = |label: &str, kind: lsp::CompletionItemKind, detail: &str| json!({ "label": label, "kind": kind, "detail": detail });
        let Some(TagContext { element, place }) = tag_context(&text, line, character) else {
            return json!([]);
        };
        let items: Vec<Json> = match place {
            // a value: the enumeration the XSD gives, or names from the index
            TagPlace::Value(attribute) => {
                let word = word_at(&text, line, character);
                let values = digest
                    .as_ref()
                    .and_then(|digest| digest.element(&element).cloned())
                    .and_then(|spec| spec.attribute(&attribute).cloned())
                    .map(|spec| spec.values)
                    .unwrap_or_default();
                if !values.is_empty() {
                    values
                        .iter()
                        .map(|value| item(value, lsp::CompletionItemKind::ENUM_MEMBER, ""))
                        .collect()
                } else if let Some(kinds) = names_kind(&element, &attribute) {
                    let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_string()).collect();
                    self.backend
                        .find(&word, &kinds, 50)
                        .iter()
                        .map(|target| item(&target.name, lsp::CompletionItemKind::REFERENCE, &target.kind))
                        .collect()
                } else {
                    Vec::new()
                }
            }
            // a tag's name: every element of the file's kind
            TagPlace::Name => digest
                .map(|digest| {
                    digest
                        .elements
                        .iter()
                        .map(|spec| item(&spec.name, lsp::CompletionItemKind::CLASS, &first_line(&spec.doc)))
                        .collect()
                })
                .unwrap_or_default(),
            // an attribute's name: the element's attributes
            TagPlace::Attributes => digest
                .and_then(|digest| digest.element(&element).cloned())
                .map(|spec| {
                    spec.attributes
                        .iter()
                        .map(|attribute| {
                            item(
                                &attribute.name,
                                lsp::CompletionItemKind::PROPERTY,
                                &first_line(&attribute.doc),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        json!(items)
    }

    fn symbols(&self, params: &Json) -> Json {
        let uri_text = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let text = self.text_of(uri_text);
        let Ok(outline) = moqui_lib::xml_edit::outline(&text) else {
            return json!([]);
        };
        fn symbol(node: &moqui_lib::xml_edit::OutlineNode, depth: usize) -> Json {
            let named = ["name", "entity-name", "verb", "id"].iter().find_map(|key| {
                node.attrs
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.clone())
            });
            let noun = node
                .attrs
                .iter()
                .find(|(name, _)| name == "noun")
                .map(|(_, value)| value.clone());
            let label = match (named, noun) {
                (Some(verb), Some(noun)) => format!("{} {verb}#{noun}", node.name),
                (Some(name), None) => format!("{} {name}", node.name),
                _ => node.name.clone(),
            };
            let line = u32::try_from(node.line.saturating_sub(1)).unwrap_or(0);
            let column = u32::try_from(node.column.saturating_sub(1)).unwrap_or(0);
            let at = lsp::Range::new(
                lsp::Position::new(line, column),
                lsp::Position::new(line, column.saturating_add(1)),
            );
            let children: Vec<Json> = if depth < 8 {
                node.children.iter().map(|child| symbol(child, depth + 1)).collect()
            } else {
                Vec::new()
            };
            json!({ "name": label, "kind": lsp::SymbolKind::OBJECT, "range": at, "selectionRange": at, "children": children })
        }
        json!([symbol(&outline, 0)])
    }
}

/// An edit as the protocol's `TextEdit`.
fn text_edit(edit: &edit::Edit) -> Json {
    let range = lsp::Range::new(
        lsp::Position::new(edit.line, edit.start),
        lsp::Position::new(edit.line, edit.end),
    );
    json!({ "range": range, "newText": edit.text })
}

/// The protocol's position of the byte offset `at` of `text`: its 0-based line and UTF-16 character.
fn position_of(text: &str, at: usize) -> lsp::Position {
    let before = &text[..at];
    let line_start = before.rfind('\n').map_or(0, |found| found + 1);
    lsp::Position::new(
        u32::try_from(before.matches('\n').count()).unwrap_or(u32::MAX),
        u32::try_from(before[line_start..].encode_utf16().count()).unwrap_or(u32::MAX),
    )
}

/// The one `TextEdit` making `text` into `new`: the span between their common start and end.
fn span_edit(text: &str, new: &str) -> Json {
    let (span, replacement) = extend::changed_span(text, new);
    let range = lsp::Range::new(position_of(text, span.start), position_of(text, span.end));
    json!({ "range": range, "newText": replacement })
}

/// A `WorkspaceEdit`'s `changes`: each file's URI with its edits.
fn workspace_changes(edits: &std::collections::BTreeMap<PathBuf, Vec<edit::Edit>>) -> serde_json::Map<String, Json> {
    edits
        .iter()
        .map(|(path, list)| (uri_of(path), Json::Array(list.iter().map(text_edit).collect())))
        .collect()
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").chars().take(120).collect()
}

fn symbol_kind(kind: &str) -> lsp::SymbolKind {
    match kind {
        "entity" | "view-entity" => lsp::SymbolKind::STRUCT,
        "field" => lsp::SymbolKind::FIELD,
        "service" => lsp::SymbolKind::FUNCTION,
        "screen" => lsp::SymbolKind::MODULE,
        "transition" => lsp::SymbolKind::METHOD,
        _ => lsp::SymbolKind::OBJECT,
    }
}

/// Serve `connection` until the client says exit.
pub(crate) fn serve(connection: &Connection, server: &mut Server) -> Result<(), Error> {
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection
                    .handle_shutdown(&request)
                    .map_err(|err| Error::new(format!("lsp: {err}")))?
                {
                    return Ok(());
                }
                let response = server.answer(&request);
                connection
                    .sender
                    .send(Message::Response(response))
                    .map_err(|err| Error::new(format!("lsp: {err}")))?;
            }
            Message::Notification(note) => {
                if note.method == "exit" {
                    return Ok(());
                }
                for out in server.notify(&note) {
                    connection
                        .sender
                        .send(Message::Notification(out))
                        .map_err(|err| Error::new(format!("lsp: {err}")))?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// `moqui lsp [--server URL] [--project DIR | --java DIR]`: the server over stdio.
pub(crate) fn dispatch_lsp(args: &[String]) -> Result<(), Error> {
    let mut server_url = None;
    let mut project = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--server" => server_url = Some(rest.next().ok_or_else(|| crate::usage::command_usage("lsp"))?.clone()),
            other if let Some(url) = other.strip_prefix("--server=") => server_url = Some(url.to_string()),
            other if let Some(found) = crate::devtools_cmd::ProjectArg::parse(other, &mut rest) => {
                project = Some(found.ok_or_else(|| crate::usage::command_usage("lsp"))?);
            }
            "--stdio" => {}
            _ => return Err(crate::usage::command_usage("lsp")),
        }
    }
    // stdout is the protocol's: the log goes to stderr only
    crate::logging::install_with_console(Some(tracing::Level::WARN));
    let backend = match server_url {
        Some(url) => Backend::Remote(crate::devtools_cmd::remote(&url, project.as_ref())?),
        None => {
            let factory = crate::devtools_cmd::model_only(project.as_ref())?;
            let index = Box::new(MemoryIndex::build(&factory));
            Backend::Local { factory, index }
        }
    };
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(Server::capabilities()).unwrap_or(Json::Null);
    let (id, client) = connection
        .initialize_start()
        .map_err(|err| Error::new(format!("lsp initialize: {err}")))?;
    connection
        .initialize_finish(
            id,
            json!({ "capabilities": capabilities, "serverInfo": { "name": "moqui", "version": env!("CARGO_PKG_VERSION") } }),
        )
        .map_err(|err| Error::new(format!("lsp initialize: {err}")))?;
    let mut server = Server::new(backend);
    server.set_client(&client);
    serve(&connection, &mut server)?;
    drop(connection);
    io_threads.join().map_err(|err| Error::new(format!("lsp: {err}")))?;
    Ok(())
}

#[cfg(test)]
#[path = "lsp_tests.rs"]
mod tests;
