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

//! Conf values the user and authorization code reads on every call, parsed once per factory.

use std::collections::HashMap;

use moqui_lib::node::MNode;

/// One `response-header` of a `webapp`: the response type it is for, the header, its raw value (`${}` not yet expanded),
/// and whether it is added to the header's values rather than replacing them (`add="true"`).
#[derive(Clone, Debug)]
pub(crate) struct ResponseHeader {
    pub kind: String,
    pub name: String,
    pub value: String,
    pub add: bool,
}

#[derive(Default)]
pub(super) struct ConfCache {
    /// `artifact-execution` attributes: type, then attribute, to true when the value is `true`. Nested, so a lookup by
    /// `&str` makes no key.
    artifact: HashMap<String, HashMap<String, bool>>,
    /// `user-facade` child attributes: `(child, attribute)` to the non-empty value.
    user_facade: HashMap<(String, String), String>,
    /// `webapp` attributes: `(webapp name, attribute)` to the raw value, `${}` not yet expanded.
    webapp: HashMap<(String, String), String>,
    /// `web-protection` attributes of a `webapp`: `(webapp name, child, attribute)` to the raw value, the child empty
    /// for an attribute of `web-protection` itself.
    protection: HashMap<(String, String, String), String>,
    /// Other `webapp` children: `(webapp name, child, attribute)` to the raw value. The child is the element name,
    /// `endpoint path` for an `endpoint` (`endpoint /notws`), or `error-screen error` for an `error-screen`
    /// (`error-screen not-found`).
    webapp_child: HashMap<(String, String, String), String>,
    /// The `response-header` entries of each `webapp`, in conf order.
    response_headers: HashMap<String, Vec<ResponseHeader>>,
    /// Attributes of the first element at each path of up to three names below the root (`ssh-server`,
    /// `search-facade/cluster`): path, then attribute, to the raw value.
    elements: HashMap<String, HashMap<String, String>>,
}

impl ConfCache {
    pub(super) fn parse(text: &str) -> Self {
        let mut cache = Self::default();
        if text.trim().is_empty() {
            return cache;
        }
        let Ok(root) = MNode::parse_text("moqui-conf", text) else {
            return cache;
        };
        if let Some(facade) = root.first_named("artifact-execution-facade") {
            for node in facade.children_named("artifact-execution") {
                let Some(artifact_type) = node.attribute("type") else {
                    continue;
                };
                for (name, value) in node.attributes() {
                    // The first entry for a type wins, as the Java conf lookup does.
                    cache
                        .artifact
                        .entry(artifact_type.clone())
                        .or_default()
                        .entry(name)
                        .or_insert(value == "true");
                }
            }
        }
        if let Some(facade) = root.first_named("user-facade") {
            for child in facade.children() {
                for (name, value) in child.attributes() {
                    if !value.is_empty() {
                        cache.user_facade.entry((child.name(), name)).or_insert(value);
                    }
                }
            }
        }
        record_elements(&mut cache.elements, &root, "", 0);
        if let Some(list) = root.first_named("webapp-list") {
            for node in list.children_named("webapp") {
                let Some(webapp) = node.attribute("name") else {
                    continue;
                };
                for (name, value) in node.attributes() {
                    cache.webapp.entry((webapp.clone(), name)).or_insert(value);
                }
                for child in node.children() {
                    let key = match child.name().as_str() {
                        "endpoint" => format!("endpoint {}", child.attribute("path").unwrap_or_default()),
                        "error-screen" => format!("error-screen {}", child.attribute("error").unwrap_or_default()),
                        "web-protection" => continue,
                        "response-header" => {
                            let attr = |name: &str| child.attribute(name).unwrap_or_default();
                            cache
                                .response_headers
                                .entry(webapp.clone())
                                .or_default()
                                .push(ResponseHeader {
                                    kind: attr("type"),
                                    name: attr("name"),
                                    value: attr("value"),
                                    add: attr("add") == "true",
                                });
                            continue;
                        }
                        name => name.to_string(),
                    };
                    for (name, value) in child.attributes() {
                        cache
                            .webapp_child
                            .entry((webapp.clone(), key.clone(), name))
                            .or_insert(value);
                    }
                }
                if let Some(protection) = node.first_named("web-protection") {
                    for (name, value) in protection.attributes() {
                        cache
                            .protection
                            .entry((webapp.clone(), String::new(), name))
                            .or_insert(value);
                    }
                    for child in protection.children() {
                        for (name, value) in child.attributes() {
                            cache
                                .protection
                                .entry((webapp.clone(), child.name(), name))
                                .or_insert(value);
                        }
                    }
                }
            }
        }
        cache
    }

