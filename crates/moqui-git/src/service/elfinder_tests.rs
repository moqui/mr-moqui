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

//! The elFinder connector (`org.moqui.impl.ElFinderServices.run#Command`) on a `git://` volume: each write a commit.
//! The connector's other tests are moqui-core's `service/elfinder/tests.rs`.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::object::Value;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::service::ServicePools;
use moqui_core::web::multipart::UploadedFile;

struct Setup {
    facade: EntityFacade,
    context: ExecutionContext,
}

/// The connector on this test's thread is not production, whatever another test's boot left in
/// the process's `instance_purpose`.
fn setup() -> Setup {
    moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(Some(false)));
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::idle(), moqui_core::context::framework_dir());
    context.factory().set_entity(facade.clone());
    Setup { facade, context }
}

fn install() {
    let services = moqui_core::context::framework_dir().join("service/org/moqui/impl/ElFinderServices.xml");
    let failures = moqui_core::service::lint(&[services]).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
}

/// The hash Java makes of a path under the `db_` volume.
fn hash(path: &str) -> String {
    use base64::Engine as _;
    format!("db_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path))
}

/// Run one command. `other` holds the request parameters as `ec.web.parameters` would.
fn command(root: &str, cmd: &str, target: &str, other: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
    let other: IndexMap<String, Value> = other.into_iter().map(|(key, value)| (key.to_string(), value)).collect();
    let mut parameters = BTreeMap::new();
    parameters.insert("cmd".to_string(), Value::string(cmd));
    parameters.insert("target".to_string(), Value::string(target));
    parameters.insert("resourceRoot".to_string(), Value::string(root));
    parameters.insert("otherParameters".to_string(), Value::Map(other));
    moqui_core::service::sync("org.moqui.impl.ElFinderServices.run#Command", &parameters).unwrap()
}

fn response(out: &BTreeMap<String, Value>) -> serde_json::Value {
    let value = out.get("responseMap").cloned().unwrap_or(Value::Null);
    serde_json::from_str(&moqui_core::entity::query_rest::encode_json(&value)).unwrap()
}

fn upload(name: &str, bytes: &[u8]) -> Value {
    UploadedFile::in_memory("upload[]", name, "text/plain", bytes.to_vec()).into_value()
}

/// A `git://` volume: the commands go through the resource facade, so each write is a commit; a new directory holds a
/// `.gitkeep`, as git keeps no empty directories.
#[test]
fn commands_run_against_a_git_volume() {
    let setup = setup();
    let _entered = setup.context.enter();
    let _authz = setup.context.disable_authz();
    install();
    crate::install(setup.context.factory()).unwrap();
    let mut repository = setup.facade.make_value("moqui.resource.GitRepository").unwrap();
    repository.set("repositoryId", Value::string("ef"));
    repository.set("repositoryKind", Value::string("db"));
    repository.create(&setup.facade).unwrap();
    let root = "git://ef";

    let opened = response(&command(root, "open", "", vec![("init", Value::string("1"))]));
    assert_eq!(opened["cwd"]["mime"], "directory", "{opened}");
    let made = response(&command(
        root,
        "mkdir",
        "db_cm9vdA",
        vec![("name", Value::string("docs"))],
    ));
    assert_eq!(made["added"][0]["mime"], "directory", "{made}");
    let docs = hash("docs");
    let uploaded = response(&command(
        root,
        "upload",
        &docs,
        vec![("_fileUploadList", Value::List(vec![upload("a.txt", b"hello")]))],
    ));
    assert_eq!(uploaded["added"][0]["size"], 5, "{uploaded}");
    let listed = response(&command(root, "ls", &docs, vec![]));
    assert_eq!(listed["list"], serde_json::json!([".gitkeep", "a.txt"]), "{listed}");
    let file = hash("docs/a.txt");
    response(&command(
        root,
        "put",
        &file,
        vec![("content", Value::string("hello again"))],
    ));
    let got = response(&command(root, "get", &file, vec![]));
    assert_eq!(got["content"], "hello again", "{got}");
    let resources = setup.context.factory().resource();
    assert_eq!(resources.version_history("git://ef/docs/a.txt").unwrap().len(), 2);
    let renamed = response(&command(root, "rename", &file, vec![("name", Value::string("b.txt"))]));
    assert_eq!(renamed["added"][0]["hash"], hash("docs/b.txt"), "{renamed}");
    let removed = response(&command(
        root,
        "rm",
        "",
        vec![("targets[]", Value::List(vec![Value::string(&docs)]))],
    ));
    assert!(
        removed["removed"]
            .as_array()
            .is_some_and(|list| list.contains(&serde_json::json!(docs))),
        "{removed}"
    );
    assert!(resources.list("git://ef/").unwrap().is_empty());
}
