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

use super::*;

fn rewrite(text: &str) -> String {
    rewrite_expression(text).0
}

#[test]
fn map_literals_become_objects_and_lists_and_indexes_stay() {
    assert_eq!(rewrite("[a: 1, b: 'x']"), "{a: 1, b: 'x'}");
    assert_eq!(rewrite("[:]"), "{}");
    assert_eq!(rewrite("[1, 2, [a:3]]"), "[1, 2, {a:3}]");
    assert_eq!(rewrite("list[0].x"), "list[0].x");
    assert_eq!(rewrite("map['k']"), "map['k']");
    assert_eq!(rewrite("[(name): v, 'q r': 2]"), "{[name]: v, 'q r': 2}");
    assert_eq!(rewrite("[x ? 1 : 2, y]"), "[x ? 1 : 2, y]");
    assert_eq!(rewrite("f('[a: 1]', [b: \"[c:2]\"])"), "f('[a: 1]', {b: \"[c:2]\"})");
    assert_eq!(rewrite("return [ok: true]"), "return {ok: true}");
    assert_eq!(rewrite("[a: m[0], b: [c: f(x)[1]]]"), "{a: m[0], b: {c: f(x)[1]}}");
}

#[test]
fn context_plus_becomes_an_object_assign() {
    let (text, kinds, _) = rewrite_expression("context + [flattenDocument: false]");
    assert_eq!(text, "Object.assign({}, context, {flattenDocument: false})");
    assert_eq!(kinds, vec![RewriteKind::ContextPlus]);
    assert_eq!(
        rewrite("context+[a:1, b:[c:2]]"),
        "Object.assign({}, context, {a:1, b:{c:2}})"
    );
    let (text, kinds, left) = rewrite_expression("baseMap + [a: 1]");
    assert_eq!(text, "baseMap + [a: 1]");
    assert!(kinds.is_empty());
    assert_eq!(left.len(), 1);
    assert_eq!(rewrite("mycontext + [a: 1]"), "mycontext + [a: 1]");
    assert_eq!(rewrite("x = [a: 1] + 2"), "x = {a: 1} + 2");
}

#[test]
fn elvis_becomes_or_unless_a_ternary_follows() {
    assert_eq!(rewrite("a ?: 'x'"), "a || 'x'");
    assert_eq!(
        rewrite("${product?.productName ?: productId}"),
        "${product?.productName || productId}"
    );
    assert_eq!(rewrite("a?:b?:c"), "a||b||c");
    assert_eq!(rewrite("'?:' + x"), "'?:' + x");
    assert_eq!(rewrite("s.replaceAll(/(?:x)/, '')"), "s.replaceAll(/(?:x)/, '')");
    let (text, kinds, left) = rewrite_expression("a ?: b ? c : d");
    assert_eq!(text, "a ?: b ? c : d");
    assert!(kinds.is_empty());
    assert_eq!(left, vec!["Elvis before a ternary"]);
    assert_eq!(rewrite("f(a ?: b, c ? d : e)"), "f(a || b, c ? d : e)");
}

#[test]
fn xml_places_are_converted_and_the_rest_is_copied() {
    let xml = r#"<services>
    <!-- a ?: b in a comment -->
    <service verb="get" noun="Thing">
        <description>Prose with ?: stays</description>
        <actions>
            <service-call name="x.y#Z" in-map="context + [flattenDocument:false]" out-map="context"/>
            <set field="name" from="product?.productName ?: productId"/>
            <message>Name ${name ?: 'none'}</message>
            <script><![CDATA[def m = [a: 1]; out = m.a ?: 2]]></script>
        </actions>
    </service>
</services>"#;
    let file = convert_xml(xml);
    assert!(
        file.text
            .contains("in-map=\"Object.assign({}, context, {flattenDocument:false})\""),
        "{}",
        file.text
    );
    assert!(
        file.text.contains("from=\"product?.productName || productId\""),
        "{}",
        file.text
    );
    assert!(file.text.contains("Name ${name || 'none'}"), "{}", file.text);
    assert!(file.text.contains("def m = {a: 1}; out = m.a || 2"), "{}", file.text);
    assert!(file.text.contains("<!-- a ?: b in a comment -->"), "{}", file.text);
    assert!(file.text.contains("Prose with ?: stays"), "{}", file.text);
    assert_eq!(file.rewrites.len(), 4);
    assert_eq!(file.rewrites[0].line, 6);
}

