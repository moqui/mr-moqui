use super::*;

fn scan(xml: &str) -> Vec<(Idiom, String)> {
    let root = MNode::parse_text("test.xml", xml).unwrap();
    let mut file = FileReport::default();
    super::super::screen_lint::scan_for_tests(&root, &mut file);
    refine(&root, &mut file);
    file.sites
        .into_iter()
        .flat_map(|site| site.idioms)
        .filter(|(idiom, _)| {
            matches!(
                idiom,
                Idiom::DecimalOperator
                    | Idiom::FindListMutation
                    | Idiom::InOnList
                    | Idiom::EmptyCollectionTruthiness
                    | Idiom::DuplicateDeclaration
                    | Idiom::StringAsRegex
                    | Idiom::EntityValueIdentity
            )
        })
        .collect()
}

fn kinds(xml: &str) -> Vec<Idiom> {
    scan(xml).into_iter().map(|(idiom, _)| idiom).collect()
}

#[test]
fn decimal_parameters_and_sets_are_tracked() {
    let xml = r#"<services><service verb="a" noun="B">
            <in-parameters><parameter name="hours" type="BigDecimal"/></in-parameters>
            <actions>
                <set field="total" from="toDecimal(0)"/>
                <if condition="!hours"><set field="x" from="1"/></if>
                <if condition="hours != 0"><set field="x" from="1"/></if>
                <if condition="hours != null &amp;&amp; total > 1"><set field="x" from="1"/></if>
                <set field="y" from="hours || 1"/>
                <set field="z" from="hours + 1"/>
            </actions></service></services>"#;
    let found = kinds(xml);
    assert_eq!(
        found.iter().filter(|k| **k == Idiom::DecimalOperator).count(),
        5,
        "{:?}",
        scan(xml)
    );
}

#[test]
fn decimal_methods_nulls_and_keys_are_fine() {
    let xml = r#"<services><service verb="a" noun="B">
            <in-parameters><parameter name="hours" type="BigDecimal"/></in-parameters>
            <actions>
                <if condition="hours == null || toDecimal(hours).isZero()"><set field="x" from="1"/></if>
                <set field="y" from="hours.add(2)"/>
                <set field="z" from="{hours: hours, q: n}"/>
                <set field="c" from="hours != null ? hours : 0"/>
                <set field="d" from="x => x.hours > 1"/>
            </actions></service></services>"#;
    assert_eq!(scan(xml), Vec::new());
}

#[test]
fn a_name_set_to_a_number_elsewhere_is_not_a_decimal() {
    let xml = r#"<services><service verb="a" noun="B">
            <in-parameters><parameter name="n" type="BigDecimal"/></in-parameters>
            <actions>
                <set field="n" from="parseInt(m, 10)"/>
                <if condition="n > 3"><set field="x" from="1"/></if>
            </actions></service></services>"#;
    assert!(kinds(xml).is_empty());
}

#[test]
fn find_list_mutations_are_found_until_the_list_is_copied() {
    let xml = r#"<services><service verb="a" noun="B"><actions>
            <entity-find entity-name="Foo" list="fooList"/>
            <script>fooList.push({a: 1}); const first = fooList.first()</script>
            <entity-find entity-name="Foo" list="copyList"/>
            <set field="copyList" from="Array.from(copyList)"/>
            <script>copyList.push({a: 1})</script>
        </actions></service></services>"#;
    let found = kinds(xml);
    // `push` and `first()` on fooList; copyList was copied.
    assert_eq!(found, vec![Idiom::FindListMutation; 2], "{:?}", scan(xml));
}

#[test]
fn a_cloned_entity_value_compared_with_equals_signs_is_found() {
    let xml = r#"<services><service verb="a" noun="B"><actions>
            <set field="copy" from="orig.cloneValue()"/>
            <if condition="copy == orig"><return/></if>
            <if condition="copy != orig"><return/></if>
            <if condition="copy.equals(orig)"><return/></if>
            <if condition="copy == null"><return/></if>
        </actions></service></services>"#;
    assert_eq!(kinds(xml), vec![Idiom::EntityValueIdentity; 2], "{:?}", scan(xml));
}

