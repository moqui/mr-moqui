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

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use moqui_lib::env;
use moqui_lib::node::MNode;

use super::{Authz, ExecutionContextFactory, Tarpit};

pub(super) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Clears what a boot test leaves in the process: the version properties and the `.env` table.
pub(super) struct EnvGuard;

impl EnvGuard {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        env::clear_property("moqui_version");
        env::clear_property("moqui.version");
    }
}

pub(super) struct TempRuntime(PathBuf);

impl TempRuntime {
    pub(super) fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moqui-phase2-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(path.join("conf")).unwrap();
        Self(path)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn repo_env_stamp(root: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(root.join("runtime/.env")).ok()?;
    Some((meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

#[test]
fn component_conf_mounts_and_runtime_conf_wins() {
    let root = workspace_root();
    let runtime = TempRuntime::new("component-conf");
    let component = runtime.path().join("components/addon/app");
    std::fs::create_dir_all(&component).unwrap();
    std::fs::write(
        component.join("MoquiConf.xml"),
        r#"<moqui-conf><instance purpose="component"/>
                <screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
                    <subscreens-item name="app" location="component://app/screen/App.xml"/></screen></screen-facade></moqui-conf>"#,
    )
    .unwrap();
    std::fs::write(
        runtime.path().join("conf/MoquiDevConf.xml"),
        r#"<moqui-conf><instance purpose="dev"/></moqui-conf>"#,
    )
    .unwrap();
    let none = super::merge_component_confs(
        &root.join("runtime/components/base/framework"),
        runtime.path(),
        &[root.join("runtime/components/base/framework")],
        true,
    );
    assert!(none.unwrap().is_none());
    // test/MoquiConf.xml merges after the component's own, in a moqui test run only
    std::fs::create_dir_all(component.join("test")).unwrap();
    std::fs::write(
        component.join("test/MoquiConf.xml"),
        r#"<moqui-conf><app test-overlay="test"/></moqui-conf>"#,
    )
    .unwrap();
    // the component's own element, which the test conf sets
    let overlay = |conf: &MNode| conf.first_named("app").and_then(|node| node.attribute("test-overlay"));
    let plain = super::merge_component_confs(
        &root.join("runtime/components/base/framework"),
        runtime.path(),
        std::slice::from_ref(&component),
        false,
    )
    .unwrap()
    .expect("a component conf");
    assert_eq!(overlay(&plain), None);
    let conf = super::merge_component_confs(
        &root.join("runtime/components/base/framework"),
        runtime.path(),
        &[component],
        true,
    )
    .unwrap()
    .expect("a component conf");
    assert_eq!(overlay(&conf).as_deref(), Some("test"));
    let item = conf
        .first_named("screen-facade")
        .unwrap()
        .first_named_attrs("screen", &[("location", "component://webroot/screen/webroot/apps.xml")])
        .unwrap()
        .first_named_attrs("subscreens-item", &[("name", "app")]);
    assert!(item.is_some());
    let purpose = conf.first_named("instance").unwrap();
    assert_eq!(purpose.attribute("purpose").as_deref(), Some("dev"));
}

#[test]
fn runtime_conf_selected_by_moqui_conf() {
    use std::ffi::OsStr;
    let temp = TempRuntime::new("conf-select");
    let runtime = temp.path();
    assert_eq!(super::conf::runtime_conf_from(runtime, None).unwrap(), None);
    std::fs::write(runtime.join("conf/MoquiDevConf.xml"), "<moqui-conf/>").unwrap();
    std::fs::write(runtime.join("conf/MoquiProductionConf.xml"), "<moqui-conf/>").unwrap();
    let dev = Some(runtime.join("conf/MoquiDevConf.xml"));
    assert_eq!(super::conf::runtime_conf_from(runtime, None).unwrap(), dev);
    assert_eq!(
        super::conf::runtime_conf_from(runtime, Some(OsStr::new(""))).unwrap(),
        dev
    );
    let production = runtime.join("conf/MoquiProductionConf.xml");
    let selected = super::conf::runtime_conf_from(runtime, Some(OsStr::new("conf/MoquiProductionConf.xml"))).unwrap();
    assert_eq!(selected, Some(production.clone()));
    let absolute = super::conf::runtime_conf_from(runtime, Some(production.as_os_str())).unwrap();
    assert_eq!(absolute, Some(production));
    let missing = super::conf::runtime_conf_from(runtime, Some(OsStr::new("conf/Nope.xml"))).unwrap_err();
    assert!(missing.message().contains("conf/Nope.xml"), "{}", missing.message());
}

#[test]
fn instance_conf_in_the_runtime_wins_over_the_dev_conf() {
    use std::ffi::OsStr;
    let temp = TempRuntime::new("conf-instance");
    let runtime = temp.path().to_path_buf();
    std::fs::write(runtime.join("conf/MoquiDevConf.xml"), "<moqui-conf/>").unwrap();
    std::fs::write(runtime.join("conf/MoquiProductionConf.xml"), "<moqui-conf/>").unwrap();
    let instance = runtime.join("MoquiConf.xml");
    std::fs::write(&instance, "<moqui-conf/>").unwrap();
    assert_eq!(
        super::conf::runtime_conf_from(&runtime, None).unwrap(),
        Some(instance.clone())
    );
    assert_eq!(
        super::conf::runtime_conf_from(&runtime, Some(OsStr::new(""))).unwrap(),
        Some(instance)
    );
    let selected = super::conf::runtime_conf_from(&runtime, Some(OsStr::new("conf/MoquiProductionConf.xml"))).unwrap();
    assert_eq!(selected, Some(runtime.join("conf/MoquiProductionConf.xml")));
}

#[test]
fn runtime_conf_property_wins_over_the_environment() {
    let _lock = crate::user::session::cookie_property_lock();
    let temp = TempRuntime::new("conf-property");
    let runtime = temp.path();
    std::fs::write(runtime.join("conf/MoquiProductionConf.xml"), "<moqui-conf/>").unwrap();
    moqui_lib::env::set_property("MOQUI_CONF", "conf/MoquiProductionConf.xml");
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            moqui_lib::env::clear_property("MOQUI_CONF");
        }
    }
    let _clear = Clear;
    let selected = super::conf::runtime_conf(runtime).unwrap();
    assert_eq!(selected, Some(runtime.join("conf/MoquiProductionConf.xml")));
}

#[test]
fn dev_conf_merge_sets_purpose_and_entity_definition_idle() {
    let root = workspace_root();
    let base = MNode::parse_file(root.join("runtime/components/base/framework/conf/MoquiDefaultConf.xml")).unwrap();
    let over = MNode::parse_file(root.join("runtime/conf/MoquiDevConf.xml")).unwrap();
    super::merge_conf(&base, &over);
    let instance = base.first_named("instance").unwrap();
    assert_eq!(instance.attribute("purpose").as_deref(), Some("dev"));
    assert_eq!(instance.attribute("default-locale").as_deref(), Some("en_US"));
    let entity = base
        .first_named("cache-list")
        .unwrap()
        .first_named_attrs("cache", &[("name", "entity.definition")])
        .unwrap();
    assert_eq!(entity.attribute("expire-time-idle").as_deref(), Some("30"));
    assert!(entity.has_attribute("value-type"));
}

#[test]
fn boot_registers_components_and_named_resource_errors() {
    let _lock = crate::user::session::cookie_property_lock();
    let root = workspace_root();
    let _env = EnvGuard::new();
    let before = repo_env_stamp(&root);
    let temp = TempRuntime::new("boot");
    std::fs::copy(
        root.join("runtime/conf/MoquiDevConf.xml"),
        temp.path().join("conf/MoquiDevConf.xml"),
    )
    .unwrap();
    crate::context::layout::link_base_set(&root.join("runtime/components/base"), temp.path()).unwrap();
    // A component whose MoquiConf.xml selects a tool factory this binary does not link, as a dev component's is in a
    // Lite binary: the boot leaves the tool factory out and goes on.
    let unlinked = temp.path().join("components/addon/unlinked-tool");
    std::fs::create_dir_all(&unlinked).unwrap();
    std::fs::write(
        unlinked.join("component.xml"),
        r#"<component name="unlinked-tool" version="${moqui_version}"/>"#,
    )
    .unwrap();
    std::fs::write(
        unlinked.join("MoquiConf.xml"),
        r#"<moqui-conf><tools><tool-factory class="unlinked::ToolFactory"/></tools></moqui-conf>"#,
    )
    .unwrap();
    env::set_property("moqui_version", "phase2-test");
    let framework = root.join("runtime/components/base/framework");
    let factory = ExecutionContextFactory::boot_with_framework(temp.path(), &framework).unwrap();
    let components = factory.components();
    let names: Vec<&str> = components.iter().map(|info| info.name.as_str()).collect();
    // agent-code and basic-comms depend on webroot (basic-comms' shell values go on webroot's Quasar theme), so they
    // come after it
    assert_eq!(
        names,
        vec!["framework", "webroot", "agent-code", "basic-comms", "tools", "unlinked-tool"]
    );
    for info in components.iter() {
        assert_eq!(info.version, "phase2-test");
    }
    let tools = factory.resource().get("component://tools/screen/Tools.xml").unwrap();
    assert!(tools.exists());
    let screen = factory.resource().get("component://tools/screen").unwrap();
    assert!(!screen.children().unwrap().is_empty());
    assert!(screen.find_child_file("Tools.xml").exists());
    assert!(factory
        .resource()
        .get("runtime://conf/MoquiDevConf.xml")
        .unwrap()
        .exists());
    let caches = factory.cache().names().unwrap();
    assert!(caches.iter().any(|name| name == "entity.definition"));
    assert!(caches.iter().any(|name| name == "service.location"));
    let content = factory.resource().get("content://main/x").unwrap_err();
    assert!(content.to_string().contains("content://main/x"));
    // An http location resolves without a connection; it is fetched when read.
    factory.resource().get("https://example.com/").expect("https resolves");
    let database = factory.resource().get("dbresource://x").unwrap_err();
    assert!(database.to_string().contains("not available"));
    let unknown = factory.resource().get("ftp://files.example/a").unwrap_err();
    assert!(unknown.to_string().contains("ftp"));
    assert_eq!(repo_env_stamp(&root), before);
}

/// S4: a boot writes no secret into `runtime/.env` (the seal's store keeps them, plans/Sealed-Secrets.md S4) and leaves
/// the host settings there as they were written, the last line with no newline too; a `.env` that is a directory still
/// fails the boot, naming it.
#[test]
fn a_boot_writes_no_secret_to_env_and_an_unreadable_env_fails() {
    let _lock = crate::user::session::cookie_property_lock();
    let root = workspace_root();
    let _env = EnvGuard::new();
    let before = repo_env_stamp(&root);
    let temp = TempRuntime::new("secret");
    // Host settings written before the first start (docs/Run-Deploy.md), the last line with no newline
    std::fs::write(temp.path().join(".env"), "phase2_probe_host=erp.example.com").unwrap();
    let framework = root.join("runtime/components/base/framework");
    let _factory = ExecutionContextFactory::boot_with_framework(temp.path(), &framework).unwrap();
    let _again = ExecutionContextFactory::boot_with_framework(temp.path(), &framework).unwrap();
    let env_text = std::fs::read_to_string(temp.path().join(".env")).unwrap();
    assert_eq!(env_text, "phase2_probe_host=erp.example.com", "no secret is added");
    assert!(!env_text.contains(PROBE_SECRET));
    assert_eq!(env::get_prop_or_env("phase2_probe_host"), "erp.example.com");
    env::clear_dotenv();

    let blocked = TempRuntime::new("blocked");
    std::fs::create_dir(blocked.path().join(".env")).unwrap();
    let err = match ExecutionContextFactory::boot_with_framework(blocked.path(), &framework) {
        Ok(_) => panic!("boot succeeded with an unreadable runtime environment file"),
        Err(err) => err,
    };
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains("MoquiDefaultPassword"));
    assert!(!shown.contains("CHANGEME"));
    assert!(shown.contains("directory"));
    assert_eq!(repo_env_stamp(&root), before);
}

/// A generated secret is 32 random bytes in standard base64, never the same twice. A process property holds a secret
/// when it is a secret conf attribute's environment name, or by `pass`, `pw` or `key` in its name.
#[test]
fn generated_secrets_are_32_random_bytes_and_secret_names_are_known() {
    use base64::Engine as _;
    let first = crate::seal::secrets::random_secret().unwrap();
    let second = crate::seal::secrets::random_secret().unwrap();
    assert_ne!(first, second);
    for value in [&first, &second] {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.as_bytes())
            .unwrap();
        assert_eq!(bytes.len(), 32);
    }
    let secret = super::factory::property_is_secret;
    assert!(secret("remote_db_pass"));
    assert!(secret(
        "moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key"
    ));
    assert!(secret("MOQUI_CONF__ENTITY_FACADE__CRYPT_PASS"));
    assert!(
        secret("moqui_conf__push__fcm__service_account"),
        "secret by the schema, not its name"
    );
    assert!(secret("smtp_pw"));
    assert!(!secret("moqui_conf__webapp_list__webapp__WEBROOT__http_port"));

