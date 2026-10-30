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

//! `source` (plans/design-dev-aide.md 13.2, READ): a file's lines (a range, or the element of an artifact the index
//! names), its outline (the elements to a depth, with their lines and naming attributes), and its git status, with
//! whether it is the site's or upstream. Files are confined to the Workspace's roots as the editor's are.

use moqui_devkit::index;
use moqui_lib::xml_edit::{Doc, Kind};
use moqui_llm::run::context::TurnContext;
use moqui_llm::tools::format::ToolError;
use moqui_llm::tools::schema::{object, Property};
use moqui_llm::tools::{caps, AgentTool, Classification, ToolOutcome, ToolSpec};
use serde_json::{json, Value as Json};

use super::{arg, context, file};
use crate::roots::RootKind;
use crate::tier::Tier;

/// Lines one answer gives at most.
const MAX_LINES: usize = 400;

pub struct SourceTool(ToolSpec);

impl Default for SourceTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "source",
            "Read a file of a Moqui component: lines (from, to), or the whole element of an artifact (artifact: an id \
             from the artifact tool), with line numbers; outline=true lists its elements with their lines instead. Says \
             whether the file is the site's or upstream, and its git status.",
            object(&[
                Property::optional("file", "string", "component://<component>/<path>, or root:path"),
                Property::optional("artifact", "string", "An artifact id: its element's lines"),
                Property::optional("from", "integer", "First line, 1-based"),
                Property::optional("to", "integer", "Last line"),
                Property::optional("outline", "boolean", "The elements and their lines, not the text"),
            ]),
            Classification::Read,
        )
        .needs(&[caps::DEV]);
        spec.mcp = true;
        Self(spec)
    }
}

impl AgentTool for SourceTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, _ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        match run(&args) {
            Ok(answer) => super::json(&answer),
            Err(error) => error.outcome(),
        }
    }
}

fn run(args: &Json) -> Result<Json, ToolError> {
    let context = context(Tier::View)?;
    // an artifact names its file and its first line
    let (named, artifact_line) = match arg(args, "artifact") {
        Some(id) => {
            let artifact = index::named(&context.factory, id)
                .map_err(|err| ToolError::new("index", err.to_string()))?
                .ok_or_else(|| ToolError::new("not_found", format!("No artifact is named {id}.")))?;
            (artifact.path.clone(), Some(artifact.line))
        }
        None => (arg(args, "file").unwrap_or("").to_string(), None),
    };
    let named = file(&context, &named)?;
    let text = std::fs::read_to_string(&named.path)
        .map_err(|err| ToolError::new("not_read", format!("{}: {err}", named.shown)))?;
    let site = named.root.is_site_path(&named.relative);
    let status = git_status(&context, named.root, &named.relative);
    let mut out = json!({
        "file": named.shown, "side": if site { "site" } else { "upstream" },
        "lines": text.lines().count(), "status": status,
    });
    if args.get("outline").and_then(Json::as_bool).unwrap_or(false) {
        out["outline"] = json!(outline(&text, 4));
        return Ok(out);
    }
    let line_count = text.lines().count().max(1);
    let (from, to) = match artifact_line {
        Some(line) => element_lines(&text, line as usize).unwrap_or((line as usize, line as usize)),
        None => {
            let from = args
                .get("from")
                .and_then(Json::as_u64)
                .map_or(1, |line| line as usize)
                .max(1);
            let to = args
                .get("to")
                .and_then(Json::as_u64)
                .map_or(line_count, |line| line as usize)
                .min(line_count);
            (from, to.max(from))
        }
    };
    let to = to.min(from + MAX_LINES - 1);
    let numbered: Vec<String> = text
        .lines()
        .enumerate()
        .skip(from - 1)
        .take(to + 1 - from)
        .map(|(index, line)| format!("{:>5}  {line}", index + 1))
        .collect();
    out["from"] = json!(from);
    out["to"] = json!(to);
    out["text"] = json!(numbered.join("\n"));
    if to < line_count && artifact_line.is_none() && args.get("to").is_none() {
        out["more"] = json!(format!(
            "lines {} to {line_count} not shown: ask with from and to",
            to + 1
        ));
    }
    Ok(out)
}

/// The git status letter of a file, or "" when unchanged or not in a repository.
fn git_status(context: &crate::workspace::Context, root: &crate::roots::Root, relative: &str) -> String {
    if root.kind != RootKind::Git {
        return "not versioned".into();
    }
    context
        .git()
        .read(&root.path, &["status", "--porcelain=v2", "-z", "--", relative])
        .map(|out| {
            crate::git::parse_status(&out)
                .entries
                .first()
                .map(|entry| entry.letter().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// The 1-based lines of the element that starts on `line`.
fn element_lines(text: &str, line: usize) -> Option<(usize, usize)> {
    let doc = Doc::parse(text).ok()?;
    let line_of = |offset: usize| text[..offset].matches('\n').count() + 1;
    doc.nodes
        .iter()
        .filter(|node| node.kind == Kind::Element)
        .find(|node| line_of(node.span.start) == line)
        .map(|node| (line, line_of(node.span.end.saturating_sub(1))))
}

/// The elements of an XML text to `depth`, each with its line and its naming attributes.
pub(crate) fn outline(text: &str, depth: usize) -> Vec<String> {
    let Ok(doc) = Doc::parse(text) else {
        return vec!["(the file does not parse as XML)".into()];
    };
    let line_of = |offset: usize| text[..offset].matches('\n').count() + 1;
    let mut lines = Vec::new();
    let mut stack: Vec<(usize, usize)> = doc.root().map(|root| vec![(root, 0)]).unwrap_or_default();
    while let Some((index, level)) = stack.pop() {
        let mut label = format!(
            "{:>5}  {}<{}",
            line_of(doc.nodes[index].span.start),
            "  ".repeat(level),
            doc.name(index)
        );
        for key in [
            "name",
            "entity-name",
            "package",
            "verb",
            "noun",
            "id",
            "service",
            "entity",
            "location",
            "field-name",
            "related",
        ] {
            if let Some(value) = doc.attr(index, key) {
                label.push_str(&format!(" {key}=\"{value}\""));
            }
        }
        label.push('>');
        lines.push(label);
        if level + 1 < depth {
            let children: Vec<usize> = doc.elements(index).collect();
            for child in children.into_iter().rev() {
                stack.push((child, level + 1));
            }
        }
        if lines.len() >= MAX_LINES {
            lines.push("…".into());
            break;
        }
    }
    lines
}

moqui_llm::submit_tool!(SourceTool);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_lines_and_outline() {
        let text = "<entities>\n    <entity entity-name=\"A\" package=\"p\">\n        <field name=\"x\"/>\n    </entity>\n</entities>\n";
        assert_eq!(element_lines(text, 2), Some((2, 4)));
        let shown = outline(text, 3);
        assert_eq!(shown[1], "    2    <entity entity-name=\"A\" package=\"p\">");
        assert_eq!(shown[2], "    3      <field name=\"x\">");
    }
}
