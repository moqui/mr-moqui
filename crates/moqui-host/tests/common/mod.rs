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

//! The seeded server of Gate 3 and the plain HTTP client its tests use, shared by `gate3.rs` and `gate3_browser.rs`.
//!
//! The fixture is `component://tools/screen/System/Security/ArtifactGroup/ArtifactGroupDetail.xml`. The tools screens are
//! not mounted under any app by the conf in this repository, so the setup adds the mount in a `conf/MoquiDevConf.xml` of its
//! temporary runtime, which is where a deployment would.

// Each test binary that includes this module uses part of it.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moqui_host::{load_data, start_server, LoadFlags};

pub mod cdp;

/// The fixture screen under the plain `html` root.
pub const FIXTURE: &str = "/System/Security/ArtifactGroup/ArtifactGroupDetail";

/// The conf of the test runtime. `companion` is the location of a screen with qjs and qvue text of its own, `widgets` and `static_list`
/// the screens of the browser smoke.
fn conf(companion: &str, widgets: &str, static_list: &str, extra: &str) -> String {
    format!(
        r#"<moqui-conf>
    {extra}
    <screen-facade>
        <screen location="component://webroot/screen/webroot.xml">
            <subscreens-item name="System" menu-title="System" location="component://tools/screen/System.xml"/>
            <subscreens-item name="Tools" menu-title="Tools" location="component://tools/screen/Tools.xml"/>
        </screen>
        <screen location="component://webroot/screen/webroot/apps.xml">
            <subscreens-item name="System" menu-title="System" location="component://tools/screen/System.xml"/>
            <subscreens-item name="GateCompanion" menu-title="Companion" menu-include="false" location="{companion}"/>
            <subscreens-item name="GateWidgets" menu-title="Widgets" menu-include="false" location="{widgets}"/>
            <subscreens-item name="GateStaticList" menu-title="Static List" menu-include="false" location="{static_list}"/>
        </screen>
    </screen-facade>
</moqui-conf>"#
    )
}

/// A screen with the three client forms: its widgets are the qvt template, and it has a qjs object and a qvue component file.
const COMPANION: &str = r#"<screen require-authentication="false">
    <widgets>
        <label text="Companion {{literal}} text" type="p"/>
        <render-mode>
            <text type="qjs"><![CDATA[define([], function () { return { data: function () { return { count: 1 }; } }; });]]></text>
            <text type="qvue"><![CDATA[<template><p>{{count}}</p></template>
<script>export default { data() { return { count: 2 }; } }</script>]]></text>
        </render-mode>
    </widgets>
</screen>"#;