    // the request log's rule: a PASS payment result's returnKey reads the attempt's outcome (SEC-PSA-09)
    let parameter = super::secret_parameter;
    assert!(parameter("returnKey"));
    assert!(parameter("downloadToken"));
    assert!(!parameter("primaryKeyId"));
    assert!(!parameter("returnKeyId"));
}

/// SEC-WEB-13: a session cookie key (`webapp-list/webapp[webroot]/session-config@cookie-key`) an operator set is
/// checked before the server starts: still `CHANGEME`, not base64, or under 32 bytes stops the boot with a message
/// naming the setting and not the value; 32 bytes start.
#[test]
fn a_weak_session_cookie_key_stops_the_boot() {
    use base64::Engine as _;
    const KEY: &str = "moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key";
    /// The key is cleared however the test ends: one left at a weak value stops every later boot in this process.
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            env::clear_property(KEY);
        }
    }
    let _lock = crate::user::session::cookie_property_lock();
    let _clear = Clear;
    let root = workspace_root();
    let _env = EnvGuard::new();
    let framework = root.join("runtime/components/base/framework");
    let temp = TempRuntime::new("cookiekey");
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    for weak in ["CHANGEME".to_string(), "not base64!".to_string(), encode(&[7_u8; 16])] {
        env::set_property(KEY, weak.clone());
        let err = match ExecutionContextFactory::boot_with_framework(temp.path(), &framework) {
            Ok(_) => panic!("boot started with the session cookie key {weak:?}"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("session-config cookie-key"), "{err}");
        assert!(!err.contains(&weak) || weak == "CHANGEME", "{err}");
    }
    env::set_property(KEY, encode(&[7_u8; 32]));
    ExecutionContextFactory::boot_with_framework(temp.path(), &framework).unwrap();
}

