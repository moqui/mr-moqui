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
//! `moqui_core::devtools::check`, on each open file's text as it changes), go to definition and references (the artifact
//! index, read into memory on a model-only boot and kept current with the open buffers), hover (an artifact's summary,
//! or an element's or attribute's documentation from the XSD digest), completion (the elements and attributes the
//! digest gives, and entity and service names from the index in their attributes), and document and workspace symbols.
//! With `--server URL` the checks and the index are a running dev instance's, over its MCP tools `check` and
//! `artifact`, with the login key `moqui tui --server` keeps (or `MOQUI_LOGIN_KEY`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types as lsp;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::devtools::index::{self, Direction, MemoryIndex};
use moqui_core::devtools::{check, schema, Problem, Severity};
use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use crate::devtools_remote::{Remote, Target};

/// What answers the server: this checkout's model and index, or a running instance's.
pub enum Backend {
    Local {
        factory: Arc<ExecutionContextFactory>,
        index: MemoryIndex,
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
                check::check_text(factory, path, text)
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
                artifact.map(|artifact| target_of(&artifact))
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
            Self::Local { index, .. } => index.find(text, kinds, limit).iter().map(target_of).collect(),
            Self::Remote(remote) => remote.find(text, kinds, limit),
        }
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

fn target_of(artifact: &index::ArtifactRow) -> Target {
    Target {
        id: artifact.id.clone(),
        kind: artifact.kind.clone(),
        name: artifact.name.clone(),
        path: PathBuf::from(&artifact.path),
        line: artifact.line,
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
    // file:///C:/x on Windows
    let path = if path.len() > 2 && path.as_bytes()[2] == b':' {
        path[1..].to_string()
    } else {
        path
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
    let Some(line) = text.lines().nth(line as usize) else {
        return String::new();
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
    chars[start..end].iter().collect()
}

/// Where the cursor is in a tag: the element's name, and the attribute whose value it is in, if any.
pub(crate) fn tag_context(text: &str, line: u32, character: u32) -> Option<(String, Option<String>, bool)> {
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
    let name: String = tag.chars().take_while(|ch| !ch.is_whitespace()).collect();
    if name.starts_with('/') || name.starts_with('!') || name.starts_with('?') {
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
        return Some((name, Some(attribute), true));
    }
    let naming = !tag.contains(char::is_whitespace);
    Some((name, None, naming))
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
}

impl Server {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            documents: HashMap::new(),
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

    fn position(params: &Json) -> (String, u32, u32) {
        (
            params["textDocument"]["uri"].as_str().unwrap_or_default().to_string(),
            u32::try_from(params["position"]["line"].as_u64().unwrap_or(0)).unwrap_or(0),
            u32::try_from(params["position"]["character"].as_u64().unwrap_or(0)).unwrap_or(0),
        )
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
        let (uri_text, line, character) = Self::position(params);
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

    fn hover(&self, params: &Json) -> Json {
        let (uri_text, line, character) = Self::position(params);
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
        if let (Some(root), Some((element, attribute, _))) = (
            schema::root_of(&text),
            tag_context(&text, line, character.saturating_add(1)),
        ) {
            if let Some(spec) = self
                .backend
                .digest(root)
                .and_then(|digest| digest.element(&element).cloned())
            {
                let doc = match attribute.as_deref().and_then(|name| spec.attribute(name)) {
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
        let (uri_text, line, character) = Self::position(params);
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
        let Some((element, attribute, naming)) = tag_context(&text, line, character) else {
            return json!([]);
        };
        let items: Vec<Json> = match (attribute, naming) {
            // a value: the enumeration the XSD gives, or names from the index
            (Some(attribute), _) => {
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
            (None, true) => digest
                .map(|digest| {
                    digest
                        .elements
                        .iter()
                        .map(|spec| item(&spec.name, lsp::CompletionItemKind::CLASS, &first_line(&spec.doc)))
                        .collect()
                })
                .unwrap_or_default(),
            // an attribute's name: the element's attributes
            (None, false) => digest
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

/// `moqui lsp [--server URL]`: the server over stdio.
pub(crate) fn dispatch_lsp(args: &[String]) -> Result<(), Error> {
    let mut server_url = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--server" => server_url = rest.next().cloned(),
            other if other.starts_with("--server=") => server_url = Some(other["--server=".len()..].to_string()),
            "--stdio" => {}
            _ => return Err(crate::usage::command_usage("lsp")),
        }
    }
    // stdout is the protocol's: the log goes to stderr only
    crate::logging::install_with_console(Some(tracing::Level::WARN));
    let backend = match server_url {
        Some(url) => Backend::Remote(Remote::connect(&url)?),
        None => {
            let factory = crate::devtools_cmd::model_only()?;
            let index = MemoryIndex::build(&factory);
            Backend::Local { factory, index }
        }
    };
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(Server::capabilities()).unwrap_or(Json::Null);
    let (id, _client) = connection
        .initialize_start()
        .map_err(|err| Error::new(format!("lsp initialize: {err}")))?;
    connection
        .initialize_finish(
            id,
            json!({ "capabilities": capabilities, "serverInfo": { "name": "moqui", "version": env!("CARGO_PKG_VERSION") } }),
        )
        .map_err(|err| Error::new(format!("lsp initialize: {err}")))?;
    let mut server = Server::new(backend);
    serve(&connection, &mut server)?;
    drop(connection);
    io_threads.join().map_err(|err| Error::new(format!("lsp: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_and_paths_both_ways() {
        let path = Path::new("/w/my site/screen/A.xml");
        assert_eq!(uri_of(path), "file:///w/my%20site/screen/A.xml");
        assert_eq!(path_of(&uri_of(path)).unwrap(), path);
        assert_eq!(path_of("file:///C:/w/a.xml").unwrap(), PathBuf::from("C:/w/a.xml"));
        assert!(path_of("untitled:1").is_none());
        // a % before a character of several bytes is taken as it is, not sliced
        assert_eq!(
            path_of("file:///w/%a\u{e9}.xml").unwrap(),
            PathBuf::from("/w/%a\u{e9}.xml")
        );
    }

    #[test]
    fn the_word_and_the_tag_under_the_cursor() {
        let text = "<screen>\n    <service-call name=\"mantle.order.OrderServices.place#Order\" in-map=\"\"/>\n    <entity-find ";
        assert_eq!(word_at(text, 1, 30), "mantle.order.OrderServices.place#Order");
        assert_eq!(
            tag_context(text, 1, 30),
            Some(("service-call".into(), Some("name".into()), true))
        );
        assert_eq!(tag_context(text, 2, 17), Some(("entity-find".into(), None, false)));
        assert_eq!(
            tag_context("<screen>\n    <ent", 1, 8),
            Some(("ent".into(), None, true))
        );
        assert_eq!(tag_context("<screen>text", 0, 10), None);
    }
}