/// The client features the browser smoke drives that the fixture does not have: click to edit, a background submit with a date, a
/// drop-down, a multiple drop-down (Tom Select in html) and a message, a form-list with the column picker, sort and pages, and Chart.js and Mermaid in the qvt text. Each transition
/// answers JSON with what it received, so the smoke reads what the server got from the response.
const WIDGETS: &str = r#"<screen require-authentication="false">
    <transition name="saveValue"><actions><script>ec.web.sendJsonResponse({saved: value || ''})</script></actions>
        <default-response type="none"/></transition>
    <transition name="saveNote"><actions><script>ec.web.sendJsonResponse({note: note || '', when: when || '', kind: kind || '', tags: tags || ''})</script></actions>
        <default-response type="none"/></transition>
    <transition name="checkCode"><actions><script>if (!/\d/.test(code || '')) ec.message.addValidationError(null, 'code', null, 'Code needs a digit', null)</script></actions>
        <default-response url="."/></transition>
    <transition name="formSelectColumns"><actions><script>ec.web.sendJsonResponse({columnsTree: columnsTree || ''})</script></actions>
        <default-response type="none"/></transition>
    <actions>
        <entity-find entity-name="moqui.basic.Enumeration" list="enumList">
            <search-form-inputs default-order-by="enumId"/></entity-find>
    </actions>
    <widgets>
        <label text="Gate widgets" type="h4"/>
        <container-box id="GateClosedBox" initial="closed"><box-header title="Gate closed box"/>
            <box-body><label text="Shown once opened" type="p"/></box-body></container-box>
        <editable id="GateEditable" text="Edit me" transition="saveValue" widget-type="text"/>
        <form-single name="NoteForm" transition="saveNote" background-submit="true" background-message="Note sent">
            <field name="note"><default-field title="Note"><text-line/></default-field></field>
            <field name="when"><default-field title="When"><date-time type="date"/></default-field></field>
            <field name="kind"><default-field title="Kind"><drop-down>
                <option key="A" text="Alpha"/><option key="B" text="Beta"/></drop-down></default-field></field>
            <field name="tags"><default-field title="Tags"><drop-down allow-multiple="true">
                <option key="X" text="Ex"/><option key="Y" text="Why"/></drop-down></default-field></field>
            <field name="submitButton"><default-field title="Send"><submit/></default-field></field>
        </form-single>
        <form-single name="CodeForm" transition="checkCode" background-submit="true" background-message="Code saved">
            <field name="code"><default-field title="Code"><text-line/></default-field></field>
            <field name="checkButton"><default-field title="Check"><submit/></default-field></field>
        </form-single>
        <form-list name="EnumList" list="enumList" select-columns="true">
            <field name="enumId"><header-field title="Enum ID" show-order-by="true"/><default-field><display/></default-field></field>
            <field name="description"><header-field show-order-by="true"/><default-field><display/></default-field></field>
            <field name="enumTypeId"><default-field><display/></default-field></field>
        </form-list>
        <render-mode><text type="qvt"><![CDATA[<div id="GateCharts"><m-chart :config="{type: 'bar', data: {labels: ['a', 'b'], datasets: [{label: 'n', data: [1, 2]}]}}" height="200px"></m-chart>
<m-mermaid height="200px">graph TD; GateA-->GateB</m-mermaid></div>]]></text></render-mode>
    </widgets>
</screen>"#;

/// A server-static screen with a long list: the static form-list's pages and sort run in the browser against `/actions`.
const STATIC_LIST: &str = r#"<screen require-authentication="false" server-static="qvt">
    <transition name="pickEnums"><actions><script>ec.web.sendJsonResponse({picked: enumId_0})</script></actions><default-response type="none"/></transition>
    <actions>
        <entity-find entity-name="moqui.basic.Enumeration" list="enumList">
            <search-form-inputs default-order-by="enumId"/></entity-find>
    </actions>
    <widgets>
        <label text="Enumerations of ${ec.user.username}" type="h4"/>
        <form-list name="EnumList" list="enumList" header-dialog="true" saved-finds="true" show-csv-button="true" select-columns="true" views="list,board">
            <view-board group-field="enumTypeId" card-fields="description"/>
            <row-selection id-field="enumId"><action><dialog button-text="Pick"/>
                <form-single name="PickForm" transition="pickEnums"><field name="go"><default-field title="Pick Them"><submit/></default-field></field></form-single>
            </action></row-selection>
            <field name="enumId"><header-field title="Enum ID" show-order-by="true"/><default-field><display/></default-field></field>
            <field name="description"><header-field show-order-by="true"><text-find/></header-field><default-field><display/></default-field></field>
        </form-list>
    </widgets>
</screen>"#;

/// The component of a deployment that marks the fixture screen server-static for qvt without editing the screen: a `screen-extend` file.
const STATIC_EXTEND: &str = r#"<screen-extend server-static="qvt"/>"#;

/// The tests set process-wide properties while they start a server, so those take the write lock, and one runs at a time. A test
/// that only boots a factory to load data, and sets no property, takes the read lock with the others like it: [`shared`].
static SERIAL: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// The lock of the tests that start a server or set process properties, alone. Hold it for the whole test.
pub fn serial() -> std::sync::RwLockWriteGuard<'static, ()> {
    SERIAL.write().unwrap_or_else(|err| err.into_inner())
}

/// The lock of a test that loads data or runs a cluster without a server or a process property. It runs beside the others of its
/// kind, and not beside a test that holds [`serial`]. Hold it for the whole test.
pub fn shared() -> std::sync::RwLockReadGuard<'static, ()> {
    SERIAL.read().unwrap_or_else(|err| err.into_inner())
}

