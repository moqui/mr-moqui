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

use super::conf_env::{self, Override};
use super::conf_schema::merge_keys as keys;
use super::Error;

const WEBAPP_LIST: &str = "webapp-list";
const WEBAPP: &str = "webapp";
const PROTECTION: &str = "web-protection";
const RESOURCE: &str = "resource-facade";
const SCREEN: &str = "screen-facade";
const SERVICE: &str = "service-facade";
const LLM: &str = "llm-facade";
const DATABASES: &str = "database-list";

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

/// The runtime conf file this runtime merges ([`runtime_conf`]), for the dev file watcher and the conf reload; `None`
/// when there is none or the one named is missing.
#[must_use]
pub fn runtime_conf_file(runtime_dir: &Path) -> Option<PathBuf> {
    runtime_conf(runtime_dir).ok().flatten()
}

pub(super) fn runtime_conf_from(runtime_dir: &Path, selected: Option<&OsStr>) -> Result<Option<PathBuf>, Error> {
    match selected.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = runtime_dir.join(value);
            if moqui_lib::vfs::is_file(&path) {
                Ok(Some(path))
            } else {
                Err(Error::new(format!("MOQUI_CONF file {} not found", path.display())))
            }
        }
        None => {
            let instance = runtime_dir.join("MoquiConf.xml");
            if moqui_lib::vfs::is_file(&instance) {
                return Ok(Some(instance));
            }
            Ok(Some(runtime_dir.join("conf/MoquiDevConf.xml")).filter(|path| moqui_lib::vfs::is_file(path)))
        }
    }
}

/// The default conf with the runtime conf merged over it and the environment's names applied
/// ([`conf_env::apply_overrides`]): the conf component discovery reads.
///
/// # Errors
///
/// When a conf file does not parse, or the runtime conf `MOQUI_CONF` names is missing.
pub fn load_merged_conf(framework_dir: &Path, runtime_dir: &Path) -> Result<MNode, Error> {
    load_merged_conf_with(framework_dir, runtime_dir).map(|(conf, _)| conf)
}

/// [`load_merged_conf`], with what each environment name did.
///
/// # Errors
///
/// As [`load_merged_conf`].
pub fn load_merged_conf_with(framework_dir: &Path, runtime_dir: &Path) -> Result<(MNode, Vec<Override>), Error> {
    let base = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
    if let Some(runtime_conf) = runtime_conf(runtime_dir)? {
        tracing::info!("Merging runtime conf {}", runtime_conf.display());
        let over = MNode::parse_file(&runtime_conf)?;
        merge_conf(&base, &over);
    }
    let overrides = conf_env::apply_overrides(&base, Some(&runtime_dir.join(".env")));
    Ok((base, overrides))
}

/// The conf with each component's `MoquiConf.xml` merged in component order, then the runtime conf on top again, as
/// Java merges them: default, components, runtime; then the environment's names applied. `components` come from the conf `load_merged_conf` returned.
/// `test_confs` (a `moqui test` run, [`super::BootOptions::test_confs`]) merges each component's `test/MoquiConf.xml`
/// right after its own `MoquiConf.xml` (plans/archive/ClientTestHarness.md 6). `None` when no component has a conf.
pub fn merge_component_confs(
    framework_dir: &Path,
    runtime_dir: &Path,
    components: &[std::path::PathBuf],
    test_confs: bool,
) -> Result<Option<MNode>, Error> {
    merge_component_confs_with(framework_dir, runtime_dir, components, test_confs)
        .map(|merged| merged.map(|(conf, _)| conf))
}

