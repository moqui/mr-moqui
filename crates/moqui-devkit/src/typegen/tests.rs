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

//! Golden tests for the emitters: the fixtures in `typegen/testdata` and every file `moqui types` writes for them, in
//! `typegen/testdata/expected`. Set `UPDATE_GOLDEN=1` to rewrite the expected files, then read the diff. The host's
//! `types` integration test runs `tsc --noEmit` over the expected files with `usage-fixture.ts`.

use std::sync::Arc;

use moqui_db::model::EntityModel;
use moqui_lib::node::MNode;

use super::{assemble_files, fixture_root, screen_sources, testdata, TypeFiles};
use moqui_core::context::ExecutionContextFactory;
use moqui_core::screen::ScreenFacade;

const SCREEN_LOCATION: &str = "component://typegen/screen/Invoices.xml";

fn model() -> EntityModel {
    let conf = MNode::parse_file(moqui_core::testing::framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap();
    EntityModel::load(&conf, &[fixture_root("entities.xml")]).unwrap()
}

fn fixture_files() -> TypeFiles {
    let model = model();
    let (services, failures) =
        moqui_core::service::define::definitions_in(&[testdata().join("service/typegen/services.xml")]).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let factory = Arc::clone(ctx.factory());
    let facade = ScreenFacade::of(&factory);
    let path = testdata().join("Invoices.xml");
    let def = facade.screen_def(&factory, &path.to_string_lossy()).unwrap();
    let defs = vec![(SCREEN_LOCATION.to_string(), def)];
    let sources = screen_sources(&facade, &factory, &defs);
    assert!(sources[0].server_static, "the fixture screen is static for qvt");
    let failures = vec![(
        "component://typegen/screen/Broken.xml".to_string(),
        "Broken.xml: not a screen\nsecond line".to_string(),
    )];
    assemble_files(Some(&model), &services, &sources, &failures)
}

fn check(files: &TypeFiles, name: &str) {
    let actual = files.get(name).unwrap_or_else(|| panic!("no {name}"));
    let path = testdata().join("expected").join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(actual, expected, "{name} differs from {}", path.display());
}

#[test]
fn golden_every_file_for_the_fixtures() {
    let files = fixture_files();
    let names: Vec<&str> = files.files().iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "moqui-host.d.ts",
            "moqui-globals.d.ts",
            "entities.d.ts",
            "services.d.ts",
            "screens.d.ts",
            "shape.ts",
            "tsconfig.json"
        ]
    );
    for name in names {
        check(&files, name);
    }
    // Generating twice gives the same text.
    let again = fixture_files();
    assert_eq!(files.files(), again.files());
}

#[test]
fn entities_type_fields_keys_and_every_name_the_facade_resolves() {
    let files = fixture_files();
    let text = files.get("entities.d.ts").unwrap();
    let invoice = text
        .split("interface typegen_test_Invoice {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .unwrap();
    for line in [
        "\"invoiceId\": string;",
        "\"amount\": Decimal;",
        "\"invoiceDate\": Timestamp;",
        "\"dueDate\": string;",
        "\"lineCount\": number;",
        "\"document\": never;",
        "pk: \"invoiceId\";",
        "\"status\": \"typegen.test.Status\";",
        "\"typegen.test.Status\": \"typegen.test.Status\";",
    ] {
        assert!(invoice.contains(line), "{line} in {invoice}");
    }
    // A titled relationship is found by its full name only, and an untitled one by the related entity name too.
    assert!(
        invoice.contains("\"Line#typegen.test.InvoiceItem\": \"typegen.test.InvoiceItem\";"),
        "{invoice}"
    );
    assert!(!invoice.contains("\"InvoiceItem\": "), "{invoice}");
    assert!(invoice.contains("\"Status\": \"typegen.test.Status\";"), "{invoice}");
    let map = text.split("interface MoquiEntityMap {").nth(1).unwrap();
    assert!(map.contains("\"inv\": MoquiEntity.typegen_test_Invoice;"), "{map}");
    assert!(map.contains("\"Invoice\": MoquiEntity.typegen_test_Invoice;"), "{map}");
    // Two entities are named Status, so neither is in the map by that name.
    assert!(!map.contains("\"Status\":"), "{map}");
    // The view takes its alias types from the members, and the excluded field is not an alias.
    let view = text.split("interface typegen_test_InvoiceAndStatus {").nth(1).unwrap();
    let view = view.split("\n    }\n").next().unwrap();
    assert!(
        view.contains("\"amount\": Decimal;") && view.contains("\"statusDescription\": string;"),
        "{view}"
    );
    assert!(!view.contains("document"), "{view}");
}

#[test]
fn services_mark_required_keys_as_the_validation_does() {
    let files = fixture_files();
    let text = files.get("services.d.ts").unwrap();
    let send = text
        .split("\"typegen.services.send#Invoice\": {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }").next())
        .unwrap();
    assert!(send.contains("\"invoiceId\": string;"), "{send}");
    // Required with a default-value, so the caller may leave it out.
    assert!(send.contains("\"copies\"?: number | string | null;"), "{send}");
    assert!(send.contains("\"sendDate\"?: TimestampInput | null;"), "{send}");
    assert!(send.contains("\"total\"?: DecimalInput | null;"), "{send}");
    assert!(!send.contains("hidden"), "{send}");
    // auto-parameters expand against the entity model; the primary key is required there.
    let status = text
        .split("\"typegen.services.update#InvoiceStatus\": {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }").next())
        .unwrap();
    assert!(status.contains("\"invoiceId\": string;"), "{status}");
    assert!(
        status.contains("\"amount\"?: DecimalInput | null;") && !status.contains("document"),
        "{status}"
    );
    // validate="false" lets other keys through, an entity-auto service keeps any field of its entity, and a service with
    // no parameters takes none.
    assert!(
        text.contains("\"typegen.services.echo#Anything\": {\n        \"text\"?: string | null;\n    } & MoquiMap;"),
        "{text}"
    );
    let create = text
        .split("\"typegen.services.create#typegen.test.Status\": {")
        .nth(1)
        .unwrap();
    assert!(
        create.starts_with("\n        \"description\"?: string | null;\n        \"lastUpdatedStamp\"?: TimestampInput | null;\n        \"statusId\": string;"),
        "{create}"
    );
    assert!(
        text.contains("\"typegen.services.noop#Nothing\": Record<string, never>;"),
        "{text}"
    );
    // An entity that is not loaded is a note.
    assert!(
        text.contains("// not expanded: typegen.services.lookup#Missing: auto-parameters:"),
        "{text}"
    );
    assert!(
        files.notes.iter().any(|note| note.contains("lookup#Missing")),
        "{:?}",
        files.notes
    );
    // Outputs: declared keys only, all optional.
    let out = text.split("interface MoquiServiceOut {").nth(1).unwrap();
    assert!(
        out.contains("\"sent\"?: boolean | null;") && out.contains("\"total\"?: Decimal | null;"),
        "{out}"
    );
}