/// The seeded server the gate tests share the setup of. Fields drop in order: the server stops before its directories go.
pub struct Gate {
    pub server: moqui_host::Server,
    /// The runtime directory the server reads, removed when the gate drops.
    pub runtime: PathBuf,
    /// The embedded cluster directory: `data` is the socket directory, `pgpass` the password.
    pub cluster_dir: PathBuf,
    _properties: PropertyReset,
    /// `None` for a second gate, which runs under the first one's lock ([`start_second`]).
    _serial: Option<std::sync::RwLockWriteGuard<'static, ()>>,
    _runtime: DirGuard,
    _cluster: DirGuard,
}

pub fn start(label: &str) -> Gate {
    start_with(label, false)
}

/// [`start`] with one more `entity-facade-xml` document loaded after the grants, for the rows a test needs.
pub fn start_with_data(label: &str, data: &str) -> Gate {
    start_seeded(label, false, Some((data, None)), "", &[], &[], &[])
}

/// Load the seed (the framework files and each component's `data` directory), the demo users, and the grants of the fixture, then serve on an
/// ephemeral port. With `static_fixture` a `screen-extend` marks the fixture server-static for qvt.
pub fn start_with(label: &str, static_fixture: bool) -> Gate {
    start_seeded(label, static_fixture, None, "", &[], &[], &[])
}

/// The token of the token-route gate's public link, and of a revoked one ([`start_token_route`]).
#[allow(dead_code)]
pub const LINK_TOKEN: &str = "GateLinkToken0123456789abcdefXY";
#[allow(dead_code)]
pub const REVOKED_TOKEN: &str = "GateRevokedToken0123456789abcde";

/// A gate with the token route `tk` (`component://example/screen/PublicLink.xml`, checked by
/// `FileServices.check#PublicShareToken`) and a drive with a `docs` folder shared by john.doe through the link
/// [`LINK_TOKEN`] (note "For the gate") and the revoked link [`REVOKED_TOKEN`].
#[allow(dead_code)]
pub fn start_token_route(label: &str) -> Gate {
    let hash = |token: &str| moqui_core::user::single_use_hash(token).unwrap();
    let data = format!(
        r#"<entity-facade-xml type="seed">
    <moqui.resource.ResourceDrive driveId="TKD" driveName="Token drive" driveKindEnumId="RdkShared" rootLocation="dbresource://tokendrive"
            ownerUserId="EX_JOHN_DOE"/>
    <moqui.resource.DbResource resourceId="TKR1" filename="tokendrive" isFile="N"/>
    <moqui.resource.DbResource resourceId="TKR2" parentResourceId="TKR1" filename="docs" isFile="N"/>
    <moqui.resource.ResourceShare shareId="TKS1" driveId="TKD" path="" isDirectory="Y" isLink="Y" linkTokenHash="{}"
            permissionEnumId="RspView" note="For the gate" createdByUserId="EX_JOHN_DOE" openedCount="0"/>
    <moqui.resource.ResourceShare shareId="TKS2" driveId="TKD" path="" isDirectory="Y" isLink="Y" linkTokenHash="{}"
            permissionEnumId="RspView" note="Revoked" createdByUserId="EX_JOHN_DOE" revokedDate="2026-01-01 00:00:00"/>
</entity-facade-xml>"#,
        hash(LINK_TOKEN),
        hash(REVOKED_TOKEN)
    );
    let conf = r#"<webapp-list><webapp name="webroot"><token-route name="tk" screen-location="component://example/screen/PublicLink.xml"
        token-service="org.moqui.impl.FileServices.check#PublicShareToken" title="Shared with you"/>
        <token-route name="tkp" screen-location="component://example/screen/PublicLink.xml"
        token-service="org.moqui.impl.FileServices.check#PublicShareToken" post-transition="files"/></webapp></webapp-list>"#;
    start_seeded(label, false, Some((&data, None)), conf, &[], &[], &[])
}

