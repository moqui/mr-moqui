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

//! Conf merge. Key rules follow `ExecutionContextFactoryImpl.mergeConfigNodes`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use moqui_lib::node::MNode;

use super::error::Error;

/// The runtime conf merged over the default, like Java's `moqui.conf`. A process property `MOQUI_CONF` names it,
/// then the environment variable of the same name. Relative to the runtime directory or absolute, and a named file
/// that is missing is an error. Unset or empty means the instance's `MoquiConf.xml` in the runtime directory (beside the
/// binary in an archive from `./moq archive`) when it exists, else `conf/MoquiDevConf.xml` when that exists.
/// `runtime/.env` is applied after the conf merge, so it cannot select the conf.
pub(super) fn runtime_conf(runtime_dir: &Path) -> Result<Option<PathBuf>, Error> {
    let from_property = moqui_lib::env::get_property("MOQUI_CONF").map(std::ffi::OsString::from);
    let selected = from_property.or_else(|| std::env::var_os("MOQUI_CONF"));
    runtime_conf_from(runtime_dir, selected.as_deref())
}

pub(super) fn runtime_conf_from(runtime_dir: &Path, selected: Option<&OsStr>) -> Result<Option<PathBuf>, Error> {
    match selected.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = runtime_dir.join(value);
            if path.is_file() {
                Ok(Some(path))
            } else {
                Err(Error::new(format!("MOQUI_CONF file {} not found", path.display())))
            }
        }
        None => {
            let instance = runtime_dir.join("MoquiConf.xml");
            if instance.is_file() {
                return Ok(Some(instance));
            }
            Ok(Some(runtime_dir.join("conf/MoquiDevConf.xml")).filter(|path| path.is_file()))
        }
    }
}

pub fn load_merged_conf(framework_dir: &Path, runtime_dir: &Path) -> Result<MNode, Error> {
    let base = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
    if let Some(runtime_conf) = runtime_conf(runtime_dir)? {
        tracing::info!("Merging runtime conf {}", runtime_conf.display());
        let over = MNode::parse_file(&runtime_conf)?;
        merge_conf(&base, &over);
    }
    Ok(base)
}

/// The conf with each component's `MoquiConf.xml` merged in component order, then the runtime conf on top again, as
/// Java merges them: default, components, runtime. `components` come from the conf `load_merged_conf` returned.
/// `test_confs` (a `moqui test` run, [`super::BootOptions::test_confs`]) merges each component's `test/MoquiConf.xml`
/// right after its own `MoquiConf.xml` (plans/archive/ClientTestHarness.md 6).
pub fn merge_component_confs(
    framework_dir: &Path,
    runtime_dir: &Path,
    components: &[std::path::PathBuf],
    test_confs: bool,
) -> Result<Option<MNode>, Error> {
    let files: Vec<_> = components
        .iter()
        .flat_map(|dir| {
            let test = test_confs.then(|| dir.join("test/MoquiConf.xml"));
            std::iter::once(dir.join("MoquiConf.xml")).chain(test)
        })
        .filter(|file| file.is_file())
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    let base = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
    for file in &files {
        tracing::info!("Merging component conf {}", file.display());
        merge_conf(&base, &MNode::parse_file(file)?);
    }
    if let Some(runtime_conf) = runtime_conf(runtime_dir)? {
        merge_conf(&base, &MNode::parse_file(&runtime_conf)?);
    }
    Ok(Some(base))
}

/// Merge `over` into `base`. `component-list` children are appended.
pub fn merge_conf(base: &MNode, over: &MNode) {
    merge_config_component_nodes(base, over);
    merge_config_nodes(base, over);
}

fn merge_config_component_nodes(base: &MNode, over: &MNode) {
    let Some(over_list) = over.first_named("component-list") else {
        return;
    };
    let base_list = if let Some(existing) = base.first_named("component-list") {
        existing
    } else {
        base.append("component-list")
    };
    for child in over_list.children() {
        base_list.append_node(child.deep_copy());
    }
}