/// [`merge_component_confs`], with what each environment name did.
///
/// # Errors
///
/// When a conf file does not parse, or the runtime conf named is missing.
pub fn merge_component_confs_with(
    framework_dir: &Path,
    runtime_dir: &Path,
    components: &[std::path::PathBuf],
    test_confs: bool,
) -> Result<Option<(MNode, Vec<Override>)>, Error> {
    let named: Vec<(String, PathBuf)> = components
        .iter()
        .map(|dir| (component_name(dir), dir.clone()))
        .collect();
    let files = conf_files_of(framework_dir, runtime_dir, &named, test_confs)?;
    if !files.iter().any(|file| file.role.component().is_some()) {
        return Ok(None);
    }
    merge_files_with(&files, None, Some(&runtime_dir.join(".env"))).map(Some)
}

/// What a conf file is to the merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfRole {
    /// `framework/conf/MoquiDefaultConf.xml`, merged first.
    Default,
    /// A component's `MoquiConf.xml`, by the component's name.
    Component(String),
    /// A component's `test/MoquiConf.xml`, merged under `moqui test` only.
    ComponentTest(String),
    /// The runtime conf, merged last ([`runtime_conf_file`]).
    Runtime,
}

impl ConfRole {
    /// The component of a component's conf.
    #[must_use]
    pub fn component(&self) -> Option<&str> {
        match self {
            Self::Component(name) | Self::ComponentTest(name) => Some(name),
            Self::Default | Self::Runtime => None,
        }
    }
}

/// One file of the merged conf.
#[derive(Clone, Debug)]
pub struct ConfFile {
    pub path: PathBuf,
    pub role: ConfRole,
}

impl ConfFile {
    /// The location its elements carry when parsed ([`MNode::parse_file`]).
    #[must_use]
    pub fn location(&self) -> String {
        self.path.display().to_string()
    }
}

/// The files the conf is merged from, in merge order, as boot merges them: the default conf, each component's
/// `MoquiConf.xml` (and with `test_confs` its `test/MoquiConf.xml`) in component order, then the runtime conf.
/// `components` are (name, directory) in component order.
///
/// # Errors
///
/// When the runtime conf `MOQUI_CONF` names is missing.
pub fn conf_files_of(
    framework_dir: &Path,
    runtime_dir: &Path,
    components: &[(String, PathBuf)],
    test_confs: bool,
) -> Result<Vec<ConfFile>, Error> {
    let mut files = vec![ConfFile {
        path: framework_dir.join("conf/MoquiDefaultConf.xml"),
        role: ConfRole::Default,
    }];
    for (name, dir) in components {
        let own = Some((dir.join("MoquiConf.xml"), ConfRole::Component(name.clone())));
        let test = test_confs.then(|| (dir.join("test/MoquiConf.xml"), ConfRole::ComponentTest(name.clone())));
        for (path, role) in own.into_iter().chain(test) {
            if moqui_lib::vfs::is_file(&path) {
                files.push(ConfFile { path, role });
            }
        }
    }
    if let Some(path) = runtime_conf(runtime_dir)? {
        files.push(ConfFile {
            path,
            role: ConfRole::Runtime,
        });
    }
    Ok(files)
}

/// Merge `files` in order ([`conf_files_of`]): the first is the base; then the environment's names are applied
/// ([`conf_env::apply_overrides`]). `replace` stands in a text for the file at its path, to see what an edit would make
/// before it is saved.
///
/// # Errors
///
/// When a file cannot be read or does not parse.
pub fn merge_files(files: &[ConfFile], replace: Option<(&Path, &str)>) -> Result<MNode, Error> {
    merge_files_with(files, replace, None).map(|(conf, _)| conf)
}

/// [`merge_files`], with the `.env` names read from `env_file` (the runtime's `runtime/.env`; `None` for the loaded
/// table), and what each environment name did ([`conf_env::apply_overrides`]).
///
/// # Errors
///
/// When a file cannot be read or does not parse.
pub fn merge_files_with(
    files: &[ConfFile],
    replace: Option<(&Path, &str)>,
    env_file: Option<&Path>,
) -> Result<(MNode, Vec<Override>), Error> {
    let parse = |file: &ConfFile| match replace {
        Some((path, text)) if path == file.path => MNode::parse_text(&file.location(), text),
        _ => MNode::parse_file(&file.path),
    };
    let Some((first, rest)) = files.split_first() else {
        return Err(Error::new("no conf files to merge"));
    };
    let base = parse(first)?;
    for file in rest {
        if file.role.component().is_some() {
            tracing::info!("Merging component conf {}", file.path.display());
        }
        merge_conf(&base, &parse(file)?);
    }
    let overrides = conf_env::apply_overrides(&base, env_file);
    Ok((base, overrides))
}