/// [`start`] with more conf in the runtime's `MoquiDevConf.xml`, such as a response header of this server only, and process properties
/// that hold while the gate does. They are set once the gate has the one-server lock, and cleared when it drops.
pub fn start_with_conf(label: &str, extra_conf: &str, properties: &[(&str, &str)]) -> Gate {
    let mut gate = start_seeded(label, false, None, extra_conf, &[], &[], &[]);
    for (name, value) in properties {
        moqui_lib::env::set_property(name, (*value).to_string());
    }
    gate._properties = PropertyReset(properties.iter().map(|(name, _)| (*name).to_string()).collect());
    gate
}

/// [`start_with_data`] with process properties the server reads while it starts, such as the cluster transport. They are set once the
/// gate has the one-server lock, before the load, and cleared when the gate drops. Setting them before the lock would change
/// the properties of a server another test still runs.
pub fn start_with_properties(label: &str, data: Option<&str>, properties: &[(&str, &str)]) -> Gate {
    start_seeded(label, false, data.map(|text| (text, None)), "", properties, &[], &[])
}

/// [`start_with_properties`] with more conf in the runtime's `MoquiDevConf.xml` (the children of `moqui-conf`), such
/// as `<ssh-server sftp-root="dbresource://sftp"/>`.
pub fn start_with_conf_properties(
    label: &str,
    data: Option<&str>,
    extra_conf: &str,
    properties: &[(&str, &str)],
) -> Gate {
    start_seeded(label, false, data.map(|text| (text, None)), extra_conf, properties, &[], &[])
}

/// [`start_with_files`] with more conf in the runtime's `MoquiDevConf.xml` (the children of `moqui-conf`).
pub fn start_with_conf_files(
    label: &str,
    data: Option<&str>,
    extra_conf: &str,
    properties: &[(&str, &str)],
    files: &[(&str, &str)],
) -> Gate {
    start_seeded(label, false, data.map(|text| (text, None)), extra_conf, properties, files, &[])
}

/// [`start_with_properties`] with files written into the runtime directory before the server starts, by path relative
/// to it, such as a component of the test's own (`component/<name>/component.xml` and its `service` files).
pub fn start_with_files(label: &str, data: Option<&str>, properties: &[(&str, &str)], files: &[(&str, &str)]) -> Gate {
    start_seeded(label, false, data.map(|text| (text, None)), "", properties, files, &[])
}

/// [`start_with_conf`] with symlinks created in the runtime directory before the server starts.
///
/// Each pair is an absolute source path and a path relative to the runtime, such as a component directory the seed
/// template does not carry. The link is created after the runtime copy and before the server boots. `data_types`
/// selects which documents in `data` load; `None` is the seed set.
pub fn start_with_links(
    label: &str,
    extra_conf: &str,
    data: Option<&str>,
    data_types: Option<&[&str]>,
    links: &[(&str, &str)],
) -> Gate {
    start_seeded(
        label,
        false,
        data.map(|text| (text, data_types)),
        extra_conf,
        &[],
        &[],
        links,
    )
}

/// Clears the properties a gate set, before the gate gives up the lock.
#[derive(Default)]
pub struct PropertyReset(Vec<String>);

impl PropertyReset {
    /// Clears these properties when it drops.
    pub fn of(names: &[&str]) -> Self {
        Self(names.iter().map(|name| (*name).to_string()).collect())
    }
}

impl Drop for PropertyReset {
    fn drop(&mut self) {
        for name in &self.0 {
            moqui_lib::env::set_property(name, String::new());
        }
    }
}