#[test]
fn screens_list_proven_keys_and_the_static_package() {
    let files = fixture_files();
    let text = files.get("screens.d.ts").unwrap();
    for line in [
        // The entity-find-one fills the form's map, so the entity value wins over the form's fields.
        "\"invoice\"?: EntityValue<\"typegen.test.Invoice\"> | null;",
        "\"itemList\"?: EntityList<\"typegen.test.InvoiceItem\">;",
        "\"itemCount\"?: number;",
        "\"sendOut\"?: MoquiOut<\"typegen.services.send#Invoice\">;",
        "\"pageSize\"?: number;",
        "\"names\"?: unknown[];",
        "\"shellTitle\"?: string;",
        "\"dynamicName\"?: unknown;",
        "\"statusId\"?: unknown;",
        "\"fieldValues\"?: { \"query\"?: unknown };",
        "\"invoiceList\"?: EntityList<\"typegen.test.Invoice\">;",
        "\"invoiceListPageIndex\"?: number;",
    ] {
        assert!(text.contains(line), "{line} in {text}");
    }
    assert!(!text.contains("scriptOnly") && !text.contains("\"nested"), "{text}");
    assert!(
        text.contains("// not loaded: component://typegen/screen/Broken.xml: Broken.xml: not a screen\n"),
        "{text}"
    );
    let package = text.split("interface MoquiScreenPackages {").nth(1).unwrap();
    assert!(
        package.contains(
            "MoquiScreenPackage<MoquiScreen.typegen_screen_Invoices, \"invoiceList\", { \"mqExtra_EditInvoice\"?: { \"mqOpt_EditInvoice_statusId_0_0\"?: MoquiOption[] } }>;"
        ),
        "{package}"
    );
    let form_list = text.split("interface MoquiFormListPackages {").nth(1).unwrap();
    assert!(form_list.contains("\"component://typegen/screen/Invoices.xml#Invoices\": MoquiFormListPackage<\"invoiceList\", MoquiPlain<\"typegen.test.Invoice\"> & {"), "{form_list}");
    assert!(
        form_list.contains("\"statusId_display\"?: string") && form_list.contains("\"statusShown\"?: unknown"),
        "{form_list}"
    );
    assert!(
        form_list.contains("\"invoiceId_op\"?: unknown")
            && form_list.contains("\"mqOpt_Invoices_nextStatus_0_0\"?: MoquiOption[]"),
        "{form_list}"
    );
}

#[test]
fn host_and_shape_take_the_names_from_the_rust_constants() {
    let files = fixture_files();
    let host = files.get("moqui-host.d.ts").unwrap();
    assert!(host.contains("type MoquiPageSuffix = \"Count\" | \"PageIndex\" | \"PageSize\" | \"PageMaxIndex\" | \"PageRangeLow\" | \"PageRangeHigh\";"), "{host}");
    assert!(host.contains("\"currentpassword\"") && host.contains("\"api_key\""));
    let shape = files.get("shape.ts").unwrap();
    assert!(
        shape.contains("export const PAGE_SUFFIXES = [\"Count\", \"PageIndex\""),
        "{shape}"
    );
    assert!(
        shape.contains("`mqOpt_${form}_${field}_${subIndex}_${widgetIndex}`"),
        "{shape}"
    );
    assert_eq!(
        moqui_core::screen::statics::option_key("F", "f", 1, 2),
        "mqOpt_F_f_1_2",
        "shape.ts writes the same key"
    );
}

#[test]
fn tools_declare_their_types_in_the_tool_map() {
    let text = super::tools_text(&[("ExampleTool".to_string(), " { greet(name: string): string } ")]);
    assert!(text.contains("interface MoquiToolMap {\n"), "{text}");
    assert!(
        text.contains("    \"ExampleTool\": { greet(name: string): string };\n"),
        "{text}"
    );
    assert!(super::tools_text(&[]).ends_with("interface MoquiToolMap {\n}\n"));
}
