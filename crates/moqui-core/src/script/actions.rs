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

//! XML actions. One cached script per action XML text. Names in that script are bare.

use std::collections::HashSet;
use std::sync::{mpsc, Mutex, OnceLock, PoisonError};

use moqui_lib::node::MNode;
use rquickjs::{Ctx, Object};

use super::runtime::ScriptText;
use super::walker::{self, Walk};
use crate::context::Error;

const TEMPLATE_SUPPORT: &str = include_str!("../../../../runtime/base-component/framework/template/TemplateSupport.js");
const XML_ACTIONS: &str = include_str!("../../../../runtime/base-component/framework/template/XmlActions.js");

/// Set this to add a comment naming the XML element before each generated statement. The value `dump` also prints
/// each generated script to stderr, with line numbers as an error's `eval_script:LINE` has them.
const DEBUG_ENV: &str = "MOQUI_ACTIONS_DEBUG";

/// The generated scripts kept, and the XML and script text they may add up to.
const MAX_CACHED_SCRIPTS: usize = 4096;
const MAX_CACHED_SCRIPT_BYTES: usize = 32 * 1024 * 1024;

type Generated = Mutex<super::bounded::BoundedMap<String, ScriptText>>;

/// The script generated for each action XML text, for every thread: generation is a function of the text alone.
fn generated_scripts() -> &'static Generated {
    static SCRIPTS: OnceLock<Generated> = OnceLock::new();
    SCRIPTS.get_or_init(|| {
        Mutex::new(super::bounded::BoundedMap::with_bytes(
            MAX_CACHED_SCRIPTS,
            MAX_CACHED_SCRIPT_BYTES,
        ))
    })
}

pub fn run_actions(actions: &str) -> Result<(), Error> {
    run_actions_at(actions, None)
}

/// Name generated JavaScript after its owning actions, without pretending generated lines are XML lines.
pub(crate) fn run_actions_at(actions: &str, source: Option<&moqui_lib::error::SourceLocation>) -> Result<(), Error> {
    if actions.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let script = generated(actions)?;
    let file_name = source.and_then(|source| {
        source
            .file
            .as_ref()
            .map(|file| format!("{file}#actions-{}-{}.generated.js", source.line, source.column))
    });
    let script = match &file_name {
        Some(file_name) => script.with_file_name(file_name),
        None => script,
    };
    let file_name = file_name.as_deref().unwrap_or("eval_script");
    // Data preparation may do what a browser cannot, so a widget check in progress stands down while the actions run.
    let _unchecked = super::runtime::WidgetWrap::set(false);
    super::runtime::eval_script_text(&script).map_err(|err| with_source_line(err, script.text(), file_name))
}

/// Evaluate a `<condition>` element as `<if>` does. Its children are xml-actions conditions
/// (`expression`, `compare`, `and`, `or`, `not`), and all of them must hold. An element with no
/// condition macro is an error, never a pass.
pub fn eval_condition_element(xml: &str) -> Result<bool, Error> {
    if xml.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let root = MNode::parse_text("condition", xml)?;
    if !root.has_name("condition") {
        return Err(Error::new(format!(
            "expected a condition element, found {}",
            root.name()
        )));
    }
    // The children of an `and` render joined by `&&`, or `true` when there are none.
    let mut combined = String::from("<and>");
    for child in root.child_iter() {
        combined.push_str(&child.write_string());
    }
    combined.push_str("</and>");
    let script = generated(&combined)?;
    // Only the root `actions` macro strips the raw-region marks of multi-line user code.
    super::runtime::eval_condition(&script.text().replace(['\u{1}', '\u{2}'], ""))
}

/// [`eval_condition_element`] for a condition group made once: `<and>` holding the condition macros.
pub fn eval_condition_group(and_xml: &str) -> Result<bool, Error> {
    if and_xml.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let script = generated(and_xml)?;
    super::runtime::eval_condition(&script.text().replace(['\u{1}', '\u{2}'], ""))
}

/// An error at `eval_script:LINE:COL` with that line of the generated script after it, so a failure in actions points at
/// the statement. With `MOQUI_ACTIONS_DEBUG` set, the element comment above it is shown too.
fn with_source_line(err: Error, source: &str, file_name: &str) -> Error {
    let message = err.message();
    let prefix = format!("{file_name}:");
    let line = message
        .find(&prefix)
        .and_then(|at| message[at + prefix.len()..].split(':').next()?.parse::<usize>().ok());
    let Some(line) = line.filter(|line| *line > 0) else {
        return err;
    };
    let lines: Vec<&str> = source.lines().collect();
    let Some(text) = lines.get(line - 1) else {
        return err;
    };
    let mut detail = format!("\n    at generated line {line}: {}", text.trim());
    if let Some(comment) = lines[..line - 1]
        .iter()
        .rev()
        .map(|text| text.trim())
        .find(|text| text.starts_with("//"))
    {
        detail.push_str(&format!("\n    after {comment}"));
    }
    let message = format!("{}{detail}", message.trim_end());
    err.with_message(message)
}