/// The seeded cluster every gate starts from, built once and kept under `target/<profile>/moqui-test-template`.
///
/// Loading the seed (the framework files and each component's data), the demo users and the grants is most of the time a gate
/// takes to start. Its result does not depend on the test, so it is loaded once into a template, and each gate copies the
/// stopped cluster. The template is keyed by the inputs of the load, so an edit to the data, the framework or the sources of the
/// crates builds a new one, and the old ones are removed.
struct Template {
    /// The fixture screens. The grants name them by location, so a gate's conf must point at these files.
    companion: PathBuf,
    widgets: PathBuf,
    static_list: PathBuf,
    /// The stopped cluster directory after the loads: `data`, `pgpass` and the libxml2 shim.
    cluster: PathBuf,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The repo's own runtime directory, where `./moq chromium-download` puts `chromium`.
pub fn repo_runtime() -> PathBuf {
    let runtime = repo_root().join("runtime");
    std::fs::canonicalize(&runtime).unwrap_or(runtime)
}

/// The Chromium that `./moq chromium-download` put in the repo's `runtime/chromium`.
#[cfg(feature = "pdf")]
pub fn downloaded_chromium() -> Option<PathBuf> {
    moqui_host::pdf::downloaded_chromium(&repo_runtime())
}

/// Without the `pdf` feature the host does not look for a downloaded Chromium.
#[cfg(not(feature = "pdf"))]
pub fn downloaded_chromium() -> Option<PathBuf> {
    None
}

/// The files under `path` as a hash of names, sizes and modification times, for the key of the template.
fn stamp_tree(path: &Path, hasher: &mut std::collections::hash_map::DefaultHasher) {
    use std::hash::Hash;
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    path.file_name().hash(hasher);
    if meta.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|dir| dir.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for entry in entries {
            if entry.file_name().is_some_and(|name| name == "target" || name == ".git") {
                continue;
            }
            stamp_tree(&entry, hasher);
        }
    } else {
        meta.len().hash(hasher);
        meta.modified().ok().hash(hasher);
    }
}