fn component_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Merge `over` into `base`. `component-list` children are appended.
pub fn merge_conf(base: &MNode, over: &MNode) {
    merge_config_component_nodes(base, over);
    merge_config_nodes(base, over);
    merge_component_elements(base, over);
}

/// A top-level element the conf schema does not know, a component's own settings (basic-comms's `comms`, mr-pass's
/// `pass`; plans/archive/Conf-Paths.md §6.27), merged as a settings element: its attributes replace the base's, and each child
/// with a `name` or `id` merges into the base's child of that element name and key (its attributes replace, its
/// children replace when it has any), any other child is added. A component reading another's setting gets nothing
/// when that component is absent.
fn merge_component_elements(base: &MNode, over: &MNode) {
    for over_child in over.children() {
        let name = over_child.name().to_string();
        // a default-property is kept whole, each one, so the boot's refusal names it (conf_env::refuse_retired)
        if name == "default-property" {
            base.append_node(over_child.deep_copy());
            continue;
        }
        if name == "component-list" || super::conf_schema::is_known_element(&name) {
            continue;
        }
        let Some(base_child) = base.first_named(&name) else {
            base.append_node(over_child.deep_copy());
            continue;
        };
        base_child.copy_attributes_from(&over_child);
        for grandchild in over_child.children() {
            let key = ["name", "id"].into_iter().find_map(|attr| {
                grandchild
                    .attribute(attr)
                    .filter(|value| !value.is_empty())
                    .map(|value| (attr, value))
            });
            let found = key
                .as_ref()
                .and_then(|(attr, value)| base_child.first_named_attrs(&grandchild.name(), &[(*attr, value.as_str())]));
            match found {
                Some(existing) => {
                    existing.copy_attributes_from(&grandchild);
                    if !grandchild.children().is_empty() {
                        existing.replace_children_from(&grandchild);
                    }
                }
                None => base_child.append_node(grandchild.deep_copy()),
            }
        }
    }
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
    base.merge_child_with_child_key(over, "tools", "tool-factory", keys(&["tools", "tool-factory"]), None);
    base.merge_child_with_child_key(over, "cache-list", "cache", keys(&["cache-list", "cache"]), None);
    base.merge_child_with_child_key(over, "telemetry", "artifact", keys(&["telemetry", "artifact"]), None);
    if let (Some(base_telemetry), Some(over_telemetry)) = (base.first_named("telemetry"), over.first_named("telemetry"))
    {
        base_telemetry.merge_single_child(&over_telemetry, "receiver");
        base_telemetry.merge_single_child(&over_telemetry, "browser");
        base_telemetry.merge_single_child(&over_telemetry, "regressions");
        base_telemetry.merge_single_child(&over_telemetry, "recovery");
        // A component adds or replaces operation rules by service pattern, keeping the framework's.
        base_telemetry.merge_child_with_child_key(
            &over_telemetry,
            "operations",
            "operation",
            keys(&["telemetry", "operations", "operation"]),
            None,
        );
    }
    merge_server_stats(base, over);
    merge_webapps(base, over);
    base.merge_child_with_child_key(
        over,
        "artifact-execution-facade",
        "artifact-execution",
        keys(&["artifact-execution-facade", "artifact-execution"]),
        None,
    );
    merge_user_facade(base, over);
    merge_transaction_facade(base, over);
    merge_resource_facade(base, over);
    merge_screen_facade(base, over);
    merge_service_facade(base, over);
    merge_search_facade(base, over);
    merge_llm_facade(base, over);
    merge_entity_facade(base, over);
    merge_database_list(base, over);
    merge_edge(base, over);
    // the mail client, with this deployment's OAuth client at each provider by its id
    base.merge_child_with_child_key(
        over,
        "email-client",
        "oauth-provider",
        keys(&["email-client", "oauth-provider"]),
        None,
    );
    // chat (docs/interfaces/Chat-Keys.md), its stickers merged as one element
    if let Some(over_chat) = over.first_named("chat") {
        match base.first_named("chat") {
            Some(base_chat) => {
                base_chat.copy_attributes_from(&over_chat);
                base_chat.merge_single_child(&over_chat, "stickers");
            }
            None => base.append_node(over_chat.deep_copy()),
        }
    }
    // push, its web and fcm children each merged as one element
    if let Some(over_push) = over.first_named("push") {
        match base.first_named("push") {
            Some(base_push) => {
                base_push.copy_attributes_from(&over_push);
                base_push.merge_single_child(&over_push, "web");
                base_push.merge_single_child(&over_push, "fcm");
            }
            None => base.append_node(over_push.deep_copy()),
        }
    }
    // the PASS authority, its clients by id
    base.merge_child_with_child_key(
        over,
        "pass-authority",
        "client",
        keys(&["pass-authority", "client"]),
        None,
    );
    // call media, with its TURN server merged as one element
    base.merge_child_with_child_key(over, "media-server", "turn-server", &[], None);
    // calls, their queue, recording and captions each merged as one element
    if let Some(over_calls) = over.first_named("calls") {
        match base.first_named("calls") {
            Some(base_calls) => {
                base_calls.copy_attributes_from(&over_calls);
                for child in ["queue", "recording", "captions"] {
                    base_calls.merge_single_child(&over_calls, child);
                }
            }
            None => base.append_node(over_calls.deep_copy()),
        }
    }
    // SIP, its limits and registrar each merged as one element
    if let Some(over_sip) = over.first_named("sip-server") {
        match base.first_named("sip-server") {
            Some(base_sip) => {
                base_sip.copy_attributes_from(&over_sip);
                base_sip.merge_single_child(&over_sip, "limits");
                base_sip.merge_single_child(&over_sip, "registrar");
            }
            None => base.append_node(over_sip.deep_copy()),
        }
    }
    // XMPP, with its upload settings merged as one element
    base.merge_child_with_child_key(over, "xmpp-server", "upload", &[], None);
    // texts and chat bridges, with the edge node's limits merged as one element
    base.merge_child_with_child_key(over, "channels", "edge-limits", &[], None);
    // mail delivery, with its one dkim element merged the same way (an attribute of the override replaces the base one)
    base.merge_child_with_child_key(over, "mail-delivery", "dkim", &[], None);
    // elements of settings only: an attribute of the override replaces the base one
    for name in [
        // the instance's purpose, id, node role, locale and time zone (plans/archive/Conf-Paths.md §6.1)
        "instance",
        // the sealed start (plans/Sealed-Secrets.md §3.3)
        "seal",
        // certificates by ACME (plans/Edge-Node2.md §4.8)
        "acme",
        // the offline clients' logs and uploads (plans/Mobile-Desktop.md MD5)
        "client-sync",
        // load runs from Tools (plans/archive/LoadRunner.md §10)
        "load-runner",
        // maps: the outside-request switch and the import and feature limits (plans/archive/Maps.md §4.2)
        "maps",
        // shared live location: the longest share and trail, the report interval (plans/archive/Location-Sharing.md §3.1)
        "location",
        // this server as a cluster member (plans/archive/Cluster.md)
        "cluster-member",
        // voicemail (docs/interfaces/Voicemail.md)
        "voicemail",
        // backups of the embedded database
        "database-backup",
        // the settings of basic-comms and demo, from their MoquiConf.xml
        "comms",
        "demo",
        "ssh-server",
        "terminal",
        "git-server",
        // the CI engine (docs/interfaces/Pipelines.md): an application names its run check and run URL services
        "git-ci",
        "dav-server",
        "smtp-server",
        "logging",
        "report",
        // CalcGrid's slow evaluation (plans/IronCalc-DB2.md CG1.4)
        "calc",
        "board",
        // the thresholds of the resource readings (plans/archive/Tools-Fancy.md B2)
        "resource-watch",
        // the dev tools (plans/archive/design-dev-aide.md 16): the file watcher, and the dev-aide component's settings
        "dev-reload",
        "dev-aide",
        // a client's settings (plans/Mobile-Desktop.md §6, MD4): read by the desktop and mobile apps, not the server
        "client-conf",
        // attested deploys and the gate (docs/tools/Component-Deploy.md): a runtime conf's allow entries replace the
        // default's (none)
        "component-deploy",
        // vaults (plans/archive/Vault-Instances.md §3)
        "vault",
        // the manager link (plans/archive/Vault-Instances.md §4.4)
        "manage",
    ] {
        base.merge_single_child(over, name);
    }
}