/// Generate the script for `xml` and parse it, without running anything. Definition load calls
/// this, so an expression that is not JavaScript fails the service there.
///
/// # Errors
///
/// Returns an error when the XML does not generate, or the generated script does not parse.
pub fn check_actions(xml: &str) -> Result<(), Error> {
    if xml.trim().is_empty() {
        return Ok(());
    }
    let script = generated(xml)?;
    super::parse::check_program(script.text())
}

/// The JavaScript the framework runs for an `<actions>` XML text, as the service writer's Generated view shows it
/// (plans/design-dev-aide.md 9.2): with `debug`, each statement after a comment naming its element, as
/// `MOQUI_ACTIONS_DEBUG` writes it. Not cached, and not the cache's.
///
/// # Errors
///
/// Returns an error when the XML does not parse or does not generate.
pub fn generated_source(xml: &str, debug: bool) -> Result<String, Error> {
    if super::runtime::in_script() {
        generate_apart(xml, debug)
    } else {
        generate_with(xml, debug)
    }
}

fn generated(xml: &str) -> Result<ScriptText, Error> {
    let scripts = generated_scripts();
    let cached = scripts.lock().unwrap_or_else(PoisonError::into_inner).get(xml);
    if let Some(script) = cached {
        return Ok(script);
    }
    // Not under the lock: two threads may generate the same text, and the second insert is the same script.
    let script = ScriptText::new(generate(xml)?);
    let bytes = xml.len() + script.text().len();
    scripts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert_sized(xml.to_string(), script.clone(), bytes);
    Ok(script)
}

fn generate(xml: &str) -> Result<String, Error> {
    let debug = std::env::var_os(DEBUG_ENV);
    let source = if super::runtime::in_script() {
        generate_apart(xml, debug.is_some())?
    } else {
        generate_with(xml, debug.is_some())?
    };
    if debug.is_some_and(|value| value == "dump") {
        eprintln!("---- generated action script ----");
        for (index, line) in source.lines().enumerate() {
            eprintln!("{:>4}: {line}", index + 1);
        }
    }
    Ok(source)
}

fn generate_with(xml: &str, debug: bool) -> Result<String, Error> {
    let root = MNode::parse_text("actions", xml)?;
    reject(&root)?;
    super::runtime::with_ctx(|ctx| {
        let name = if debug { "xml-actions-debug" } else { "xml-actions" };
        let macros = super::runtime::engine_object(ctx, name, |ctx| load_macros(ctx, debug))?;
        let (source, _) = render(ctx, &macros, &root)?;
        Ok(source)
    })
}

/// The stack of the thread that generates for a running script, as much as the outermost script on a worker has.
const GENERATOR_STACK: usize = 2 * 1024 * 1024;

type Job = (String, bool, mpsc::Sender<Result<String, Error>>);

/// Generate on a thread of its own. Text first generated inside a script, as the actions of a screen whose definition
/// loads while a template renders a link to it, would otherwise get only what is left of that script's stack, and the
/// macros recurse over the XML. Generation is a function of the text alone, so where it runs does not matter.
fn generate_apart(xml: &str, debug: bool) -> Result<String, Error> {
    static GENERATOR: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    let sender = GENERATOR.get_or_init(|| {
        let (sender, jobs) = mpsc::channel::<Job>();
        let started = std::thread::Builder::new()
            .name("xml-actions-generator".into())
            .stack_size(GENERATOR_STACK)
            .spawn(move || {
                for (xml, debug, reply) in jobs {
                    let _ = reply.send(generate_with(&xml, debug));
                }
            });
        if let Err(err) = started {
            tracing::warn!("could not start the XML actions generator thread: {err}");
        }
        Mutex::new(sender)
    });
    let (reply, result) = mpsc::channel();
    let sent = sender
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send((xml.to_string(), debug, reply));
    match sent {
        Ok(()) => result
            .recv()
            .unwrap_or_else(|_| Err(Error::new("the XML actions generator thread stopped"))),
        Err(_) => generate_with(xml, debug),
    }
}

/// The macro table. The support script evaluates to an object, and the actions script
/// is a function of that object, so no names are left in the global scope.
fn load_macros<'js>(ctx: &Ctx<'js>, debug: bool) -> Result<Object<'js>, Error> {
    let loaded = walker::load_chain(ctx, TEMPLATE_SUPPORT, &[XML_ACTIONS], debug)?;
    loaded
        .into_iter()
        .next_back()
        .ok_or_else(|| Error::new("no XML actions macros"))
}

fn reject(node: &MNode) -> Result<(), Error> {
    if node.has_name("entity-data") {
        return Err(Error::new("entity-data is not implemented"));
    }
    if node.has_name("service-call") && node.attribute_is("async", "distribute") {
        return Err(Error::new("async=\"distribute\" is not a host method"));
    }
    for child in node.child_iter() {
        reject(&child)?;
    }
    Ok(())
}

fn render<'js>(ctx: &Ctx<'js>, macros: &Object<'js>, node: &MNode) -> Result<(String, Object<'js>), Error> {
    let opaque = HashSet::new();
    let walk = Walk {
        macros,
        opaque: &opaque,
        resolve: None,
        missing: "XML action macro",
        fallback: None,
    };
    walker::walk(ctx, &walk, node, None)
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