fn merge_config_nodes(base: &MNode, over: &MNode) {
    base.merge_children_by_keys(over, "default-property", &["name"], None);
    base.merge_child_with_child_key(over, "tools", "tool-factory", &["class"], None);
    base.merge_child_with_child_key(over, "cache-list", "cache", &["name"], None);
    base.merge_child_with_child_key(over, "telemetry", "artifact", &["type"], None);
    if let (Some(base_telemetry), Some(over_telemetry)) = (base.first_named("telemetry"), over.first_named("telemetry"))
    {
        base_telemetry.merge_single_child(&over_telemetry, "receiver");
    }
    merge_server_stats(base, over);
    merge_webapps(base, over);
    base.merge_child_with_child_key(over, "artifact-execution-facade", "artifact-execution", &["type"], None);
    merge_user_facade(base, over);
    merge_transaction_facade(base, over);
    merge_resource_facade(base, over);
    merge_screen_facade(base, over);
    merge_service_facade(base, over);
    merge_search_facade(base, over);
    merge_llm_facade(base, over);
    merge_entity_facade(base, over);
    merge_database_list(base, over);
    merge_repository_list(base, over);
    // elements of settings only: an attribute of the override replaces the base one
    for name in [
        "email-client",
        "ssh-server",
        "terminal",
        "git-server",
        "dav-server",
        "smtp-server",
        "logging",
        "report",
        // the dev tools (plans/design-dev-aide.md 16): the file watcher, and the dev-aide component's settings
        "dev-reload",
        "dev-aide",
    ] {
        base.merge_single_child(over, name);
    }
}

fn merge_server_stats(base: &MNode, over: &MNode) {
    let Some(over_stats) = over.first_named("server-stats") else {
        return;
    };
    let Some(base_stats) = base.first_named("server-stats") else {
        base.append_node(over_stats.deep_copy());
        return;
    };
    base_stats.copy_attributes_from(&over_stats);
    for child in over_stats.children_named("artifact-stats") {
        let found = base_stats
            .children_named("artifact-stats")
            .into_iter()
            .find(|node| same_attr(node, &child, "type") && same_attr(node, &child, "sub-type"));
        if let Some(base_child) = found {
            base_child.copy_attributes_from(&child);
        } else {
            base_stats.append_node(child.deep_copy());
        }
    }
}

fn merge_webapps(base: &MNode, over: &MNode) {
    let mut merge_webapp = |base_web: &MNode, over_web: &MNode| {
        merge_webapp_children(base_web, over_web);
    };
    base.merge_child_with_child_key(over, "webapp-list", "webapp", &["name"], Some(&mut merge_webapp));
}

fn merge_webapp_children(base: &MNode, over: &MNode) {
    base.merge_children_by_keys(over, "root-screen", &["host"], None);
    base.merge_children_by_keys(over, "error-screen", &["error"], None);
    // anonymous token routes (screen/token_route.rs): a component's route of the same name replaces the earlier one
    base.merge_children_by_keys(over, "token-route", &["name"], None);
    base.merge_children_by_keys(over, "public-site", &["host"], None);
    for name in [
        "first-hit-in-visit",
        "after-request",
        "before-request",
        "after-login",
        "before-logout",
        "after-startup",
        "before-shutdown",
    ] {
        merge_webapp_actions(base, over, name);
    }
    let mut merge_filter = |base_child: &MNode, over_child: &MNode| {
        merge_init_params(base_child, over_child);
        append_named(base_child, over_child, "url-pattern");
        append_named(base_child, over_child, "dispatcher");
    };
    base.merge_children_by_keys(over, "filter", &["name"], Some(&mut merge_filter));
    base.merge_children_by_keys(over, "listener", &["class"], None);
    let mut merge_servlet = |base_child: &MNode, over_child: &MNode| {
        merge_init_params(base_child, over_child);
        append_named(base_child, over_child, "url-pattern");
    };
    base.merge_children_by_keys(over, "servlet", &["name"], Some(&mut merge_servlet));
    base.merge_single_child(over, "session-config");
    base.merge_children_by_keys(over, "endpoint", &["path"], None);
    merge_response_headers(base, over);
    merge_web_protection(base, over);
}

/// `web-protection`: its attributes and its single children (`connections`, `requests`, `overload`) merge
/// attributes, `rate-limit` and `ban` merge by `name`, and a rule merges by `name` when it has one and appends when
/// it has none.
fn merge_web_protection(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("web-protection") else {
        return;
    };
    let Some(base_node) = base.first_named("web-protection") else {
        base.append_node(over_node.deep_copy());
        return;
    };
    base_node.copy_attributes_from(&over_node);
    for single in ["connections", "requests", "overload"] {
        base_node.merge_single_child(&over_node, single);
    }
    base_node.merge_children_by_keys(&over_node, "rate-limit", &["name"], None);
    base_node.merge_children_by_keys(&over_node, "ban", &["name"], None);
    for rule in ["client-rule", "path-rule", "user-agent-rule"] {
        base_node.merge_children_by_keys(&over_node, rule, &["name"], None);
        for child in over_node.children_named(rule) {
            if child.attribute("name").is_none_or(|name| name.is_empty()) {
                base_node.append_node(child.deep_copy());
            }
        }
    }
}

