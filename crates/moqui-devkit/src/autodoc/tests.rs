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

//! The docs of the fixture project in `testdata/project` (a shop component and a site component that extends it, with
//! each kind of artifact), on a model-only boot with this repository's framework: every file the generator writes for
//! them, in `testdata/expected` (`UPDATE_GOLDEN=1` writes them again; read the diff), and the writes into a directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::write::{plan_writes, write_plan, Manifest, MANIFEST};
use super::*;
use crate::index::MemoryIndex;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/autodoc/testdata")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// An MR Moqui runtime in a fresh directory: this repository's framework (linked) and the fixture components.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-autodoc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let framework = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/base-component/framework"
    ))
    .unwrap();
    std::fs::create_dir_all(root.join("runtime/base-component")).unwrap();
    std::os::unix::fs::symlink(framework, root.join("runtime/base-component/framework")).unwrap();
    copy_dir(&testdata().join("project"), &root.join("runtime/component"));
    std::fs::canonicalize(root).unwrap()
}

const COMPONENTS: [&str; 2] = ["autodoc-shop", "autodoc-site"];

/// Boot the project, read its index and run `body` in an entered context, its services loaded.
fn with_project<T>(name: &str, body: impl FnOnce(&Arc<ExecutionContextFactory>, &MemoryIndex) -> T) -> T {
    let root = project(name);
    let found = crate::project::detect(&root).unwrap();
    let dirs = found.model_dirs(|| None).unwrap();
    let factory = crate::project::boot(&dirs).unwrap();
    let out = {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        moqui_core::service::load_definitions().unwrap();
        let index = MemoryIndex::build(&factory);
        body(&factory, &index)
    };
    let _ = std::fs::remove_dir_all(&root);
    out
}

fn options() -> Options {
    Options {
        components: COMPONENTS.iter().map(|name| (*name).to_string()).collect(),
        include_source: true,
        ..Options::default()
    }
}

/// The text with the boot's directory as `<root>`, so a golden is the same on any machine.
fn portable(text: &str, factory: &ExecutionContextFactory) -> String {
    let runtime = std::fs::canonicalize(factory.runtime_dir()).unwrap();
    let root = runtime.parent().unwrap().display().to_string();
    text.replace(&root, "<root>")
}