/// The edge link's conf (plans/archive/Edge-Node.md §5.2): the attributes, and each delegate, queue-kind and replicate by its
/// key; a replicate of the same set replaces its entities.
fn merge_edge(base: &MNode, over: &MNode) {
    let Some(over_edge) = over.first_named("edge") else {
        return;
    };
    let Some(base_edge) = base.first_named("edge") else {
        base.append_node(over_edge.deep_copy());
        return;
    };
    base_edge.copy_attributes_from(&over_edge);
    // the pairings, each one element (plans/archive/Conf-Paths.md §6.19)
    base_edge.merge_single_child(&over_edge, "node");
    base_edge.merge_single_child(&over_edge, "front-node");
    base_edge.merge_children_by_keys(&over_edge, "delegate", keys(&["edge", "delegate"]), None);
    base_edge.merge_children_by_keys(&over_edge, "queue-kind", keys(&["edge", "queue-kind"]), None);
    base_edge.merge_children_by_keys(&over_edge, "replicate", keys(&["edge", "replicate"]), None);
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
    base.merge_child_with_child_key(
        over,
        "webapp-list",
        "webapp",
        keys(&["webapp-list", "webapp"]),
        Some(&mut merge_webapp),
    );
}

fn merge_webapp_children(base: &MNode, over: &MNode) {
    base.merge_children_by_keys(over, "root-screen", keys(&[WEBAPP_LIST, WEBAPP, "root-screen"]), None);
    base.merge_children_by_keys(over, "error-screen", keys(&[WEBAPP_LIST, WEBAPP, "error-screen"]), None);
    // anonymous token routes (screen/token_route.rs): a component's route of the same name replaces the earlier one
    base.merge_children_by_keys(over, "token-route", keys(&[WEBAPP_LIST, WEBAPP, "token-route"]), None);
    // token paths (SEC-PSF-03): a component adds its own; the same pattern twice is one
    base.merge_children_by_keys(over, "token-path", keys(&[WEBAPP_LIST, WEBAPP, "token-path"]), None);
    base.merge_children_by_keys(over, "public-site", keys(&[WEBAPP_LIST, WEBAPP, "public-site"]), None);
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
    base.merge_children_by_keys(
        over,
        "filter",
        keys(&[WEBAPP_LIST, WEBAPP, "filter"]),
        Some(&mut merge_filter),
    );
    base.merge_children_by_keys(over, "listener", keys(&[WEBAPP_LIST, WEBAPP, "listener"]), None);
    let mut merge_servlet = |base_child: &MNode, over_child: &MNode| {
        merge_init_params(base_child, over_child);
        append_named(base_child, over_child, "url-pattern");
    };
    base.merge_children_by_keys(
        over,
        "servlet",
        keys(&[WEBAPP_LIST, WEBAPP, "servlet"]),
        Some(&mut merge_servlet),
    );
    base.merge_single_child(over, "session-config");
    base.merge_children_by_keys(over, "endpoint", keys(&[WEBAPP_LIST, WEBAPP, "endpoint"]), None);
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
    base_node.merge_children_by_keys(
        &over_node,
        "rate-limit",
        keys(&[WEBAPP_LIST, WEBAPP, PROTECTION, "rate-limit"]),
        None,
    );
    base_node.merge_children_by_keys(&over_node, "ban", keys(&[WEBAPP_LIST, WEBAPP, PROTECTION, "ban"]), None);
    for rule in ["client-rule", "path-rule", "user-agent-rule"] {
        base_node.merge_children_by_keys(&over_node, rule, keys(&[WEBAPP_LIST, WEBAPP, PROTECTION, rule]), None);
        for child in over_node.children_named(rule) {
            if child.attribute("name").is_none_or(|name| name.is_empty()) {
                base_node.append_node(child.deep_copy());
            }
        }
    }
}