/// The `.env` line a generated secret had before S4: the default conf leaves `chat@store-key-secret` at `CHANGEME`.
const PROBE_SECRET: &str = "moqui_conf__chat__store_key_secret=";

#[test]
fn artifact_stack_allows_and_authz_depth_follows_the_guard() {
    let _lock = crate::user::session::cookie_property_lock();
    let root = workspace_root();
    let _env = EnvGuard::new();
    let _example = crate::testing::ExampleOff::new();
    let before = repo_env_stamp(&root);
    let temp = TempRuntime::new("artifact");
    std::fs::copy(
        root.join("runtime/conf/MoquiDevConf.xml"),
        temp.path().join("conf/MoquiDevConf.xml"),
    )
    .unwrap();
    crate::context::layout::link_base_set(&root.join("runtime/components/base"), temp.path()).unwrap();
    let factory =
        ExecutionContextFactory::boot_with_framework(temp.path(), root.join("runtime/components/base/framework"))
            .unwrap();
    let context = factory.new_context();
    assert!(!context.authz_disabled());
    context
        .push_checked("AT_XML_SCREEN", "tools", "view", Authz::Required, Tarpit::Skip, None)
        .unwrap();
    {
        let guard = context.disable_authz();
        assert!(context.authz_disabled());
        context.push("AT_SERVICE", "nested", "run");
        context.pop().unwrap();
        drop(guard);
    }
    assert!(!context.authz_disabled());
    context.pop().unwrap();
    assert!(context.pop().is_err());

    context.message().add_message("hello");
    context.message().add_error("bad");
    assert_eq!(context.message().messages(), &["hello".to_string()]);
    assert_eq!(context.message().errors(), &["bad".to_string()]);
    let other = factory.new_context();
    assert!(other.message().messages().is_empty());
    context.message().clear();
    assert!(context.message().messages().is_empty());
    assert!(context.message().errors().is_empty());
    assert_eq!(repo_env_stamp(&root), before);
}