#[test]
fn the_fixture_project_documents_every_kind() {
    with_project("tree", |factory, index| {
        let source = MemorySource::new(index);
        let plan = build_plan(factory, &source).unwrap();
        let generated = generate(factory, &source, &plan, &options());
        assert!(generated.failures.is_empty(), "{:?}", generated.failures);
        let expected = testdata().join("expected");
        let update = std::env::var_os("UPDATE_GOLDEN").is_some();
        if update {
            let _ = std::fs::remove_dir_all(&expected);
        }
        let mut paths: Vec<&str> = generated.files.iter().map(|file| file.path.as_str()).collect();
        paths.sort_unstable();
        for file in &generated.files {
            let path = expected.join(&file.path);
            let text = portable(&file.text, factory);
            if update {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &text).unwrap();
            } else {
                let golden = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no golden {}", file.path));
                assert_eq!(
                    text, golden,
                    "{} differs from its golden; UPDATE_GOLDEN=1 writes it",
                    file.path
                );
            }
        }
        let listing = paths.join("\n") + "\n";
        let listing_path = testdata().join("expected-files.txt");
        if update {
            std::fs::write(&listing_path, &listing).unwrap();
        } else {
            assert_eq!(
                listing,
                std::fs::read_to_string(&listing_path).unwrap(),
                "the files written"
            );
        }
        // every diagram of the tree, for the webroot's page test to parse with the vendored Mermaid
        let mut diagrams: Vec<serde_json::Value> = Vec::new();
        for file in &generated.files {
            for (at, block) in file.text.split("```mermaid\n").skip(1).enumerate() {
                let text = block.split("\n```").next().unwrap_or_default();
                diagrams.push(serde_json::json!({"doc": format!("{}#{}", file.path, at + 1), "text": text}));
            }
        }
        diagrams.sort_by(|a, b| a["doc"].as_str().cmp(&b["doc"].as_str()));
        let diagrams_text = serde_json::to_string_pretty(&diagrams).unwrap() + "\n";
        let diagrams_path = testdata().join("mermaid/tree.json");
        if update {
            std::fs::write(&diagrams_path, &diagrams_text).unwrap();
        } else {
            assert_eq!(
                diagrams_text,
                std::fs::read_to_string(&diagrams_path).unwrap(),
                "the diagrams of the tree"
            );
        }
        assert!(diagrams.len() > 10, "{}", diagrams.len());
        // what each kind's doc has, beside the goldens
        let text = |path: &str| {
            generated
                .files
                .iter()
                .find(|file| file.path == path)
                .map(|file| file.text.clone())
                .unwrap_or_else(|| panic!("no {path} in {paths:?}"))
        };
        let order = text("entity/shop/order/ShopOrder.md");
        assert!(order.contains("One per checkout"), "the full description");
        assert!(
            order.contains("The order's id, \\| a pipe"),
            "a pipe in a cell is escaped"
        );
        assert!(
            order.contains("| autodoc-site |"),
            "a field an extend-entity adds names its component"
        );
        assert!(order.contains("```mermaid\nflowchart LR"));
        assert!(order.contains("[ShopOrderItem](ShopOrderItem.md)"));
        let package = text("entity/shop/order/README.md");
        assert!(package.contains("```mermaid\nerDiagram"));
        let service = text("service/shop/OrderServices/create-Order.md");
        assert!(
            service.contains("`items.item.quantity`"),
            "nested parameters by their dotted path"
        );
        assert!(service.contains("[`shop.OrderServices.define#OrderInfo`](define-OrderInfo.md)"));
        assert!(service.contains("```xml\n"), "the definition, asked for");
        assert!(
            text("screen/Shop.md").contains("The shop app: its orders."),
            "the comment heads the screen's doc"
        );
        assert!(
            paths.contains(&"screen/Shop/Orders.autodoc-shop.md")
                && paths.contains(&"screen/Shop/Orders.autodoc-site.md")
        );
        assert!(text("eca/service/shop/OrderServices.secas.md").contains("Read the new order."));
        assert!(text("rest/shop.md").contains("`/shop/orders/{orderId}`"));
        assert!(
            !paths.iter().any(|path| path.starts_with("entity/moqui")),
            "only the components asked for"
        );
    });
}

#[test]
fn a_doc_is_found_by_its_path_its_artifact_or_a_part() {
    with_project("find", |factory, index| {
        let source = MemorySource::new(index);
        let plan = build_plan(factory, &source).unwrap();
        let options = Options::default();
        let by_part = render_doc(factory, &source, &plan, &options, "field:shop.order.ShopOrder.statusId").unwrap();
        assert_eq!(by_part.path, "entity/shop/order/ShopOrder.md");
        assert_eq!(by_part.artifact_id, "entity:shop.order.ShopOrder");
        let by_path = render_doc(factory, &source, &plan, &options, "/screen/Shop.md").unwrap();
        assert_eq!(by_path.page, "artifact");
        assert!(by_path.text.starts_with("# Shop\n"), "{}", by_path.text);
        // the viewer links every doc of the plan: the framework's StatusItem too
        assert!(
            by_part.text.contains("](../../moqui/basic/StatusItem.md)"),
            "{}",
            by_part.text
        );
        assert!(render_doc(factory, &source, &plan, &options, "entity:no.Such").is_err());
    });
}