fn merge_init_params(base: &MNode, over: &MNode) {
    base.merge_children_by_keys(over, "init-param", &["name"], None);
}

fn merge_webapp_actions(base: &MNode, over: &MNode, name: &str) {
    let Some(over_child) = over.first_named(name) else {
        return;
    };
    let Some(over_actions) = over_child.first_named("actions") else {
        return;
    };
    let kids = over_actions.children();
    if kids.is_empty() {
        return;
    }
    let child = base.first_named(name).unwrap_or_else(|| base.append(name));
    let actions = child.first_named("actions").unwrap_or_else(|| child.append("actions"));
    for node in kids {
        actions.append_node(node.deep_copy());
    }
}

fn merge_response_headers(base: &MNode, over: &MNode) {
    for child in over.children_named("response-header") {
        let found = base
            .children_named("response-header")
            .into_iter()
            .find(|node| same_attr(node, &child, "type") && same_attr(node, &child, "name"));
        if let Some(base_child) = found {
            base_child.copy_attributes_from(&child);
            base_child.replace_children_from(&child);
        } else {
            base.append_node(child.deep_copy());
        }
    }
}

fn merge_user_facade(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("user-facade") else {
        return;
    };
    let Some(base_node) = base.first_named("user-facade") else {
        base.append_node(over_node.deep_copy());
        return;
    };
    base_node.merge_single_child(&over_node, "password");
    base_node.merge_single_child(&over_node, "login-key");
    base_node.merge_single_child(&over_node, "login");
    base_node.merge_single_child(&over_node, "authz");
}

fn merge_transaction_facade(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("transaction-facade") else {
        return;
    };
    let Some(base_node) = base.first_named("transaction-facade") else {
        base.append_node(over_node.deep_copy());
        return;
    };
    base_node.copy_attributes_from(&over_node);
    base_node.merge_single_child(&over_node, "server-jndi");
    base_node.merge_single_child(&over_node, "transaction-jndi");
    base_node.merge_single_child(&over_node, "transaction-internal");
}

fn merge_resource_facade(base: &MNode, over: &MNode) {
    base.merge_child_with_child_key(over, "resource-facade", "resource-reference", &["scheme"], None);
    base.merge_child_with_child_key(over, "resource-facade", "template-renderer", &["extension"], None);
    base.merge_child_with_child_key(over, "resource-facade", "script-runner", &["extension"], None);
}

fn merge_screen_facade(base: &MNode, over: &MNode) {
    base.merge_child_with_child_key(over, "screen-facade", "screen-text-output", &["type"], None);
    base.merge_child_with_child_key(over, "screen-facade", "screen-output", &["type"], None);
    let mut merge_screen = |base_screen: &MNode, over_screen: &MNode| {
        base_screen.merge_children_by_keys(over_screen, "subscreens-item", &["name"], None);
    };
    base.merge_child_with_child_key(over, "screen-facade", "screen", &["location"], Some(&mut merge_screen));
    if let (Some(base_node), Some(over_node)) = (base.first_named("screen-facade"), over.first_named("screen-facade")) {
        base_node.merge_single_child(&over_node, "pdf");
    }
}

fn merge_service_facade(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("service-facade") else {
        return;
    };
    let Some(base_node) = base.first_named("service-facade") else {
        base.append_node(over_node.deep_copy());
        return;
    };
    base_node.merge_node_with_child_key(&over_node, "service-location", &["name"], None);
    base_node.merge_children_by_keys(&over_node, "service-type", &["name"], None);
    base_node.merge_children_by_keys(&over_node, "service-file", &["location"], None);
    base_node.merge_children_by_keys(&over_node, "startup-service", &["name"], None);
    if let Some(over_pool) = over_node.first_named("thread-pool") {
        if let Some(base_pool) = base_node.first_named("thread-pool") {
            base_pool.merge_node_with_child_key(&over_pool, "run-from-pool", &["name"], None);
        } else {
            base_node.append_node(over_pool.deep_copy());
        }
    }
    append_named(&base_node, &over_node, "jms-service");
}

/// `search-facade` attributes override; a `postgres`, `cluster` or `semantic` child merges into the base one.
fn merge_search_facade(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("search-facade") else {
        return;
    };
    match base.first_named("search-facade") {
        None => base.append_node(over_node.deep_copy()),
        Some(base_node) => {
            base_node.copy_attributes_from(&over_node);
            base_node.merge_single_child(&over_node, "postgres");
            base_node.merge_single_child(&over_node, "cluster");
            base_node.merge_single_child(&over_node, "semantic");
        }
    }
}

