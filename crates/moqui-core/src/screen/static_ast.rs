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

//! The static half of the server-static compatibility check (`plans/archive/QvtServerStatic.md`): an expression the static macros
//! write into a template is parsed with `oxc_parser`, and its ESTree JSON is walked for what the browser host cannot do.
//! The walk is static. A method chosen at runtime is not seen, and the full check (a render with the wrapped `ec`) is
//! the other half.

use oxc_parser::{ParseOptions, Parser};
use serde_json::Value as Json;

/// One thing in an expression that a browser cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AstViolation {
    /// `ec`, `sri`, `context`, `decimal`, `utilities` or `server-global`.
    pub kind: &'static str,
    /// The call or member path, such as `ec.entity.find`.
    pub operation: String,
}

/// `ec` members the browser host has, by area. A member of another area, or another member of one of these, throws in the browser.
const EC_ALLOWED: &[(&str, &[&str])] = &[
    (
        "l10n",
        &["localize", "format", "formatCurrency", "formatCurrencyNoSymbol"],
    ),
    (
        "user",
        &[
            "userId",
            "username",
            "locale",
            "timeZone",
            "userAccount",
            "getPreference",
        ],
    ),
    ("resource", &["expand", "expandNoL10n"]),
    ("factory", &["moquiVersion"]),
];

/// Utility calls the browser host has, the same pure functions as on the server.
const UTILITIES_ALLOWED: &[(&str, &str)] = &[("StringUtilities", "camelCaseToPretty")];

/// Methods of a decimal host value. A browser holds a decimal as a string, so a call to one of these is arithmetic the
/// package cannot do.
const DECIMAL_METHODS: &[&str] = &["add", "subtract", "multiply", "divide", "compare", "isZero", "setScale"];

const UTILITIES: &[&str] = &[
    "ObjectUtilities",
    "CollectionUtilities",
    "StringUtilities",
    "WebUtilities",
    "BigDecimal",
];

const SERVER_GLOBALS: &[&str] = &["System", "Thread", "Packages", "java", "groovy", "javax"];

/// The violations of one expression. An expression that does not parse gives none: the screen load checks that.
pub fn check_expression(expression: &str) -> Vec<AstViolation> {
    let source = format!("({}\n)", expression.trim());
    let allocator = std::default::Default::default();
    let source_type = std::default::Default::default();
    let options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, &source, source_type)
        .with_options(options)
        .parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return Vec::new();
    }
    let json = parsed.program.to_estree_json(false, false);
    let Ok(tree) = serde_json::from_str::<Json>(&json) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(&tree, false, &mut found);
    found.dedup();
    found
}

/// The dotted path of a member chain that starts at an identifier: `ec.entity.find`. None when the chain starts elsewhere.
fn member_path(node: &Json) -> Option<Vec<String>> {
    match node.get("type")?.as_str()? {
        "Identifier" => Some(vec![node.get("name")?.as_str()?.to_string()]),
        "MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression" => {
            let mut path = member_path(node.get("object")?)?;
            let property = node.get("property")?;
            if node.get("computed").and_then(Json::as_bool) == Some(true) {
                match property.get("type").and_then(Json::as_str) {
                    Some("Literal") => path.push(property.get("value")?.as_str()?.to_string()),
                    _ => path.push("[]".to_string()),
                }
            } else {
                path.push(property.get("name")?.as_str()?.to_string());
            }
            Some(path)
        }
        "ChainExpression" => member_path(node.get("expression")?),
        _ => None,
    }
}

fn check_path(path: &[String], found: &mut Vec<AstViolation>) {
    let operation = path.join(".");
    match path.first().map(String::as_str) {
        Some("ec") => {
            let allowed = path.get(1).is_some_and(|area| {
                EC_ALLOWED.iter().any(|(name, members)| {
                    name == area && path.get(2).is_some_and(|member| members.contains(&member.as_str()))
                })
            });
            if !allowed {
                found.push(AstViolation { kind: "ec", operation });
            }
        }
        Some("sri") => found.push(AstViolation { kind: "sri", operation }),
        Some("context") => found.push(AstViolation {
            kind: "context",
            operation,
        }),
        Some(name) if UTILITIES.contains(&name) => {
            let allowed = matches!(path, [_, called] if UTILITIES_ALLOWED
                .iter()
                .any(|(object, method)| *object == name && called == method));
            if !allowed {
                found.push(AstViolation {
                    kind: "utilities",
                    operation,
                });
            }
        }
        Some(name) if SERVER_GLOBALS.contains(&name) => found.push(AstViolation {
            kind: "server-global",
            operation,
        }),
        _ => {
            if let Some(last) = path.last() {
                if path.len() > 1 && DECIMAL_METHODS.contains(&last.as_str()) {
                    found.push(AstViolation {
                        kind: "decimal",
                        operation,
                    });
                }
            }
        }
    }
}