fn merge_init_params(base: &MNode, over: &MNode) {
    base.merge_children_by_keys(
        over,
        "init-param",
        keys(&[WEBAPP_LIST, WEBAPP, "servlet", "init-param"]),
        None,
    );
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
    merge_authz(&base_node, &over_node);
}

/// `authz`'s attributes replace the base's, as other settings elements do, but its `sealed-permission` and
/// `privileged-group` children are added to the base's by `name`, never replacing them: each component adds its own
/// to the lists and keeps the framework's.
fn merge_authz(base: &MNode, over: &MNode) {
    let Some(over_authz) = over.first_named("authz") else {
        return;
    };
    let Some(base_authz) = base.first_named("authz") else {
        base.append_node(over_authz.deep_copy());
        return;
    };
    base_authz.copy_attributes_from(&over_authz);
    for child in ["sealed-permission", "privileged-group"] {
        base_authz.merge_children_by_keys(&over_authz, child, keys(&["user-facade", "authz", child]), None);
    }
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
    base.merge_child_with_child_key(
        over,
        RESOURCE,
        "resource-reference",
        keys(&[RESOURCE, "resource-reference"]),
        None,
    );
    base.merge_child_with_child_key(
        over,
        RESOURCE,
        "template-renderer",
        keys(&[RESOURCE, "template-renderer"]),
        None,
    );
    base.merge_child_with_child_key(
        over,
        RESOURCE,
        "script-runner",
        keys(&[RESOURCE, "script-runner"]),
        None,
    );
    // the share link limits (SEC-BOS-83) and the rich-text image limits (SEC-CLI-92), one element each
    if let (Some(base_resource), Some(over_resource)) = (base.first_named(RESOURCE), over.first_named(RESOURCE)) {
        base_resource.merge_single_child(&over_resource, "drive-share");
        base_resource.merge_single_child(&over_resource, "rich-text-images");
    }
}