fn template_key() -> String {
    use std::hash::{Hash, Hasher};
    let root = repo_root();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (COMPANION, WIDGETS, STATIC_LIST).hash(&mut hasher);
    for part in ["runtime/base-component", "Cargo.lock"] {
        stamp_tree(&root.join(part), &mut hasher);
    }
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .map(|dir| dir.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    crates.sort();
    for krate in crates {
        stamp_tree(&krate.join("src"), &mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// `cp -a` of the contents of `from` into the existing directory `to`, as a reflink where the file system has them
/// (GNU `--reflink=auto`; macOS's `cp` names a clone `-c`, which falls back to a copy off APFS).
fn copy_dir(from: &Path, to: &Path) {
    let reflink = if cfg!(target_os = "macos") { "-c" } else { "--reflink=auto" };
    let status = std::process::Command::new("cp")
        .arg("-a")
        .arg(reflink)
        .arg(format!("{}/.", from.display()))
        .arg(to)
        .status()
        .expect("cp");
    assert!(status.success(), "cp {} {}", from.display(), to.display());
}

fn template() -> &'static Template {
    static TEMPLATE: std::sync::OnceLock<Template> = std::sync::OnceLock::new();
    TEMPLATE.get_or_init(build_template)
}

fn build_template() -> Template {
    let exe = std::env::current_exe().unwrap();
    // target/<profile>/deps/<test binary>
    let profile_dir = exe.ancestors().nth(2).expect("target directory").to_path_buf();
    let cache = profile_dir.join("moqui-test-template");
    let key = template_key();
    let fixtures = cache.join(format!("fixtures-{key}"));
    let done = cache.join(&key);
    std::fs::create_dir_all(&fixtures).unwrap();
    let template = Template {
        companion: fixtures.join("GateCompanion.xml"),
        widgets: fixtures.join("GateWidgets.xml"),
        static_list: fixtures.join("GateStaticList.xml"),
        cluster: done.join("cluster"),
    };
    std::fs::write(&template.companion, COMPANION).unwrap();
    std::fs::write(&template.widgets, WIDGETS).unwrap();
    std::fs::write(&template.static_list, STATIC_LIST).unwrap();
    if !done.is_dir() {
        let building = cache.join(format!("{key}.building-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&building);
        std::fs::create_dir_all(&building).unwrap();
        // The cluster loads in a short path (a unix socket path has a length limit) and is copied in when it is done.
        let cluster_dir = temp_dir("moqui-template-pg");
        let cluster_guard = DirGuard(cluster_dir.clone());
        std::fs::create_dir_all(&cluster_dir).unwrap();
        let runtime = building.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        seed(&runtime, &cluster_dir, &template, "");
        std::fs::remove_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(building.join("cluster")).unwrap();
        copy_dir(&cluster_dir, &building.join("cluster"));
        drop(cluster_guard);
        if std::fs::rename(&building, &done).is_err() {
            // Another process built the same template first.
            let _ = std::fs::remove_dir_all(&building);
        }
        // Templates and fixtures of other keys are stale.
        for entry in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.contains(&key) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    template
}

/// Write the conf and the links of a runtime directory. The Postgres install is shared ([`temp_dir`]), so the gate does
/// not unpack Postgres again.
fn prepare_runtime(runtime: &Path, fixtures: &Template, extra_conf: &str) {
    std::fs::create_dir_all(runtime.join("conf")).unwrap();
    std::fs::write(
        runtime.join("conf/MoquiDevConf.xml"),
        conf(
            &fixtures.companion.display().to_string(),
            &fixtures.widgets.display().to_string(),
            &fixtures.static_list.display().to_string(),
            extra_conf,
        ),
    )
    .unwrap();
    let base =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component")).unwrap();
    std::os::unix::fs::symlink(&base, runtime.join("base-component")).unwrap();
}

/// Load the seed (the framework files and each component's `data` directory), the demo users, and the grants of the fixture.
fn seed(runtime: &Path, cluster_dir: &Path, fixtures: &Template, extra_conf: &str) {
    prepare_runtime(runtime, fixtures, extra_conf);
    let framework = repo_root().join("runtime/base-component/framework");
    let base = std::fs::canonicalize(repo_root().join("runtime/base-component")).unwrap();
    // The seed load reads the data directory of each component too: the default themes, the pagination text, the access to the
    // applications list (webroot) and the System and Tools app groups (tools).
    let seeded = load_data(runtime, &framework, cluster_dir, &LoadFlags::default()).unwrap();
    assert!(seeded.enumeration > 0, "enumeration {}", seeded.enumeration);
    let demo = base.join("webroot/data/WebrootSecurityDemoData.xml");
    let demo_flags = LoadFlags {
        location: Some(demo.display().to_string()),
        data_types: Some(vec!["demo".to_string()]),
        ..LoadFlags::default()
    };
    load_data(runtime, &framework, cluster_dir, &demo_flags).unwrap();
    // The artifact group the screen shows (SYSTEM_APP, from the tools component's own security data) gets a second
    // authorization and a tarpit to list, and its member says N for the pattern flag the tools data leaves empty.
    // The screens of the browser smoke read entities, so ADMIN gets them as an artifact group that passes its authorization on.
    let gate_members: String = [&fixtures.widgets, &fixtures.static_list]
        .iter()
        .map(|location| {
            format!(
                r#"<moqui.security.ArtifactGroupMember artifactGroupId="GATE_SCREENS" artifactName="{}"
                artifactTypeEnumId="AT_XML_SCREEN" nameIsPattern="N" inheritAuthz="Y"/>"#,
                location.display()
            )
        })
        .collect();
    let grant = runtime.join("system-app.xml");
    std::fs::write(
        &grant,
        r#"<entity-facade-xml type="seed">
            <moqui.security.ArtifactGroup artifactGroupId="GATE_SCREENS" description="Gate 3 browser screens"/>
            GATE_MEMBERS
            <moqui.security.ArtifactAuthz artifactAuthzId="GATE_SCREENS_ADMIN" userGroupId="ADMIN"
                artifactGroupId="GATE_SCREENS" authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
            <moqui.security.ArtifactGroupMember artifactGroupId="SYSTEM_APP"
                artifactName="component://tools/screen/System.xml" artifactTypeEnumId="AT_XML_SCREEN" nameIsPattern="N"/>
            <moqui.security.ArtifactAuthz artifactAuthzId="SYSTEM_APP_ALL" userGroupId="ALL_USERS"
                artifactGroupId="SYSTEM_APP" authzTypeEnumId="AUTHZT_DENY" authzActionEnumId="AUTHZA_UPDATE"/>
            <moqui.security.ArtifactTarpit artifactTarpitId="SYSTEM_APP_TARPIT" userGroupId="ALL_USERS"
                artifactGroupId="SYSTEM_APP" maxHitsCount="100" maxHitsDuration="60" tarpitDuration="30"/>
            </entity-facade-xml>"#
            .replace("GATE_MEMBERS", &gate_members),
    )
    .unwrap();
    let grant_flags = LoadFlags {
        location: Some(grant.display().to_string()),
        ..LoadFlags::default()
    };
    load_data(runtime, &framework, cluster_dir, &grant_flags).unwrap();
}

/// A second seeded server beside the gate a test holds, on its own runtime and database: a remote for a client of the
/// first (the A2A and MCP clients over loopback). It runs under the first gate's lock, so it takes none, and sets no
/// process property; drop it before the first.
#[allow(dead_code)]
pub fn start_second(label: &str, data: Option<&str>, extra_conf: &str) -> Gate {
    start_seeded_under(None, label, false, data.map(|text| (text, None)), extra_conf, &[], &[], &[])
}

fn start_seeded(
    label: &str,
    static_fixture: bool,
    data: Option<(&str, Option<&[&str]>)>,
    extra_conf: &str,
    properties: &[(&str, &str)],
    files: &[(&str, &str)],
    links: &[(&str, &str)],
) -> Gate {
    start_seeded_under(Some(serial()), label, static_fixture, data, extra_conf, properties, files, links)
}

#[allow(clippy::too_many_arguments)]
fn start_seeded_under(
    serial: Option<std::sync::RwLockWriteGuard<'static, ()>>,
    label: &str,
    static_fixture: bool,
    data: Option<(&str, Option<&[&str]>)>,
    extra_conf: &str,
    properties: &[(&str, &str)],
    files: &[(&str, &str)],
    links: &[(&str, &str)],
) -> Gate {
    let reset = PropertyReset::of(&properties.iter().map(|(name, _)| *name).collect::<Vec<_>>());
    for (name, value) in properties {
        moqui_lib::env::set_property(name, (*value).to_string());
    }
    let template = template();
    let runtime = temp_dir(&format!("moqui-gate3-rt-{label}"));
    let cluster_dir = temp_dir(&format!("moqui-gate3-pg-{label}"));
    let runtime_guard = DirGuard(runtime.clone());
    let cluster_guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&cluster_dir).unwrap();
    prepare_runtime(&runtime, template, extra_conf);
    copy_dir(&template.cluster, &cluster_dir);
    if static_fixture {
        let component = runtime.join("component/gate-static");
        let extend = component.join("screen-extend/tools/screen/System/Security/ArtifactGroup");
        std::fs::create_dir_all(&extend).unwrap();
        std::fs::write(component.join("component.xml"), r#"<component name="gate-static"/>"#).unwrap();
        std::fs::write(extend.join("ArtifactGroupDetail.xml"), STATIC_EXTEND).unwrap();
    }
    for (path, text) in files {
        let path = runtime.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    for (from, to) in links {
        let dest = runtime.join(to);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(from, &dest).unwrap();
    }
    let framework = repo_root().join("runtime/base-component/framework");
    if let Some((data, data_types)) = data {
        let extra = runtime.join("test-data.xml");
        std::fs::write(&extra, data).unwrap();
        let extra_flags = LoadFlags {
            location: Some(extra.display().to_string()),
            data_types: data_types.map(|types| types.iter().map(|kind| (*kind).to_string()).collect()),
            ..LoadFlags::default()
        };
        load_data(&runtime, &framework, &cluster_dir, &extra_flags).unwrap();
    }

    // 0 binds an ephemeral cluster port, so a test run beside another server does not need 8444 free.
    moqui_lib::env::set_property("cluster_https_port", "0");
    let bind = SocketAddr::from(([127, 0, 0, 1], 0));
    let server = start_server(&runtime, &framework, &cluster_dir, bind).unwrap();
    Gate {
        server,
        runtime: runtime.clone(),
        cluster_dir: cluster_dir.clone(),
        _properties: reset,
        _serial: serial,
        _runtime: runtime_guard,
        _cluster: cluster_guard,
    }
}

pub fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// The cookies a response sets as a `Cookie` header value.
pub fn cookies_of(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| value.split(';').next().map(str::to_string))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn basic(username: &str, password: &str) -> String {
    format!("Basic {}", base64(&format!("{username}:{password}").into_bytes()))
}

/// Sign in through the login form, as a browser does. Returns the session cookie.
pub fn sign_in(address: SocketAddr, username: &str, password: &str) -> String {
    let (_, headers, _) = exchange(address, &http_request("GET", "/Login", &[], ""));
    let cookie = cookies_of(&headers);
    let body = format!("username={username}&password={password}");
    let post = http_request(
        "POST",
        "/Login/login",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &body,
    );
    let (status, headers, text) = exchange(address, &post);
    assert_eq!(status, 302, "{text}");
    merge_cookies(&cookie, &headers)
}

/// `cookie` with the cookies a response sets replacing those of the same name.
pub fn merge_cookies(cookie: &str, headers: &[(String, String)]) -> String {
    let mut jar: Vec<(String, String)> = cookie
        .split("; ")
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    for pair in cookies_of(headers).split("; ") {
        if let Some((name, value)) = pair.split_once('=') {
            match jar.iter_mut().find(|(known, _)| known == name) {
                Some(entry) => entry.1 = value.to_string(),
                None => jar.push((name.to_string(), value.to_string())),
            }
        }
    }
    jar.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The value of `<meta name="moquiSessionToken" content="...">` in a page.
pub fn session_token(html: &str) -> Option<String> {
    let marker = "<meta name=\"moquiSessionToken\" content=\"";
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}

pub fn http_request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (name, value) in headers {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    if !body.is_empty() || method.eq_ignore_ascii_case("POST") {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

pub fn exchange(address: SocketAddr, request: &str) -> (u16, Vec<(String, String)>, String) {
    let (status, headers, body) = exchange_bytes(address, request);
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// [`exchange`] with the body as bytes, for a binary response such as an xlsx render.
#[allow(dead_code)]
pub fn exchange_bytes(address: SocketAddr, request: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 8192];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(count) => buf.extend_from_slice(&tmp[..count]),
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock => {
                if response_complete(&buf) {
                    break;
                }
                panic!("timed out after {} bytes: {}", buf.len(), String::from_utf8_lossy(&buf));
            }
            Err(err) => panic!("read failed after {} bytes: {err}", buf.len()),
        }
        if response_complete(&buf) {
            break;
        }
    }
    parse_response(&buf)
}

fn response_complete(buf: &[u8]) -> bool {
    let Some(split) = find_header_end(buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..split]);
    let Some(length) = content_length(&head) else {
        return false;
    };
    buf.len() >= split + 4 + length
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &str) -> Option<usize> {
    for line in head.split("\r\n").skip(1) {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn parse_response(buf: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let split = find_header_end(buf);
    let head = String::from_utf8_lossy(split.map_or(buf, |at| &buf[..at]));
    let body = split.map_or(Vec::new(), |at| buf[at + 4..].to_vec());
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    (status, headers, body)
}

fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8) | (bytes[index + 2] as u32);
        push_base64(&mut out, chunk, 4);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        push_base64(&mut out, (bytes[index] as u32) << 16, 2);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
        push_base64(&mut out, chunk, 3);
        out.push('=');
    }
    out
}

fn push_base64(out: &mut String, chunk: u32, count: usize) {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let shifts = [18, 12, 6, 0];
    for shift in shifts.into_iter().take(count) {
        let piece = ((chunk >> shift) & 63) as usize;
        out.push(ALPHABET[piece] as char);
    }
}

/// A path under `/tmp` for a runtime or cluster directory. The runtimes of a test run share one Postgres install,
/// unpacked once in `target/<profile>/moqui-test-install`, rather than each unpacking its own.
pub fn temp_dir(prefix: &str) -> PathBuf {
    share_install();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

/// Points `MOQUI_INSTALL` at the install the test runs share.
pub fn share_install() {
    static SHARED: std::sync::Once = std::sync::Once::new();
    SHARED.call_once(|| {
        let exe = std::env::current_exe().unwrap();
        // target/<profile>/deps/<test binary>
        let profile_dir = exe.ancestors().nth(2).expect("target directory");
        let install = profile_dir.join("moqui-test-install");
        moqui_lib::env::set_property("MOQUI_INSTALL", install.display().to_string());
    });
}

pub struct DirGuard(pub PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