#[test]
fn options_select_by_kind_component_and_package() {
    let doc = |kind: DocKind, component: &str, package: &str| DocEntry {
        path: String::new(),
        page: Page::Artifact(kind, String::new()),
        kind,
        title: String::new(),
        name: String::new(),
        component: component.into(),
        package: package.into(),
        summary: String::new(),
        file: String::new(),
        line: 0,
        id: String::new(),
    };
    let mut options = Options::default();
    assert!(selected(&options, &doc(DocKind::Entity, "c", "mantle.order")));
    options.packages = vec!["mantle".into()];
    assert!(selected(&options, &doc(DocKind::Entity, "c", "mantle.order")));
    assert!(
        selected(&options, &doc(DocKind::Service, "c", "mantle/order")),
        "a dot or a slash separates segments"
    );
    assert!(
        !selected(&options, &doc(DocKind::Entity, "c", "mantleX.order")),
        "by whole segments"
    );
    assert!(
        selected(&options, &doc(DocKind::Eca, "c", "service/mantle/order")),
        "an ECA file by its path under service/"
    );
    options.kinds = vec![DocKind::Service];
    assert!(!selected(&options, &doc(DocKind::Entity, "c", "mantle.order")));
    options.kinds.clear();
    options.components = vec!["other".into()];
    assert!(!selected(&options, &doc(DocKind::Entity, "c", "mantle.order")));
}

#[test]
fn writes_change_only_what_changed_and_remove_only_stale_generated_files() {
    with_project("write", |factory, index| {
        let source = MemorySource::new(index);
        let plan = build_plan(factory, &source).unwrap();
        let dir = std::env::temp_dir().join(format!("moqui-autodoc-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut options = options();
        options.kinds = vec![DocKind::Entity];
        let generated = generate(factory, &source, &plan, &options);
        let first = plan_writes(&dir, &generated, &Manifest::read(&dir), &plan, true);
        assert_eq!((first.write.len(), first.unchanged), (generated.files.len(), 0));
        let report = write_plan(&dir, &first);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(dir.join("entity/shop/order/ShopOrder.md").is_file());

        // a hand-written file and a stale generated one (in the manifest, no doc in the plan any more)
        std::fs::write(dir.join("entity/NOTES.md"), "mine").unwrap();
        let mut manifest = Manifest::read(&dir);
        manifest
            .files
            .insert("entity/shop/order/Gone.md".into(), Default::default());
        std::fs::write(dir.join("entity/shop/order/Gone.md"), "old").unwrap();
        std::fs::write(dir.join(MANIFEST), manifest.to_text()).unwrap();

        // a second export of the services adds to the tree; the entity docs stay as they are
        options.kinds = vec![DocKind::Service];
        options.previous = Manifest::read(&dir).paths();
        let services = generate(factory, &source, &plan, &options);
        let second = plan_writes(&dir, &services, &Manifest::read(&dir), &plan, true);
        assert_eq!(second.remove, ["entity/shop/order/Gone.md"]);
        let report = write_plan(&dir, &second);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(!dir.join("entity/shop/order/Gone.md").exists());
        assert_eq!(std::fs::read_to_string(dir.join("entity/NOTES.md")).unwrap(), "mine");
        let after = Manifest::read(&dir);
        assert!(
            after.files.contains_key("entity/shop/order/ShopOrder.md")
                && after.files.contains_key("service/shop/OrderServices.md")
        );
        let root = std::fs::read_to_string(dir.join("README.md")).unwrap();
        assert!(
            root.contains("[Entities](entity/README.md)") && root.contains("[Services](service/README.md)"),
            "{root}"
        );

        // the same export again writes nothing
        let again = generate(factory, &source, &plan, &options);
        let third = plan_writes(&dir, &again, &Manifest::read(&dir), &plan, true);
        assert!(
            third.write.is_empty(),
            "{:?}",
            third.write.iter().map(|(path, _)| path).collect::<Vec<_>>()
        );
        assert_eq!(third.unchanged, again.files.len());
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn tidy_keeps_one_blank_line_and_separates_blocks() {
    assert_eq!(
        render::tidy("\n\n# A  \n\n\n\ntext\n```\na\n\n\nb\n```\n\n\n"),
        "# A\n\ntext\n\n```\na\n\n\nb\n```\n"
    );
    assert_eq!(
        render::tidy("text\n## B\n| a |\n| b |\n#hash"),
        "text\n\n## B\n\n| a |\n| b |\n#hash\n",
        "a blank line before a heading and a table"
    );
}
