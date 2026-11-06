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

//! The expressions of a theme template, checked on their ESTree before any runs (design-shop §12.3, §12.4). A theme
//! expression reads its view and calls the pure helpers; it names no framework object (`ec`, `sri`, `context`), no
//! escape hatch (`raw`, the renderer's `$` internals), no way to the global object or to new code, and makes no
//! function bodies, assignments or objects with `new`. The locked sandbox stops what gets past this (it holds nothing
//! but the snapshot), and the output check refuses unsafe markup however it was made.

use oxc_parser::{ParseOptions, Parser};
use serde_json::Value as Json;

/// One reason an expression is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub code: &'static str,
    pub message: String,
}

/// Names a theme expression may not use, as a variable or a property. Browser globals that are not in the sandbox
/// (`window`, `document`) are left to it, since `parent` and `top` are also names of data.
const DENIED: &[&str] = &[
    "ec",
    "sri",
    "context",
    "raw",
    "globalThis",
    "eval",
    "Function",
    "AsyncFunction",
    "GeneratorFunction",
    "constructor",
    "__proto__",
    "prototype",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "defineProperty",
    "defineProperties",
    "setPrototypeOf",
    "getPrototypeOf",
    "Reflect",
    "Proxy",
    "Symbol",
    "arguments",
    "require",
    "import",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "setTimeout",
    "setInterval",
    "queueMicrotask",
    "Atomics",
    "SharedArrayBuffer",
    "WebAssembly",
];

/// Whether `name` is one a theme cannot use: a denied name, or the renderer's own (`$o`, `$h`, `$scope`...).
pub(crate) fn is_denied_name(name: &str) -> bool {
    name.starts_with('$') || DENIED.contains(&name)
}

/// The problems of one expression; none when it may run.
pub(crate) fn check(expression: &str) -> Vec<Problem> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return vec![Problem {
            code: "syntax",
            message: "the expression is empty".to_string(),
        }];
    }
    let source = format!("({trimmed}\n)");
    let allocator = oxc_allocator::Allocator::default();
    let parsed = Parser::new(&allocator, &source, oxc_span::SourceType::default())
        .with_options(ParseOptions::default())
        .parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() || parsed.program.body.len() != 1 {
        return vec![Problem {
            code: "syntax",
            message: format!("`{}` is not a JavaScript expression", clip(trimmed)),
        }];
    }
    let json = parsed.program.to_estree_json(false, false);
    let Ok(tree) = serde_json::from_str::<Json>(&json) else {
        return vec![Problem {
            code: "syntax",
            message: "the expression could not be read".to_string(),
        }];
    };
    let mut found = Vec::new();
    walk(&tree, &mut found);
    found.dedup();
    found
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(60).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

fn walk(node: &Json, found: &mut Vec<Problem>) {
    match node {
        Json::Array(items) => {
            for item in items {
                walk(item, found);
            }
        }
        Json::Object(map) => {
            let kind = map.get("type").and_then(Json::as_str).unwrap_or("");
            let refuse = |what: &str| Problem {
                code: "forbidden-expression",
                message: format!("{what} is not allowed in a theme expression"),
            };
            match kind {
                "Identifier" | "IdentifierReference" | "IdentifierName" | "BindingIdentifier" | "PrivateIdentifier" => {
                    if let Some(name) = map.get("name").and_then(Json::as_str) {
                        if is_denied_name(name) {
                            found.push(Problem {
                                code: "forbidden-name",
                                message: format!("`{name}` is not part of a theme's view"),
                            });
                        }
                    }
                }
                "MemberExpression" | "ComputedMemberExpression" => {
                    // `x["constructor"]` and x[`constructor`] name the property as a string. A name built at run time
                    // (`x["con" + "structor"]`) is not seen here; the frozen sandbox holds nothing it could reach.
                    if map.get("computed").and_then(Json::as_bool) == Some(true) {
                        let property = map.get("property");
                        let kind = property
                            .and_then(|property| property.get("type"))
                            .and_then(Json::as_str);
                        let named = match kind {
                            Some("Literal") => property.and_then(|property| property.get("value")),
                            Some("TemplateLiteral") => property
                                .filter(|property| {
                                    property
                                        .get("expressions")
                                        .and_then(Json::as_array)
                                        .is_some_and(Vec::is_empty)
                                })
                                .and_then(|property| property.get("quasis"))
                                .and_then(|quasis| quasis.get(0))
                                .and_then(|quasi| quasi.get("value"))
                                .and_then(|value| value.get("cooked")),
                            _ => None,
                        };
                        if let Some(name) = named.and_then(Json::as_str) {
                            if is_denied_name(name) {
                                found.push(Problem {
                                    code: "forbidden-name",
                                    message: format!("`{name}` is not part of a theme's view"),
                                });
                            }
                        }
                    }
                }
                "ThisExpression" => found.push(refuse("`this`")),
                "NewExpression" => found.push(refuse("`new`")),
                "FunctionExpression" | "FunctionDeclaration" => found.push(refuse("a function")),
                "ClassExpression" | "ClassDeclaration" => found.push(refuse("a class")),
                "AssignmentExpression" => found.push(refuse("an assignment (use <#assign>)")),
                "UpdateExpression" => found.push(refuse("`++` and `--`")),
                "ImportExpression" | "MetaProperty" => found.push(refuse("`import`")),
                "YieldExpression" | "AwaitExpression" => found.push(refuse("`yield` and `await`")),
                "TaggedTemplateExpression" => found.push(refuse("a tagged template")),
                "Super" => found.push(refuse("`super`")),
                "ArrowFunctionExpression" => {
                    if map.get("body").and_then(|body| body.get("type")).and_then(Json::as_str)
                        == Some("BlockStatement")
                    {
                        found.push(refuse("an arrow function with a body (only `x => expression`)"));
                    }
                }
                "UnaryExpression" if map.get("operator").and_then(Json::as_str) == Some("delete") => {
                    found.push(refuse("`delete`"));
                }
                _ => {}
            }
            for (key, value) in map {
                if key != "type" {
                    walk(value, found);
                }
            }
        }
        _ => {}
    }
}