#[test]
fn in_on_an_array_is_found_and_in_on_an_object_is_not() {
    let xml = r#"<services><service verb="a" noun="B"><actions>
            <set field="ids" from="[]"/>
            <set field="seen" from="{}"/>
            <if condition="'a' in ids"><set field="x" from="1"/></if>
            <if condition="'a' in seen"><set field="x" from="1"/></if>
            <script>for (const k in ids) { log(k) }</script>
        </actions></service></services>"#;
    assert_eq!(kinds(xml), vec![Idiom::InOnList], "{:?}", scan(xml));
}

#[test]
fn a_collection_parameter_tested_in_a_script_is_a_hazard() {
    let xml = r#"<services><service verb="a" noun="B">
            <in-parameters><parameter name="ids" type="List"/><parameter name="flag" type="Boolean"/></in-parameters>
            <actions><script>
                if (ids) filter.push(ids)
                if (ids?.length) filter.push(ids)
                if (flag) filter.push(flag)
                const n = ids ? 1 : 2
                for (const id of ids) {}
            </script></actions></service></services>"#;
    assert_eq!(kinds(xml), vec![Idiom::EmptyCollectionTruthiness; 2], "{:?}", scan(xml));
}

#[test]
fn an_entity_find_list_tested_for_truthiness_is_a_hazard() {
    let xml = r#"<services><service verb="a" noun="B"><actions>
            <entity-find entity-name="Foo" list="fooList"/>
            <script>
                const first = fooList ? fooList[0].id : null
                const safe = fooList?.length ? fooList[0].id : null
            </script>
            <set field="x" from="fooList &amp;&amp; fooList[0]"/>
        </actions></service></services>"#;
    assert_eq!(kinds(xml), vec![Idiom::EmptyCollectionTruthiness; 2], "{:?}", scan(xml));
}

#[test]
fn a_regular_expression_given_to_a_string_method_as_text_is_a_hazard() {
    let xml = r#"<services><service verb="a" noun="B"><actions>
            <set field="a" from="x.replaceAll('[^a-z0-9-]+', '')"/>
            <set field="b" from="x.replaceAll(' ', '-').replace(/[^a-z]+/g, '')"/>
            <set field="c" from="x.split(':')[0]"/>
            <script>const d = x.matches('\\d+') // not .replaceAll('[a]', '')
            </script>
        </actions></service></services>"#;
    assert_eq!(kinds(xml), vec![Idiom::StringAsRegex; 2], "{:?}", scan(xml));
}

#[test]
fn a_const_declared_twice_at_function_level_is_a_hazard() {
    let xml = r#"<services><service verb="a" noun="B"><actions>
            <script>const dec = (v) => v; let total = 0</script>
            <if condition="x"><script>const dec = (v) => v + 1</script></if>
            <script>for (const dec of rows) {}; const other = 1</script>
            <script>const dec = 2</script>
        </actions></service></services>"#;
    assert_eq!(kinds(xml), vec![Idiom::DuplicateDeclaration], "{:?}", scan(xml));
}

#[test]
fn a_declared_type_rules_out_a_truthiness_warning() {
    let xml = r#"<services><service verb="a" noun="B">
            <in-parameters><parameter name="deleteEntries" type="Boolean"/><parameter name="rowList" type="List"/></in-parameters>
            <actions>
                <if condition="deleteEntries"><set field="x" from="1"/></if>
                <if condition="rowList"><set field="x" from="1"/></if>
            </actions></service></services>"#;
    let found = scan(xml);
    assert_eq!(found, vec![(Idiom::EmptyCollectionTruthiness, "rowList".to_string())]);
}

#[test]
fn last_method_reads_the_end_of_a_chain() {
    assert_eq!(last_method("a.b(c).add(d)"), Some("add"));
    assert_eq!(last_method("toDecimal(x)"), None);
    assert_eq!(last_method("a.map((x) => f(x))"), Some("map"));
    assert!(produces_decimal("toDecimal(x).multiply(y)"));
    assert!(produces_decimal("a ? toDecimal(x) : toDecimal(x).multiply(-1)"));
    assert!(!produces_decimal("toDecimal(x).compare(y) > 0"));
    assert!(!produces_decimal("toDecimal(x).isZero()"));
    assert!(produces_decimal(
        "Array.from(l).reduce((t, i) => t.add(i.q ?? 0), toDecimal(0))"
    ));
    assert!(!produces_decimal("list.map(x => x.id)"));
}