fn merge_screen_facade(base: &MNode, over: &MNode) {
    base.merge_child_with_child_key(
        over,
        SCREEN,
        "screen-text-output",
        keys(&[SCREEN, "screen-text-output"]),
        None,
    );
    base.merge_child_with_child_key(over, SCREEN, "screen-output", keys(&[SCREEN, "screen-output"]), None);
    let mut merge_screen = |base_screen: &MNode, over_screen: &MNode| {
        base_screen.merge_children_by_keys(
            over_screen,
            "subscreens-item",
            keys(&[SCREEN, "screen", "subscreens-item"]),
            None,
        );
    };
    base.merge_child_with_child_key(
        over,
        SCREEN,
        "screen",
        keys(&[SCREEN, "screen"]),
        Some(&mut merge_screen),
    );
    if let (Some(base_node), Some(over_node)) = (base.first_named("screen-facade"), over.first_named("screen-facade")) {
        base_node.merge_single_child(&over_node, "pdf");
        // a component's providers of the conf-answerable hooks, merged by hook and name
        for over_providers in over_node.children_named("shell-providers") {
            let base_providers = base_node.first_named("shell-providers").unwrap_or_else(|| {
                let created = MNode::new("shell-providers");
                base_node.append_node(created.clone());
                created
            });
            for hook in crate::screen::shell::CONF_PROVIDER_HOOKS {
                base_providers.merge_children_by_keys(&over_providers, hook, &["name"], None);
            }
            for child in over_providers.children() {
                let name = child.name();
                if !crate::screen::shell::CONF_PROVIDER_HOOKS.contains(&name.as_str()) {
                    tracing::error!("screen-facade shell-providers: {name} is not a hook a conf may answer; left out");
                }
            }
        }
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
    base_node.merge_node_with_child_key(
        &over_node,
        "service-location",
        keys(&[SERVICE, "service-location"]),
        None,
    );
    base_node.merge_children_by_keys(&over_node, "service-type", keys(&[SERVICE, "service-type"]), None);
    base_node.merge_children_by_keys(&over_node, "service-file", keys(&[SERVICE, "service-file"]), None);
    base_node.merge_children_by_keys(&over_node, "startup-service", keys(&[SERVICE, "startup-service"]), None);
    if let Some(over_pool) = over_node.first_named("thread-pool") {
        if let Some(base_pool) = base_node.first_named("thread-pool") {
            base_pool.merge_node_with_child_key(
                &over_pool,
                "run-from-pool",
                keys(&[SERVICE, "thread-pool", "run-from-pool"]),
                None,
            );
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
        base_endpoint.merge_children_by_keys(over_endpoint, "header", keys(&[LLM, "endpoint", "header"]), None);
    };
    base_node.merge_children_by_keys(
        &over_node,
        "endpoint",
        keys(&[LLM, "endpoint"]),
        Some(&mut merge_endpoint),
    );
    let mut merge_model = |base_model: &MNode, over_model: &MNode| {
        base_model.merge_children_by_keys(over_model, "reasoning", keys(&[LLM, "model", "reasoning"]), None);
        if let Some(api) = over_model.first_named("api-config") {
            base_model.remove_where(|child| child.has_name("api-config"));
            base_model.append_node(api.deep_copy());
        }
    };
    base_node.merge_children_by_keys(&over_node, "model", keys(&[LLM, "model"]), Some(&mut merge_model));
    base_node.merge_children_by_keys(&over_node, "bind", keys(&[LLM, "bind"]), None);
    base_node.merge_children_by_keys(&over_node, "data-entity", keys(&[LLM, "data-entity"]), None);
    base_node.merge_children_by_keys(&over_node, "user-facts", keys(&[LLM, "user-facts"]), None);
    base_node.merge_children_by_keys(
        &over_node,
        "constitution-provider",
        keys(&[LLM, "constitution-provider"]),
        None,
    );
    base_node.merge_children_by_keys(&over_node, "ai-task", keys(&[LLM, "ai-task"]), None);
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
    base_node.merge_node_with_child_key(
        &over_node,
        "datasource",
        keys(&["entity-facade", "datasource"]),
        Some(&mut merge_datasource),
    );
    base_node.merge_single_child(&over_node, "server-jndi");
    append_named(&base_node, &over_node, "load-entity");
    append_named(&base_node, &over_node, "load-data");
}

fn merge_database_list(base: &MNode, over: &MNode) {
    base.merge_child_with_child_key(
        over,
        DATABASES,
        "dictionary-type",
        keys(&[DATABASES, "dictionary-type"]),
        None,
    );
    let mut merge_database = |base_db: &MNode, over_db: &MNode| {
        base_db.merge_node_with_child_key(
            over_db,
            "database-type",
            keys(&[DATABASES, "database", "database-type"]),
            None,
        );
    };
    base.merge_child_with_child_key(
        over,
        DATABASES,
        "database",
        keys(&[DATABASES, "database"]),
        Some(&mut merge_database),
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