/// Visit a node. `inside_chain` is true for the object of a member expression, whose path the outer node already checked.
fn walk(node: &Json, inside_chain: bool, found: &mut Vec<AstViolation>) {
    match node {
        Json::Array(items) => {
            for item in items {
                walk(item, false, found);
            }
        }
        Json::Object(map) => {
            let kind = map.get("type").and_then(Json::as_str).unwrap_or("");
            match kind {
                "Identifier" if !inside_chain => {
                    if let Some(name) = map.get("name").and_then(Json::as_str) {
                        check_path(&[name.to_string()], found);
                    }
                }
                "MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression" => {
                    if !inside_chain {
                        if let Some(path) = member_path(node) {
                            check_path(&path, found);
                        }
                    }
                    if let Some(object) = map.get("object") {
                        // A chain that starts at an identifier was checked as a path. Any other object is walked in full.
                        walk(object, member_path(object).is_some(), found);
                    }
                    if map.get("computed").and_then(Json::as_bool) == Some(true) {
                        if let Some(property) = map.get("property") {
                            walk(property, false, found);
                        }
                    }
                }
                _ => {
                    for (key, value) in map {
                        if key != "type" {
                            walk(value, false, found);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// The names a compiled static template reads from its scope, which the actions package is limited to
/// (plans/archive/Security-Review-Medium.md SEC-RTE-02).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScopeReads {
    /// Each top-level name read: `_ctx.name`, or `_ctx.$scope['name']` with a literal name.
    pub names: std::collections::BTreeSet<String>,
    /// The text of each read whose name is computed (`_ctx[expr]`, `_ctx.$scope[expr]`) or of `$scope` passed on whole,
    /// whose names cannot be known.
    pub computed: Vec<String>,
}

/// The scope reads of `script`, the render function the Vue compiler wrote with `prefixIdentifiers`: every free name of a
/// template expression is `_ctx.name`, and `$scope` is the scope itself. A slot that declares its own `_ctx` (`m-scope`,
/// a form-list row) reads the same chain, so its reads are recorded the same way. A script that does not parse gives
/// none, with the parse error as its one computed read.
pub(crate) fn scope_reads(script: &str) -> ScopeReads {
    crate::script::parse::with_arena(|allocator| {
        // the compiler's code is the body of a function of `Vue`, which returns the render function
        let options = ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        };
        let parsed = Parser::new(allocator, script, std::default::Default::default())
            .with_options(options)
            .parse();
        let mut reads = ScopeReads::default();
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            reads.computed.push("the script does not parse".to_string());
            return reads;
        }
        match serde_json::from_str::<Json>(&parsed.program.to_estree_json(false, false)) {
            Ok(tree) => walk_reads(&tree, false, script, &mut reads),
            Err(err) => reads.computed.push(format!("the script tree could not be read: {err}")),
        }
        reads
    })
}

/// The source text of a node, by its span.
fn source_of<'a>(node: &Json, script: &'a str) -> &'a str {
    let at = |key: &str| {
        node.get(key)
            .and_then(Json::as_u64)
            .and_then(|at| usize::try_from(at).ok())
    };
    match (at("start"), at("end")) {
        (Some(start), Some(end)) => script.get(start..end).unwrap_or(""),
        _ => "",
    }
}

/// Visit a node. `inside_chain` is true for the object of a member expression, whose chain the outer node recorded.
fn walk_reads(node: &Json, inside_chain: bool, script: &str, reads: &mut ScopeReads) {
    match node {
        Json::Array(items) => {
            for item in items {
                walk_reads(item, false, script, reads);
            }
        }
        Json::Object(map) => {
            let kind = map.get("type").and_then(Json::as_str).unwrap_or("");
            match kind {
                "MemberExpression" | "StaticMemberExpression" | "ComputedMemberExpression" => {
                    if !inside_chain {
                        if let Some(path) = member_path(node) {
                            record_read(&path, node, script, reads);
                        }
                    }
                    if let Some(object) = map.get("object") {
                        walk_reads(object, member_path(object).is_some(), script, reads);
                    }
                    if map.get("computed").and_then(Json::as_bool) == Some(true) {
                        if let Some(property) = map.get("property") {
                            walk_reads(property, false, script, reads);
                        }
                    }
                }
                _ => {
                    for (key, value) in map {
                        // a function's parameters declare names (`(_ctx) => ...`), they read none
                        if key != "type" && key != "params" {
                            walk_reads(value, false, script, reads);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Record a whole member chain that starts at an identifier.
fn record_read(path: &[String], node: &Json, script: &str, reads: &mut ScopeReads) {
    let [root, first, rest @ ..] = path else {
        return;
    };
    if root != "_ctx" {
        return;
    }
    match (first.as_str(), rest.first().map(String::as_str)) {
        // `_ctx[expr]`, `_ctx.$scope[expr]`, or `_ctx.$scope` passed on whole
        ("[]", _) | ("$scope", Some("[]") | None) => reads.computed.push(source_of(node, script).to_string()),
        ("$scope", Some(name)) | (name, _) => {
            reads.names.insert(name.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{check_expression, scope_reads};

    #[test]
    fn the_scope_reads_of_a_render_function_are_its_top_level_names() {
        let reads = scope_reads(
            "const { withCtx: _withCtx } = Vue\n\nreturn function render(_ctx, _cache) { return [_ctx.greeting, _ctx.item.name, \
             _ctx.$scope['mqExtra_F'], _ctx.ec.resource.expand('x ${a}', {'a': () => (_ctx.a)}), \
             _withCtx((_ctx) => [_ctx.$scope['row'], formProps.fields.x]), (_ctx.$scope['mqSection_S']||[])[i], _ctx.$scope.dotted] }",
        );
        let names: Vec<&str> = reads.names.iter().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "a",
                "dotted",
                "ec",
                "greeting",
                "item",
                "mqExtra_F",
                "mqSection_S",
                "row"
            ]
        );
        assert!(reads.computed.is_empty(), "{:?}", reads.computed);
        let reads =
            scope_reads("function render(_ctx) { return [_ctx.$scope[_ctx.key + '_x'], _ctx[name], f(_ctx.$scope)] }");
        assert_eq!(
            reads.computed,
            ["_ctx.$scope[_ctx.key + '_x']", "_ctx[name]", "_ctx.$scope"],
            "{reads:?}"
        );
        assert!(reads.names.contains("key"), "{reads:?}");
    }

    fn operations(expression: &str) -> Vec<String> {
        check_expression(expression)
            .into_iter()
            .map(|violation| format!("{}:{}", violation.kind, violation.operation))
            .collect()
    }

    #[test]
    fn the_host_calls_and_plain_javascript_are_clean() {
        for clean in [
            "description || ''",
            "ec.l10n.format(amount, '#,##0.00')",
            "ec.l10n.formatCurrency(price, currencyUomId)",
            "ec.user.userAccount.partyId",
            "ec.user.getPreference('X') == 'true'",
            "list.map(x => x.name).join(', ')",
            "Math.max(a, b) > 3 && !done",
            "`${a}-${b}`",
            "ec.resource.expand('x ${y}', {})",
            "ec.factory.moquiVersion",
            "StringUtilities.camelCaseToPretty(fieldName)",
        ] {
            assert!(operations(clean).is_empty(), "{clean}: {:?}", operations(clean));
        }
    }

    #[test]
    fn what_a_browser_cannot_do_is_named() {
        assert_eq!(operations("ec.entity.find('A').one()"), ["ec:ec.entity.find"]);
        assert_eq!(operations("ec.web.getRequest()"), ["ec:ec.web.getRequest"]);
        assert_eq!(operations("ec.user.nowTimestamp"), ["ec:ec.user.nowTimestamp"]);
        assert_eq!(operations("sri.buildUrl('x').url"), ["sri:sri.buildUrl"]);
        assert_eq!(operations("amount.add(tax)"), ["decimal:amount.add"]);
        assert_eq!(
            operations("ObjectUtilities.compareObj(a, b)"),
            ["utilities:ObjectUtilities.compareObj"]
        );
        assert_eq!(operations("context.foo"), ["context:context.foo"]);
        assert_eq!(
            operations("StringUtilities.prettyToCamelCase(a, true)"),
            ["utilities:StringUtilities.prettyToCamelCase"]
        );
        assert_eq!(
            operations("System.getProperty('a')"),
            ["server-global:System.getProperty"]
        );
        // each use is found, in a condition and in an argument
        let mut both = operations("a ? ec.entity.find('B') : sri.getScreenUrlInstance()");
        both.sort();
        assert_eq!(both, ["ec:ec.entity.find", "sri:sri.getScreenUrlInstance"]);
        assert!(
            operations("a b").is_empty(),
            "text that is not JavaScript is the loader's to report"
        );
    }
}
