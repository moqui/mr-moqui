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

//! A doc's text from its template (plans/archive/Autodoc.md, "Templates"): the template of its kind, from the first template
//! directory that has one, else the framework's; run as a trusted fancy template (`.fancy`, not `.md.fancy`, which makes
//! HTML) with the doc's data as its names.

use moqui_core::context::ExecutionContextFactory;
use moqui_core::script::template::TemplateRegistry;
use moqui_lib::error::Error;
use serde_json::Value as Json;

use super::plan::{DocKind, Page};
use super::Options;

/// Where the framework's templates are.
pub const DEFAULT_TEMPLATES: &str = "component://framework/template/autodoc";

/// The template a doc is made with.
pub(super) fn template_of(page: &Page) -> &'static str {
    match page {
        Page::Root => "Index.fancy",
        Page::TypeIndex(_) => "TypeIndex.fancy",
        Page::Package(DocKind::Entity, _) => "EntityPackage.fancy",
        Page::Package(DocKind::Service, _) => "ServiceDirectory.fancy",
        Page::Package(_, _) => "ScreenDirectory.fancy",
        Page::Artifact(DocKind::Entity, _) => "Entity.fancy",
        Page::Artifact(DocKind::Service, _) => "Service.fancy",
        Page::Artifact(_, _) => "Screen.fancy",
        Page::File(DocKind::Service, _) => "ServiceFile.fancy",
        Page::File(DocKind::Eca, _) => "EcaFile.fancy",
        Page::File(_, _) => "RestFile.fancy",
    }
}

/// The location of the template `name`: the first of the options' directories that has it, else the framework's.
fn locate(factory: &ExecutionContextFactory, options: &Options, name: &str) -> String {
    for dir in &options.template_dirs {
        let location = format!("{}/{name}", dir.trim_end_matches('/'));
        if factory
            .resource()
            .get(&location)
            .ok()
            .is_some_and(|resource| resource.exists())
        {
            return location;
        }
    }
    format!("{DEFAULT_TEMPLATES}/{name}")
}

/// The template `name` run over `data`, its blank lines kept to one between blocks and one newline at its end.
pub(super) fn render(
    factory: &ExecutionContextFactory,
    options: &Options,
    name: &str,
    data: &Json,
) -> Result<String, Error> {
    let location = locate(factory, options, name);
    let value = moqui_lib::json::from_json(data);
    let text = TemplateRegistry::of(factory)
        .render_location(factory, &location, Some(&value))
        .map_err(|err| Error::new(format!("{location}: {err}")))?;
    Ok(tidy(&text))
}

/// Trailing spaces off, runs of blank lines as one and a blank line before a heading, a table or a fence that follows
/// text (outside fenced code), one newline at the end: the templates need not count their blank lines.
pub(super) fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    let mut fence: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim_end();
        let marker = trimmed.trim_start();
        if let Some(open) = &fence {
            if marker.starts_with(open.as_str())
                && marker.trim_start_matches(open.chars().next().unwrap_or('`')).is_empty()
            {
                fence = None;
            }
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        let opens_fence = marker.starts_with("```") || marker.starts_with("~~~");
        if opens_fence {
            let ch = marker.chars().next().unwrap_or('`');
            fence = Some(marker.chars().take_while(|c| *c == ch).collect());
        }
        let block = opens_fence || is_heading(marker) || (marker.starts_with('|') && !last_line(&out).starts_with('|'));
        if block && !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        if trimmed.is_empty() {
            blank += 1;
            if blank > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A Markdown heading: one to six `#` and a space.
fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
}

/// The last line written.
fn last_line(out: &str) -> &str {
    out.trim_end_matches('\n').rsplit('\n').next().unwrap_or_default()
}
