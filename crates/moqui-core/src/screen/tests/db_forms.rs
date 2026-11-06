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

//! A dated `DbForm` rendered as of a date (docs/ui/Db-Forms.md): its ended fields and options and its drafts left out
//! now, a successor field in place of the field it replaces, the form as it was at an earlier date, the drafts in a
//! preview, and its dependencies as `visible-when` and `require-when` in html, qvt and tui.

use moqui_lib::object::Value;

use super::{request, World};
use crate::screen::serve_at;

const JAN_2020: i64 = 1_577_836_800;
const JAN_2021: i64 = 1_609_459_200;

fn stamp(seconds: i64) -> Value {
    Value::Timestamp(jiff::Timestamp::from_second(seconds).expect("a time in range"))
}

fn seed(world: &World) {
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let row = |entity: &str, fields: &[(&str, Value)]| {
        let mut value = world.facade.make_value(entity).unwrap();
        for (name, field) in fields {
            value.set(*name, field.clone());
        }
        value.create(&world.facade).unwrap();
    };
    let text = |value: &str| Value::string(value);
    let form = || ("formId", text("CS_TPL"));
    row("moqui.screen.form.DbForm", &[form(), ("isDated", text("Y"))]);
    let field = |name: &str, title: &str, kind: &str, seq: &str, dates: &[(&str, Value)]| {
        let mut fields = vec![
            form(),
            ("fieldName", text(name)),
            ("title", text(title)),
            ("fieldTypeEnumId", text(kind)),
            ("layoutSequenceNum", text(seq)),
        ];
        fields.extend(dates.iter().cloned());
        row("moqui.screen.form.DbFormField", &fields);
    };
    let from_2020 = ("fromDate", stamp(JAN_2020));
    field("kind", "Kind", "DBFFT_drop-down", "1", std::slice::from_ref(&from_2020));
    field(
        "serial",
        "Serial",
        "DBFFT_text-line",
        "2",
        std::slice::from_ref(&from_2020),
    );
    field(
        "oldQ",
        "Old question",
        "DBFFT_text-line",
        "3",
        &[from_2020.clone(), ("thruDate", stamp(JAN_2021))],
    );
    field(
        "newQ",
        "New question",
        "DBFFT_text-line",
        "3",
        &[("fromDate", stamp(JAN_2021)), ("replacesFieldName", text("oldQ"))],
    );
    field("draftQ", "Draft question", "DBFFT_text-line", "4", &[]);
    // a field with no type is left out, and the form renders without it
    field("noType", "No type", "", "5", std::slice::from_ref(&from_2020));
    let option = |seq: &str, key: &str, label: &str, dates: &[(&str, Value)]| {
        let mut fields = vec![
            form(),
            ("fieldName", text("kind")),
            ("sequenceNum", text(seq)),
            ("keyValue", text(key)),
            ("text", text(label)),
        ];
        fields.extend(dates.iter().cloned());
        row("moqui.screen.form.DbFormFieldOption", &fields);
    };
    option("1", "A", "Alpha", &[from_2020.clone(), ("thruDate", stamp(JAN_2021))]);
    option("2", "B", "Bravo", std::slice::from_ref(&from_2020));
    option("3", "D", "Delta", &[]);
    let dependency = |seq: &str, operator: &str, value: &str, target: &str, effect: &str| {
        row(
            "moqui.screen.form.DbFormFieldDependency",
            &[
                form(),
                ("dependencySeqId", text(seq)),
                ("sourceFieldName", text("kind")),
                ("operator", text(operator)),
                ("value", text(value)),
                ("targetFieldName", text(target)),
                ("effect", text(effect)),
                from_2020.clone(),
            ],
        );
    };
    dependency("01", "equals", "B", "serial", "require");
    dependency("02", "not-empty", "", "newQ", "show");
}