    pub(super) fn artifact_flag(&self, artifact_type: &str, flag: &str) -> bool {
        self.artifact
            .get(artifact_type)
            .and_then(|flags| flags.get(flag))
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn user_facade_attr(&self, child: &str, name: &str) -> Option<String> {
        self.user_facade.get(&(child.to_string(), name.to_string())).cloned()
    }

    pub(super) fn webapp_attr(&self, webapp: &str, name: &str) -> Option<String> {
        self.webapp.get(&(webapp.to_string(), name.to_string())).cloned()
    }

    pub(super) fn webapp_child_attr(&self, webapp: &str, child: &str, name: &str) -> Option<String> {
        self.webapp_child
            .get(&(webapp.to_string(), child.to_string(), name.to_string()))
            .cloned()
    }

    /// The `response-header` entries of a webapp, None when it declares none.
    pub(super) fn response_headers(&self, webapp: &str) -> Option<&[ResponseHeader]> {
        self.response_headers.get(webapp).map(Vec::as_slice)
    }

    pub(super) fn element_attr(&self, path: &str, name: &str) -> Option<String> {
        self.elements.get(path).and_then(|attrs| attrs.get(name)).cloned()
    }

    pub(super) fn protection_attr(&self, webapp: &str, child: &str, name: &str) -> Option<String> {
        self.protection
            .get(&(webapp.to_string(), child.to_string(), name.to_string()))
            .cloned()
    }
}

/// The attributes of the first element at each path below `node`, to three names deep.
fn record_elements(elements: &mut HashMap<String, HashMap<String, String>>, node: &MNode, path: &str, depth: usize) {
    if depth == 3 {
        return;
    }
    for child in node.children() {
        let child_path = if path.is_empty() {
            child.name()
        } else {
            format!("{path}/{}", child.name())
        };
        if elements.contains_key(&child_path) {
            continue;
        }
        elements.insert(child_path.clone(), child.attributes().into_iter().collect());
        record_elements(elements, &child, &child_path, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::ConfCache;

    #[test]
    fn elements_by_path_and_webapp_children_by_name_or_endpoint_path() {
        let cache = ConfCache::parse(
            r#"<moqui-conf><search-facade backend="postgres"><cluster url="http://x" start-wait="30"/></search-facade>
                <git-server max-concurrent="2"/><git-server max-concurrent="9"/>
                <webapp-list><webapp name="webroot"><session-config timeout="30"/>
                    <endpoint path="/notws" enabled="true" timeout="1000"/><endpoint path="/termws" enabled="false"/>
                </webapp></webapp-list></moqui-conf>"#,
        );
        assert_eq!(cache.element_attr("search-facade", "backend").as_deref(), Some("postgres"));
        assert_eq!(cache.element_attr("search-facade/cluster", "start-wait").as_deref(), Some("30"));
        // the first element at a path wins
        assert_eq!(cache.element_attr("git-server", "max-concurrent").as_deref(), Some("2"));
        assert_eq!(cache.element_attr("ssh-server", "port"), None);
        assert_eq!(cache.webapp_child_attr("webroot", "session-config", "timeout").as_deref(), Some("30"));
        assert_eq!(cache.webapp_child_attr("webroot", "endpoint /notws", "timeout").as_deref(), Some("1000"));
        assert_eq!(cache.webapp_child_attr("webroot", "endpoint /termws", "enabled").as_deref(), Some("false"));
    }
}
