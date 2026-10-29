//! Generic entity-graph and metadata binding regressions.
use super::*;
use crate::context::ExecutionContextFactory;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "moqui-metadata-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        for name in ["entity", "conf", "service"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        std::fs::copy(
            crate::testing::framework_dir().join("conf/MoquiDefaultConf.xml"),
            dir.join("conf/MoquiDefaultConf.xml"),
        )
        .unwrap();
        std::fs::write(
            dir.join("entity/Graph.xml"),
            include_str!("../testdata/relationship-graph.xml"),
        )
        .unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn holds(source: &str) {
    assert!(eval_condition(source).unwrap(), "{source}");
}

#[test]
fn master_graph_round_trip_inherits_keys_and_respects_read_only_relationships() {
    clear_context_stack();
    let fixture = Fixture::new();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open_at(&fixture.0));
    let _entered = context.enter();
    run_script(
        r#"
        const graph = {parentId:"P", lookupCode:"L", children:[
            {childId:"A", position:0},
            {childId:"B", position:1, edges:[{sourceChildId:"A", kind:"related"}]}
        ], readOnlyChildren:[{childId:"must-not-write", edges:[{kind:"must-not-write"}]}],
        lookup:{}, readOnlyLookup:{code:"must-not-write"}, metadata:{unexpected:"ignored"}};
        rows = ec.entity.getValueListFromPlainMap(graph, "test.graph.Parent");
        for (const row of rows) row.create();
        loaded = ec.entity.find("test.graph.Parent").condition("parentId", "P").oneMaster("default");
        roundTrip = ec.entity.getValueListFromPlainMap(loaded, "test.graph.Parent");
    "#,
    )
    .unwrap();
    holds(r#"rows.length === 5 && roundTrip.length === 4"#);
    holds(r#"rows[0].getEntityName() === "test.graph.Parent" && rows[1].resolveEntityName() === "test.graph.Child""#);
    holds(r#"rows[3].targetChildId === "B" && rows[3].sourceChildId === "A""#);
    // A relationship key may come from a non-primary field, even after another branch was flattened.
    holds(r#"rows[4].code === "L""#);
    holds(
        r#"loaded.parentId === "P" && loaded.children.length === 2 && loaded.children.every(v => v.parentId === "P")"#,
    );
    holds(r#"ec.entity.find("test.graph.Child").condition("childId", "must-not-write").one() === null"#);
    holds(r#"ec.entity.find("test.graph.Edge").count() === 1 && ec.entity.find("test.graph.Lookup").count() === 1"#);
    holds(r#"roundTrip.find(v => v.getEntityName() === "test.graph.Edge").targetChildId === "B""#);
    holds(r#"ec.entity.find("test.graph.Parent").condition("parentId", "absent").oneMaster() === null"#);
    assert!(
        eval_condition(r#"ec.entity.find("test.graph.Parent").condition("parentId", "P").oneMaster("missing")"#)
            .is_err()
    );
}

#[test]
fn master_graph_retains_distinct_self_referential_primary_keys() {
    clear_context_stack();
    let fixture = Fixture::new();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open_at(&fixture.0));
    let _entered = context.enter();
    run_script(
        r#"
        refs = ec.entity.getValueListFromPlainMap({referenceId:"parent", children:[
            {referenceId:"child", parentReferenceId:"parent"}
        ]}, "test.graph.Reference");
        for (const row of refs) row.create();
        master = ec.entity.find("test.graph.Reference").condition("referenceId", "parent").oneMaster();
        flattened = ec.entity.getValueListFromPlainMap(master, "test.graph.Reference");
    "#,
    )
    .unwrap();
    holds(r#"master.children[0].referenceId === "child" && master.children[0].parentReferenceId === "parent""#);
    holds(r#"flattened.length === 2 && flattened[1].referenceId === "child""#);
}

#[test]
fn mounted_transition_metadata_has_service_parameters_and_a_path() {
    clear_context_stack();
    let fixture = Fixture::new();
    let root = fixture.0.join("Root.xml");
    let child = fixture.0.join("Child.xml");
    let outside = fixture.0.join("Outside.xml");
    let screen = r#"<screen><parameter name="screenId" required="true"/><transition name="submit" method="post"><path-parameter name="id"/>
        <parameter name="requiredInput" required="true"/><parameter name="optionalInput"/>
        <parameter name="defaulted" value="fallback"/><service-call name="test.store#Thing"/><default-response type="none"/></transition><widgets/></screen>"#;
    std::fs::write(&child, screen).unwrap();
    std::fs::write(&outside, screen).unwrap();
    std::fs::write(
        &root,
        format!(
            r#"<screen><subscreens>
        <subscreens-item name="bad" location="{}"/><subscreens-item name="child" location="{}"/>
        </subscreens><widgets/></screen>"#,
            fixture.0.join("missing.xml").display(),
            child.display()
        ),
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    put_context_all(&indexmap::indexmap! {
        "rootLocation".to_string() => Value::string(root.to_str().unwrap()),
        "childLocation".to_string() => Value::string(child.to_str().unwrap()),
        "outsideLocation".to_string() => Value::string(outside.to_str().unwrap()),
    });
    run_script(r#"transition = ec.screen.getTransitionInfo(rootLocation, childLocation, "submit", "post")"#).unwrap();
    holds(
        r#"transition.path === "/child/submit" && transition.serviceName === "test.store#Thing" && transition.parameterMap.id.required && transition.pathParameters[0] === "id""#,
    );
    holds(r#"transition.parameterMap.requiredInput.required && !transition.parameterMap.optionalInput.required"#);
    holds(
        r#"transition.parameterMap.defaulted.hasDefault && !transition.parameterMap.requiredInput.hasDefault && !transition.parameterMap.optionalInput.hasDefault && !transition.parameterMap.id.hasDefault"#,
    );
    run_script(r#"actions = ec.screen.getTransitionInfo(rootLocation, childLocation, "actions", "get")"#).unwrap();
    holds(
        r#"actions.path === "/child/actions" && actions.parameterMap.screenId.required && !transition.parameterMap.screenId"#,
    );
    assert!(eval_condition(r#"ec.screen.getTransitionInfo(rootLocation, childLocation, "missing", "post")"#).is_err());
    assert!(eval_condition(r#"ec.screen.getTransitionInfo(rootLocation, childLocation, "submit", "get")"#).is_err());
    assert!(eval_condition(r#"ec.screen.getTransitionInfo(rootLocation, outsideLocation, "submit", "post")"#).is_err());
    let ambiguous = fixture.0.join("Ambiguous.xml");
    std::fs::write(
        &ambiguous,
        format!(
            r#"<screen><subscreens>
        <subscreens-item name="first" location="{}"/><subscreens-item name="second" location="{}"/>
        </subscreens><widgets/></screen>"#,
            child.display(),
            child.display()
        ),
    )
    .unwrap();
    put_context_all(&indexmap::indexmap! { "ambiguousRoot".to_string() => Value::string(ambiguous.to_str().unwrap()) });
    assert!(eval_condition(r#"ec.screen.getTransitionInfo(ambiguousRoot, childLocation, "submit", "post")"#).is_err());
}

#[test]
fn login_metadata_declares_optional_credentials_for_second_factor_continuation() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let login = crate::testing::framework_dir()
        .parent()
        .unwrap()
        .join("webroot/screen/webroot/Login.xml");
    let location = login.to_str().unwrap();
    let Value::Map(info) =
        crate::screen::info::transition_info(context.factory(), location, location, "login", "post").unwrap()
    else {
        panic!("transition metadata");
    };
    assert_eq!(info["path"], Value::string("/login"));
    let Value::Map(parameters) = &info["parameterMap"] else {
        panic!("parameters");
    };
    for name in ["username", "password", "code"] {
        let Value::Map(parameter) = &parameters[name] else {
            panic!("parameter");
        };
        assert_eq!(parameter["required"], Value::Bool(false));
    }
}

#[test]
fn service_parameter_metadata_is_available_to_scripts() {
    clear_context_stack();
    let fixture = Fixture::new();
    let service = fixture.0.join("service/Meta.xml");
    std::fs::write(
        &service,
        r#"<services><service verb="inspect" noun="Metadata" authenticate="false">
        <in-parameters><parameter name="requiredId" required="true"/><parameter name="optionalId"/>
            <auto-parameters entity-name="moqui.basic.Enumeration" include="pk" required="true"/></in-parameters>
        <out-parameters><parameter name="result" type="Map"/>
            <auto-parameters entity-name="moqui.basic.EnumerationType" include="pk" required="true"/></out-parameters>
        <actions><set field="result" from="{}"/></actions>
    </service></services>"#,
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    context.factory().set_entity(crate::testing::open());
    assert!(crate::service::lint(&[service]).unwrap().is_empty());
    run_script(r#"definition = ec.service.getServiceDefinition("Meta.inspect#Metadata")"#).unwrap();
    holds(r#"definition.inParameterInfoMap.requiredId.required && !definition.inParameterInfoMap.optionalId.required"#);
    holds(
        r#"definition.inParameterNames.includes("requiredId") && definition.inParameterNames.includes("enumId") && definition.getInParameter("requiredId").attribute("required") === "true""#,
    );
    holds(r#"definition.getInParameter("missing") === null && definition.outParameterInfoMap.result.type === "Map""#);
    holds(
        r#"definition.inParameterInfoMap.enumId.required && definition.outParameterInfoMap.enumTypeId.required && !definition.outParameterInfoMap.result.required"#,
    );
}

#[test]
fn service_parameter_node_exposes_the_declared_parameter_to_scripts() {
    clear_context_stack();
    let fixture = Fixture::new();
    let service = fixture.0.join("service/Shape.xml");
    std::fs::write(
        &service,
        r#"<services><service verb="inspect" noun="Shape" authenticate="false">
        <in-parameters>
            <parameter name="richId" format="yyyy-MM" allow-html="safe" required="disabled" default-value="abc">
                <description>The rich one</description>
                <matches regexp="[a-z]+" message="letters only"/>
                <number-range min="1" max="9"/>
                <val-or><text-email/><text-url/></val-or>
            </parameter>
            <parameter name="mapId" type="Map"><parameter name="inner" required="true"/></parameter>
        </in-parameters>
    </service></services>"#,
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    assert!(crate::service::lint(&[service]).unwrap().is_empty());
    run_script(r#"definition = ec.service.getServiceDefinition("Shape.inspect#Shape")"#).unwrap();
    run_script(r#"node = definition.inParameterInfoMap.richId.parameterNode"#).unwrap();
    holds(
        r#"node.name === "parameter" && node.attribute("name") === "richId" && node.attribute("format") === "yyyy-MM" && node.attribute("allow-html") === "safe" && node.attribute("required") === "disabled" && node.attribute("default-value") === "abc" && node.attribute("type") === null"#,
    );
    holds(
        r#"node.first("description").text === "The rich one" && node.children().map(child => child.name).join() === "description,matches,number-range,val-or""#,
    );
    holds(
        r#"node.first("matches").attribute("regexp") === "[a-z]+" && node.first("matches").attribute("message") === "letters only" && node.first("number-range").attribute("min") === "1" && node.first("number-range").attribute("max-include-equals") === null"#,
    );
    holds(r#"node.first("val-or").children().map(child => child.name).join() === "text-email,text-url""#);
    holds(
        r#"definition.getInParameter("richId").attribute("format") === "yyyy-MM" && definition.getInParameter("richId").toString() === node.toString()"#,
    );
    holds(r#"!("parameterXml" in definition.inParameterInfoMap.richId)"#);
    // An element below the root is written when asked, as MNode.write_string writes it.
    holds(r#"node.first("val-or").toString() === "<val-or>\n    <text-email/>\n    <text-url/>\n</val-or>""#);
    holds(r#"node.first("description").toString() === "<description><![CDATA[The rich one]]></description>""#);
    holds(r#"node.first("matches").toString() === '<matches regexp="[a-z]+" message="letters only"/>'"#);
    holds(r#"node.toString().includes(node.first("val-or").toString().split("\n").join("\n    "))"#);
    holds(
        r#"definition.inParameterInfoMap.mapId.parameterNode.attribute("type") === "Map" && definition.inParameterInfoMap.mapId.parameterNode.children("parameter")[0].attribute("required") === "true""#,
    );
}

#[test]
fn convert_validate_clean_parameters_checks_without_running_the_service() {
    clear_context_stack();
    let fixture = Fixture::new();
    let service = fixture.0.join("service/Checked.xml");
    std::fs::write(
        &service,
        r#"<services><service verb="apply" noun="Title" authenticate="false">
        <in-parameters>
            <parameter name="title" required="true"><text-length max="5"/></parameter>
            <parameter name="count" type="Integer"/>
        </in-parameters>
        <actions><script>throw new Error('ran')</script></actions>
    </service></services>"#,
    )
    .unwrap();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    assert!(crate::service::lint(&[service]).unwrap().is_empty());
    run_script(r#"definition = ec.service.getServiceDefinition("Checked.apply#Title")"#).unwrap();
    holds(
        r#"definition.convertValidateCleanParameters({title: "abc", count: "3", extra: 1}).count === 3 && !ec.message.hasError()"#,
    );
    holds(
        r#"definition.convertValidateCleanParameters({title: "abcdefg"}).title === "abcdefg" && ec.message.hasError()"#,
    );
    run_script("ec.message.clearErrors()").unwrap();
    holds(
        r#"definition.convertValidateCleanParameters({}) && ec.message.getErrorsString().includes("Field cannot be empty")"#,
    );
}