/// Now, as of mid 2020 and with drafts: which fields and options each form has, and the dependencies on its fields in
/// html (moqui-form-when.js data), qvt (a style and a bound required) and tui (the rules as data).
#[test]
fn a_dated_db_form_renders_as_of_a_date_with_its_dependencies() {
    let dir = std::env::temp_dir().join(format!("moqui-forms-dated-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("Form.xml").to_string_lossy().into_owned();
    std::fs::write(
        &root,
        r#"<screen require-authentication="false">
    <widgets>
        <form-single name="Now" extends="DbForm#CS_TPL"/>
        <section name="Preview"><actions><set field="previewing" from="true"/></actions><widgets>
            <form-single name="Preview" extends="DbForm#CS_TPL" dynamic="true" db-form-include-drafts="previewing"/>
        </widgets></section>
        <section name="Past"><actions><set field="pastDate" value="2020-06-01 00:00:00"/></actions><widgets>
            <form-single name="Past" extends="DbForm#CS_TPL" db-form-as-of="pastDate"/>
        </widgets></section>
    </widgets>
</screen>"#,
    )
    .unwrap();
    let world = World::open("");
    seed(&world);

    let page = serve_at(&world.factory, request("GET", "/"), &root);
    let html = page.body_text();
    assert_eq!(page.status, 200, "{html}");
    let form = |name: &str| -> String {
        let start = html
            .find(&format!("id=\"{name}\""))
            .unwrap_or_else(|| panic!("form {name}: {html}"));
        let end = html[start..].find("</form>").map_or(html.len(), |end| start + end);
        html[start..end].to_string()
    };
    let now = form("Now");
    assert!(
        now.contains("id=\"Now_newQ\"") && !now.contains("Now_oldQ"),
        "a successor in place: {now}"
    );
    assert!(
        !now.contains("Now_draftQ") && !now.contains(">Delta<") && !now.contains("Now_noType"),
        "no drafts now, no field with no type: {now}"
    );
    assert!(
        now.contains(">Bravo<") && !now.contains(">Alpha<"),
        "an ended option is out: {now}"
    );
    assert!(
        now.contains(r#"data-require-when="{&quot;form&quot;:&quot;Now&quot;,&quot;field&quot;:&quot;kind&quot;,&quot;operator&quot;:&quot;equals&quot;,&quot;values&quot;:[&quot;B&quot;]}""#),
        "{now}"
    );
    assert!(now.contains("&quot;operator&quot;:&quot;not-empty&quot;"), "{now}");
    let past = form("Past");
    assert!(
        past.contains("id=\"Past_oldQ\"") && !past.contains("Past_newQ"),
        "{past}"
    );
    assert!(past.contains(">Alpha<"), "{past}");
    let preview = form("Preview");
    assert!(
        preview.contains("id=\"Preview_draftQ\"") && preview.contains(">Delta<"),
        "{preview}"
    );
    assert!(
        preview.contains("id=\"Preview_newQ\"") && !preview.contains(">Alpha<"),
        "{preview}"
    );

    // A request parameter, by the context names a render once read, neither shows the drafts nor moves the date.
    let mut get = request("GET", "/");
    get.query.push(("dbFormIncludeDrafts".into(), "true".into()));
    get.query.push(("dbFormAsOfDate".into(), "2020-06-01 00:00:00".into()));
    get.query.push(("db-form-include-drafts".into(), "true".into()));
    let asked = serve_at(&world.factory, get, &root).body_text();
    let now_asked = {
        let start = asked.find("id=\"Now\"").unwrap_or_else(|| panic!("form Now: {asked}"));
        let end = asked[start..].find("</form>").map_or(asked.len(), |end| start + end);
        asked[start..end].to_string()
    };
    assert!(
        now_asked.contains("id=\"Now_newQ\"") && !now_asked.contains("Now_oldQ") && !now_asked.contains("Now_draftQ"),
        "the parameters are not the form's date: {now_asked}"
    );

    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "qvt".into()));
    let qvt = serve_at(&world.factory, get, &root).body_text();
    assert!(
        qvt.contains(r#":required="(Array.isArray(formProps.fields.kind)?formProps.fields.kind.map(String).includes('B'):'B'==formProps.fields.kind)""#),
        "{qvt}"
    );
    assert!(
        qvt.contains("{display:(!(formProps.fields.kind==null||formProps.fields.kind===''||(Array.isArray(formProps.fields.kind)&amp;&amp;!formProps.fields.kind.length))?'':'none')}")
            || qvt.contains("{display:(!(formProps.fields.kind==null||formProps.fields.kind===''||(Array.isArray(formProps.fields.kind)&&!formProps.fields.kind.length))?'':'none')}"),
        "{qvt}"
    );

    let mut get = request("GET", "/");
    get.query.push(("renderMode".into(), "tui".into()));
    let tui = serve_at(&world.factory, get, &root).body_text();
    assert!(tui.contains(r#""requireWhen":{"field":"kind","value":"B"}"#), "{tui}");
    assert!(
        tui.contains(r#""visibleWhen":{"field":"kind","operator":"not-empty","value":""}"#),
        "{tui}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