/// `llm-facade` attributes override; `endpoint` merges by id (its `header`s by name), `model` by id (its `reasoning`s
/// by level, an `api-config` replaces the base one), `bind` by feature; the settings children merge their attributes,
/// and the `push` and `security` of `a2a` theirs.
fn merge_llm_facade(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("llm-facade") else {
        return;
    };
    let Some(base_node) = base.first_named("llm-facade") else {
        base.append_node(over_node.deep_copy());
        return;
    };
    base_node.copy_attributes_from(&over_node);
    let mut merge_endpoint = |base_endpoint: &MNode, over_endpoint: &MNode| {
        base_endpoint.merge_children_by_keys(over_endpoint, "header", &["name"], None);
    };
    base_node.merge_children_by_keys(&over_node, "endpoint", &["id"], Some(&mut merge_endpoint));
    let mut merge_model = |base_model: &MNode, over_model: &MNode| {
        base_model.merge_children_by_keys(over_model, "reasoning", &["level"], None);
        if let Some(api) = over_model.first_named("api-config") {
            base_model.remove_where(|child| child.has_name("api-config"));
            base_model.append_node(api.deep_copy());
        }
    };
    base_node.merge_children_by_keys(&over_node, "model", &["id"], Some(&mut merge_model));
    base_node.merge_children_by_keys(&over_node, "bind", &["feature"], None);
    base_node.merge_children_by_keys(&over_node, "data-entity", &["package", "entity"], None);
    base_node.merge_children_by_keys(&over_node, "user-facts", &["service"], None);
    base_node.merge_children_by_keys(&over_node, "ai-task", &["name"], None);
    for name in ["limits", "content-log", "screen-sessions", "mcp", "gateway"] {
        base_node.merge_single_child(&over_node, name);
    }
    if let Some(over_a2a) = over_node.first_named("a2a") {
        match base_node.first_named("a2a") {
            None => base_node.append_node(over_a2a.deep_copy()),
            Some(base_a2a) => {
                base_a2a.copy_attributes_from(&over_a2a);
                base_a2a.merge_single_child(&over_a2a, "push");
                base_a2a.merge_single_child(&over_a2a, "security");
            }
        }
    }
}

fn merge_entity_facade(base: &MNode, over: &MNode) {
    let Some(over_node) = over.first_named("entity-facade") else {
        return;
    };
    let Some(base_node) = base.first_named("entity-facade") else {
        base.append_node(over_node.deep_copy());
        return;
    };
    let mut merge_datasource = |base_ds: &MNode, over_ds: &MNode| {
        if over_ds.first_named("jndi-jdbc").is_some() || over_ds.first_named("inline-jdbc").is_some() {
            base_ds.remove_where(|child| child.has_name("jndi-jdbc") || child.has_name("inline-jdbc"));
            if let Some(inline) = over_ds.first_named("inline-jdbc") {
                base_ds.append_node(inline.deep_copy());
            } else if let Some(jndi) = over_ds.first_named("jndi-jdbc") {
                base_ds.append_node(jndi.deep_copy());
            }
        }
    };
    base_node.merge_node_with_child_key(&over_node, "datasource", &["group-name"], Some(&mut merge_datasource));
    base_node.merge_single_child(&over_node, "server-jndi");
    append_named(&base_node, &over_node, "load-entity");
    append_named(&base_node, &over_node, "load-data");
}

fn merge_database_list(base: &MNode, over: &MNode) {
    base.merge_child_with_child_key(over, "database-list", "dictionary-type", &["type"], None);
    let mut merge_database = |base_db: &MNode, over_db: &MNode| {
        base_db.merge_node_with_child_key(over_db, "database-type", &["type"], None);
    };
    base.merge_child_with_child_key(over, "database-list", "database", &["name"], Some(&mut merge_database));
}

fn merge_repository_list(base: &MNode, over: &MNode) {
    let mut merge_repository = |base_repo: &MNode, over_repo: &MNode| {
        base_repo.merge_children_by_keys(over_repo, "init-param", &["name"], None);
    };
    base.merge_child_with_child_key(
        over,
        "repository-list",
        "repository",
        &["name"],
        Some(&mut merge_repository),
    );
}

fn append_named(base: &MNode, over: &MNode, name: &str) {
    for child in over.children_named(name) {
        base.append_node(child.deep_copy());
    }
}

fn same_attr(left: &MNode, right: &MNode, name: &str) -> bool {
    non_empty(left.attribute(name)) == non_empty(right.attribute(name))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

#[cfg(test)]
#[path = "conf_tests.rs"]
mod tests;
