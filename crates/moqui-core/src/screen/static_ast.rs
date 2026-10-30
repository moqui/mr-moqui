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
    if parsed.panicked || parsed.diagnostics.has_errors() {
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
        Json::Array(items) => items.iter().for_each(|item| walk(item, false, found)),
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

#[cfg(test)]
mod tests {
    use super::check_expression;

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