#[test]
fn an_entity_field_default_is_an_expression() {
    let file = convert_xml(
        r#"<entity><field name="sequenceNum" type="number-integer" default="fieldSeqId as int"/></entity>"#,
    );
    assert!(
        file.text
            .contains(r#"default="(fieldSeqId == null ? null : Math.trunc(toDecimal(fieldSeqId).toNumber()))""#),
        "{}",
        file.text
    );
}

#[test]
fn a_grammar_file_becomes_javascript_the_edi_handler_reads() {
    let groovy = "// File Structure\n[\n    [ID:'ISA',MIN:0,MAX:99999,\n        LEVEL: [\n            [ID:'GS',MIN:0,MAX:99999, LEVEL: [\n                    [ID:'ST',MIN:0,MAX:99999],\n                ]\n            ],\n            [ID:'IEA',MIN:1,MAX:1]\n        ]\n    ]\n]\n";
    assert!(is_grammar(groovy));
    let js = convert_literals(groovy);
    assert!(js.contains("{ID:'ISA',MIN:0,MAX:99999,"), "{js}");
    assert!(js.contains("{ID:'ST',MIN:0,MAX:99999},"), "{js}");
    moqui_core::script::parse_js(&js).unwrap();
    assert!(!is_grammar("def x = 1\n[a]"));
}

#[test]
fn a_tree_converts_grammars_and_their_locations_and_writes_only_when_asked() {
    let dir = std::env::temp_dir().join(format!("moqui-groovy-convert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("service/edi")).unwrap();
    std::fs::write(dir.join("service/edi/X12Test.groovy"), "[[ID:'ST', MIN:1, MAX:1]]\n").unwrap();
    let services = r#"<services><service verb="a" noun="B"><in-parameters>
            <parameter name="bodyLocation" default-value="component://c/service/edi/X12Test.groovy"/></in-parameters>
            <actions><set field="x" from="y ?: 1"/></actions></service></services>"#;
    std::fs::write(dir.join("service/EdiServices.xml"), services).unwrap();
    let report = convert_tree(std::slice::from_ref(&dir), false).unwrap();
    assert_eq!(report.count(RewriteKind::GrammarFile), 1);
    assert_eq!(report.count(RewriteKind::GrammarLocation), 1);
    assert_eq!(report.count(RewriteKind::Elvis), 1);
    assert!(!dir.join("service/edi/X12Test.js").exists(), "a dry run writes nothing");
    assert!(report.render().contains("X12Test.js"));
    convert_tree(std::slice::from_ref(&dir), true).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("service/edi/X12Test.js")).unwrap(),
        "[{ID:'ST', MIN:1, MAX:1}]\n"
    );
    let written = std::fs::read_to_string(dir.join("service/EdiServices.xml")).unwrap();
    assert!(written.contains("component://c/service/edi/X12Test.js\""), "{written}");
    assert!(written.contains("from=\"y || 1\""), "{written}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_found_list_in_a_condition_is_tested_for_rows() {
    let xml = r#"<actions>
            <entity-find entity-name="a.B" list="fooList"><econdition field-name="x"/></entity-find>
            <if condition="fooList"><log message="x"/></if>
            <if condition="!fooList &amp;&amp; otherList"><log message="x"/></if>
            <if condition="fooList.size() &gt; 1 || fooList[0] || 'fooList'"><log message="x"/></if>
        </actions>"#;
    let converted = convert_xml(xml);
    assert!(
        converted.text.contains(r#"<if condition="(fooList?.length > 0)">"#),
        "{}",
        converted.text
    );
    assert!(
        converted
            .text
            .contains(r#"<if condition="!fooList?.length &amp;&amp; otherList">"#),
        "{}",
        converted.text
    );
    assert!(
        converted
            .text
            .contains(r#"<if condition="fooList.size() &gt; 1 || fooList[0] || 'fooList'">"#),
        "{}",
        converted.text
    );
    assert_eq!(
        converted
            .rewrites
            .iter()
            .filter(|rewrite| rewrite.kinds == vec![RewriteKind::EmptyList])
            .count(),
        2
    );
}

#[test]
fn string_equals_casts_includes_and_empty_collections() {
    let lists: BTreeSet<String> = ["orderList".to_string()].into_iter().collect();
    let rewrite = |text: &str| rewrite_expression_with(text, &lists).0;
    assert_eq!(
        rewrite("'DEBIT'.equals(rootId) ? 'N' : 'Y'"),
        "('DEBIT' === rootId) ? 'N' : 'Y'"
    );
    assert_eq!(
        rewrite("party.partyTypeEnumId.equals(\"PtyPerson\")"),
        "(party.partyTypeEnumId === \"PtyPerson\")"
    );
    assert_eq!(rewrite("foo(a).equals('x')"), "foo(a).equals('x')", "not a name chain");
    assert_eq!(rewrite("curYear as String"), "curYear?.toString()");
    assert_eq!(rewrite("(curYear + 1) as String"), "(curYear + 1)?.toString()");
    assert_eq!(
        rewrite("item.quantity as BigDecimal"),
        "(item.quantity == null ? null : toDecimal(item.quantity))"
    );
    assert_eq!(
        rewrite("x as int"),
        "(x == null ? null : Math.trunc(toDecimal(x).toNumber()))"
    );
    assert_eq!(
        rewrite("if ('A'.equals(x)) y"),
        "if ('A' === x) y",
        "already in parentheses"
    );
    assert_eq!(
        rewrite("(total as BigDecimal).add(1)"),
        "(total == null ? null : toDecimal(total)).add(1)"
    );
    assert_eq!(
        rewrite("'n' + count as String"),
        "'n' + count as String",
        "the sum is cast in Groovy"
    );
    assert_eq!(rewrite("x = count as String"), "x = count?.toString()");
    let (text, _, left) = rewrite_expression_with("foo(a) as BigDecimal", &lists);
    assert_eq!(text, "foo(a) as BigDecimal");
    assert_eq!(
        left,
        vec!["cast of an expression that is not a name chain, or under a tighter operator"]
    );
    assert_eq!(rewrite("'a as String'"), "'a as String'", "strings are left");
    assert_eq!(
        rewrite("asset.'mantle.product.Product'"),
        "asset['mantle.product.Product']"
    );
    assert_eq!(
        rewrite("srs?.'Origin#mantle.facility.Facility'?.facilityName"),
        "srs?.['Origin#mantle.facility.Facility']?.facilityName"
    );
    assert_eq!(
        rewrite("x + '.' + 'y'"),
        "x + '.' + 'y'",
        "a string after a string is not a property"
    );
    assert_eq!(rewrite("\"${a} 00:00:00.000\""), "`${a} 00:00:00.000`");
    assert_eq!(rewrite("\"plain\""), "\"plain\"", "no interpolation");
    assert_eq!(rewrite("\"\"\"${a}\n${b}\"\"\""), "`${a}\n${b}`");
    let (text, _, left) = rewrite_expression_with("\"Hi $name\"", &lists);
    assert_eq!(text, "\"Hi $name\"");
    assert_eq!(
        left,
        vec!["GString with a bare $name, a nested double quote, or a backtick"]
    );
    assert_eq!(
        rewrite("orderList.contains(id) || 'ab'.contains(c) || other.contains(d)"),
        "orderList.includes(id) || 'ab'.includes(c) || other.contains(d)"
    );
    assert_eq!(
        rewrite("x = new ArrayList<String>(); m = new LinkedHashMap(); n = new HashMap(map)"),
        "x = []; m = {}; n = new HashMap(map)"
    );
}

#[test]
fn multi_line_and_nested_maps_become_objects() {
    assert_eq!(
        rewrite("[\n    a: 1,\n    b: [c: 2,\n         d: [e: 3]],\n    (k): v\n]"),
        "{\n    a: 1,\n    b: {c: 2,\n         d: {e: 3}},\n    [k]: v\n}"
    );
    assert_eq!(
        rewrite("[a: [1, 2,\n  3], b: [:],\n // note: [x: 1]\n c: 'x']"),
        "{a: [1, 2,\n  3], b: {},\n // note: [x: 1]\n c: 'x'}"
    );
    let once = rewrite("[\n a: 1,\n b: [c: 2]\n]");
    assert_eq!(rewrite(&once), once, "idempotent");
}

#[test]
fn in_a_list_literal_becomes_includes() {
    assert_eq!(rewrite("statusId in ['A', 'B']"), "['A', 'B'].includes(statusId)");
    assert_eq!(rewrite("!(statusId in ['A', 'B'])"), "!(['A', 'B'].includes(statusId))");
    assert_eq!(
        rewrite("statusChanged &amp;&amp; payment.statusId in ['A','B'] &amp;&amp; x == 1"),
        "statusChanged &amp;&amp; ['A','B'].includes(payment.statusId) &amp;&amp; x == 1"
    );
    assert_eq!(
        rewrite("a && (b?.c in ['A']) || d in [1, 2]"),
        "a && (['A'].includes(b?.c)) || [1, 2].includes(d)"
    );
    assert_eq!(rewrite("x.get('k') in ['A']"), "['A'].includes(x.get('k'))");
    assert_eq!(
        rewrite("ok = oldStatusId in [\n 'A',\n 'B']"),
        "ok = [\n 'A',\n 'B'].includes(oldStatusId)"
    );
    assert_eq!(rewrite("c ? x in ['A'] : y"), "c ? ['A'].includes(x) : y");
    assert_eq!(rewrite("return x in ['A']"), "return ['A'].includes(x)");
    // Left alone: other precedence, loops, non-literals, strings.
    for same in [
        "!x in ['A']",
        "a + x in ['A']",
        "a &lt; x in ['A']",
        "for (x in ['A', 'B']) { f(x) }",
        "for (String x in ['A', 'B']) { f(x) }",
        "x in someList",
        "x in {a: 1}",
        "'x in [1]'",
        "x in ['A'].size()",
        "// x in ['A']",
    ] {
        assert_eq!(rewrite(same), same);
    }
    let once = rewrite("!(a in ['A']) && b in ['B']");
    assert_eq!(rewrite(&once), once, "idempotent");
    let (_, kinds, _) = rewrite_expression("x in ['A']");
    assert_eq!(kinds, vec![RewriteKind::InList]);
}

#[test]
fn more_cast_operand_shapes() {
    assert_eq!(
        rewrite("(convertDurOut.convertedAmount || 60) as int"),
        "((convertDurOut.convertedAmount || 60) == null ? null : Math.trunc(toDecimal((convertDurOut.convertedAmount || 60)).toNumber()))"
    );
    assert_eq!(
        rewrite("f(((a + b) as BigDecimal).divide(c, 3))"),
        "f(((a + b) == null ? null : toDecimal((a + b))).divide(c, 3))"
    );
    assert_eq!(
        rewrite("x = list[0].qty as BigDecimal"),
        "x = (list[0].qty == null ? null : toDecimal(list[0].qty))"
    );
    assert_eq!(rewrite("x = foo(a).bar as String"), "x = foo(a).bar?.toString()");
    assert_eq!(rewrite("x = a.b(c)[0]?.d as String"), "x = a.b(c)[0]?.d?.toString()");
    // A call would run twice, and a tighter operator before the operand moves the cast.
    assert_eq!(rewrite("x = foo(a) as int"), "x = foo(a) as int");
    assert_eq!(
        rewrite("x = (f(a) || 1) as BigDecimal"),
        "x = (f(a) || 1) as BigDecimal"
    );
    assert_eq!(rewrite("x = (a ? 1 : 2) as int"), "x = (a ? 1 : 2) as int");
    assert_eq!(rewrite("x = 1 + (a || 1) as int"), "x = 1 + (a || 1) as int");
    assert_eq!(rewrite("a &gt; b as int"), "a &gt; b as int");
    assert_eq!(rewrite("a &gt;= b as int"), "a &gt;= b as int");
    let once = rewrite("x = (a || 60) as int");
    assert_eq!(rewrite(&once), once, "idempotent");
}

/// The six grammar files of mantle-edi convert to the JavaScript moqui-core's EDI tests load (`edi/testdata/mantle`).
#[test]
fn the_mantle_edi_grammars_convert_to_the_scripts_the_edi_tests_load() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../moqui-core/src/edi/testdata/mantle");
    for name in [
        "X12Envelope",
        "X12V4010TS810",
        "X12V4010TS850",
        "X12V4010TS855",
        "X12V4010TS856",
        "X12V4010TS997",
    ] {
        let groovy = std::fs::read_to_string(dir.join(format!("{name}.groovy"))).unwrap();
        let js = std::fs::read_to_string(dir.join(format!("{name}.js"))).unwrap();
        assert_eq!(super::convert_literals(&groovy), js, "{name}");
    }
}
