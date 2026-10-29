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

use moqui_lib::object::Value;

use crate::context::ExecutionContextFactory;
use crate::script::{clear_context_stack, eval_condition, set_context_name};

fn holds(expression: &str) {
    assert!(eval_condition(expression).unwrap(), "{expression}");
}

#[test]
fn js_safe_escapes_backslash_first_and_the_line_separators() {
    assert_eq!(super::encode_html_js_safe(r"a\'b"), r"a\\\'b");
    assert_eq!(super::encode_html_js_safe("\\"), "\\\\");
    assert_eq!(super::encode_html_js_safe("x\u{2028}y\u{2029}z"), r"x\u2028y\u2029z");
    // The escaped text, put in a single-quoted literal, reads back as the original.
    clear_context_stack();
    holds(r#"eval("'" + WebUtilities.encodeHtmlJsSafe("a\\'b\u2028c\\") + "'") === "a\\'b\u2028c\\""#);
    holds(r#"WebUtilities.fieldValuesEncodeHtmlJsSafe({"k\\": "v\\'\u2029"}) === "{'k\\\\':'v\\\\\\'\\u2029'}""#);
}

#[test]
fn web_string_and_collection_helpers() {
    clear_context_stack();
    holds(r#"WebUtilities.encodeHtmlJsSafe("a'b\"<\n") === "a\\'b&quot;&lt;\\n""#);
    holds(
        r#"WebUtilities.fieldValuesEncodeHtmlJsSafe({a: "x'y", b: [1, true, null]}) === "{'a':'x\\'y','b':[1,true,null]}""#,
    );
    holds(r#"WebUtilities.encodeHtml("<&>") === "&lt;&amp;&gt;""#);
    holds(
        r#"ec.l10n.format("1791979200000", "yyyy") === "2026" && ec.l10n.format(1791979200000 - 1, "yyyy") === "2026""#,
    );
}

/// Every `SimpleDateFormat` pattern formats a time stamp, epoch millis or millis text, not only those with `y`,
/// `d`, `H` or `E` (`h:mm a` gave the millis back); a number pattern still formats a number.
#[test]
fn a_date_pattern_formats_a_time_stamp_whatever_its_letters() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    // 2024-03-05 14:07:09 UTC, a Tuesday; the test session is in UTC.
    set_context_name("millis", Value::I64(1_709_647_629_000)).unwrap();
    let stamp = jiff::Timestamp::from_millisecond(1_709_647_629_000).unwrap();
    set_context_name("stamp", Value::Timestamp(stamp)).unwrap();
    let table = [
        ("h:mm a", "2:07 PM"),
        ("hh:mm a", "02:07 PM"),
        ("HH:mm", "14:07"),
        ("EEE h:mm a", "Tue 2:07 PM"),
        ("K:mm a", "2:07 PM"),
        ("k:mm", "14:07"),
        ("mm:ss", "07:09"),
        ("a", "PM"),
        ("MMM d", "Mar 5"),
        ("EEEE", "Tuesday"),
        ("yyyy-MM-dd'T'HH:mm:ss", "2024-03-05T14:07:09"),
        ("D", "65"),
        ("w", "10"),
        ("W", "2"),
        ("F", "1"),
        ("YYYY", "2024"),
        ("G", "AD"),
        ("z", "UTC"),
        ("HH:mm:ss.SSS", "14:07:09.000"),
    ];
    for (pattern, expect) in table {
        for value in ["millis", "String(millis)", "stamp"] {
            holds(&format!(r#"ec.l10n.format({value}, "{pattern}") === "{expect}""#));
        }
    }
    holds(r##"ec.l10n.format(1234.5, "#,##0.00") === "1,234.50" && ec.l10n.format(42, "0") === "42""##);
    // No pattern: epoch millis text (a time stamp field as a script sees it) is a time stamp, other text itself.
    holds(
        r#"ec.l10n.format(String(millis), null) === "2024-03-05 14:07" && ec.l10n.format(stamp, "") === "2024-03-05 14:07""#,
    );
    holds(r#"ec.l10n.format(stamp) === "2024-03-05 14:07" && ec.l10n.format(millis) === "1,709,647,629,000""#);
    holds(r#"ec.l10n.format("100050", null) === "100050" && ec.l10n.format("CT-REQ", null) === "CT-REQ""#);
    clear_context_stack();
}

#[test]
fn encode_html_is_null_safe() {
    clear_context_stack();
    holds(r#"WebUtilities.encodeHtml(null) === "" && WebUtilities.encodeHtml(undefined) === """#);
    holds(r#"WebUtilities.encodeHtml() === "" && WebUtilities.encodeHtml(missingName) === """#);
    holds(r#"WebUtilities.encodeHtml(12) === "12" && WebUtilities.encodeHtml(true) === "true""#);
    holds(r#"WebUtilities.encodeHtmlJsSafe(null) === "" && WebUtilities.encodeHtmlJsSafe(undefined) === """#);
    holds(r#"WebUtilities.encodeHtmlJsSafe() === "" && WebUtilities.encodeHtmlJsSafe(3.5) === "3.5""#);
    holds(r#"WebUtilities.encodeHtmlJsSafe(false) === "false" && WebUtilities.encodeHtmlJsSafe("<'") === "&lt;\\'""#);
    holds(r##"ec.l10n.format(1234.5, "#,##0.00") === "1,234.50""##);
    holds(
        r#"toDecimal("2.345").setScale(2, "HALF_UP").toString() === "2.35" && toDecimal("2.5").setScale(0, 6).toString() === "2""#,
    );
    holds(r#"toDecimal("2").setScale(2).toString() === "2.00""#);
    // get#OrderItemTotal's quantity - 0.0 is 1.0, and the audit log records that text
    holds(r#"toDecimal(1).subtract(toDecimal('0.0')).toString() === "1.0""#);
    holds(r#"toDecimal('0.0').toString() === "0.0" && toDecimal(1).multiply('1.0').toString() === "1.0""#);
    holds(
        r#"toNumber(toDecimal("2.5")) * 2 === 5 && toNumber("3") === 3 && toNumber(null) === null && toNumber(4) === 4"#,
    );
    holds(r#"elasticQueryAutoWildcard(" john  smith ", true) === "*:john* *:smith*""#);
    holds(r#"elasticQueryAutoWildcard("pump AND name:x 10.5 a-b", false) === "pump* AND name:x 10.5 a-b""#);
    holds(r#"elasticQueryAutoWildcard("  ", true) === "*" && elasticQueryAutoWildcard(null, false) === "*""#);
    holds(r#"WebUtilities.ipMatches("10.1.*.*,192.168.0.1-5", "10.1.2.3")"#);
    holds(r#"WebUtilities.ipMatches("10.1.*.*,192.168.0.1-5", "192.168.0.4")"#);
    holds(r#"!WebUtilities.ipMatches("10.1.*.*,192.168.0.1-5", "192.168.0.9")"#);
    holds(r#"WebUtilities.ipMatches("10.0.0.1", "::ffff:10.0.0.1") && WebUtilities.ipMatches("::1", "::1")"#);
    holds(r#"WebUtilities.ipMatches("", "1.2.3.4") && !WebUtilities.ipMatches("1.2.3.4", "")"#);
    holds(r#"WebUtilities.isSafeSinglePathSegment("a.zip") && !WebUtilities.isSafeSinglePathSegment("../x")"#);
    holds(r#"!WebUtilities.isSafeSinglePathSegment("a/b") && !WebUtilities.isSafeSinglePathSegment("")"#);
    holds(r#"WebUtilities.isSameOriginRedirect("/x") && !WebUtilities.isSameOriginRedirect("//evil/x")"#);
    holds(
        r#"!WebUtilities.isSameOriginRedirect("javascript:alert(1)") && !WebUtilities.isSameOriginRedirect("http://other/x")"#,
    );
    holds(r#"WebUtilities.isSameOriginRedirect("https://app.example/x", "app.example")"#);
    holds(r#"!WebUtilities.isSameOriginRedirect("https://user@app.example/x", "app.example")"#);
    holds(r#"WebUtilities.lastPathSegment("c:\\dir\\file.zip") === "file.zip""#);
    holds(r#"StringUtilities.encodeForXmlAttribute("a<b\"\u00e9") === "a&lt;b&quot;&#233;""#);
    holds(r#"StringUtilities.base64UrlDecode(StringUtilities.base64UrlEncode("hello/world?>")) === "hello/world?>""#);
    holds(r#"StringUtilities.base64UrlEncode("??>>") === "Pz8-Pg""#);
    // base64 of text (its UTF-8) and of bytes as they are, and back to bytes: byte-safe for a file or a MIME part
    holds(r#"StringUtilities.base64Encode("é") === "w6k=" && StringUtilities.base64Encode(null) === """#);
    holds(r#"StringUtilities.base64Encode(new Uint8Array([0, 255, 128, 10])) === "AP+ACg==""#);
    holds(r#"StringUtilities.base64Encode(new Uint8Array([1, 2, 3]).buffer) === "AQID""#);
    holds(
        r#"(function () {
                const bytes = StringUtilities.base64Decode("AP+A\r\nCg");
                return bytes instanceof Uint8Array && bytes.length === 4 && bytes[0] === 0 && bytes[1] === 255 && bytes[2] === 128 && bytes[3] === 10;
            })()"#,
    );
    holds(r#"StringUtilities.base64Decode(StringUtilities.base64Encode("AP+ACg==")).length === 8"#);
    holds(
        r#"(function () { try { StringUtilities.base64Decode("not base64!"); return false; } catch (err) { return /not base64/.test(String(err)); } })()"#,
    );
    holds(r#"CollectionUtilities.getMapArrayListValues([{a: 1}, {a: null}, {b: 2}], "a", true).length === 1"#);
    holds(r#"CollectionUtilities.getMapArrayListValues([{a: 1}, {a: null}, {b: 2}], "a", false).length === 3"#);
    holds(
        r#"(function () {
                const page = {pageIndex: 1, pageSize: 10};
                CollectionUtilities.paginateParameters(25, "rows", page);
                return page.rowsCount === 25 && page.rowsPageRangeLow === 11 && page.rowsPageRangeHigh === 20 && page.rowsPageMaxIndex === 2;
            })()"#,
    );
}

#[test]
fn resource_l10n_context_and_user_helpers() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    set_context_name("who", Value::string("Ada")).unwrap();
    set_context_name("shared", Value::string("outer")).unwrap();
    holds(r#"ec.resource.expand("Hi ${who}") === "Hi Ada""#);
    holds(r#"ec.resource.expand("Hi ${who} ${other}", "", {other: "Bob", who: "Eve"}) === "Hi Eve Bob""#);
    holds(r#"ec.resource.expand("Hi ${who}") === "Hi Ada""#);
    // a text too long to be a LocalizedMessage key comes back as it is
    holds(r#"ec.l10n.localize("a".repeat(300)) === "a".repeat(300)"#);
    holds(
        r#"ec.resource.expandNoL10n("${shared}", "", {shared: "inner"}) === "inner" && ec.resource.expandNoL10n("${shared}") === "outer""#,
    );
    holds(r#"ec.resource.expression("who.length + 1") === 4 && ec.resource.expression("n * 2", "", {n: 21}) === 42"#);
    holds(r#"ec.resource.expression("{a: 1}").a === 1"#);
    holds(
        r#"ec.resource.condition("who == 'Ada'") && !ec.resource.condition("who == 'Bob'") && !ec.resource.condition("")"#,
    );
    holds(r#"ec.l10n.localize("Plain text") === "Plain text""#);
    holds(
        r##"ec.l10n.format(1234.5, "#,##0.00") === "1,234.50" && ec.l10n.format("text", "0") === "text" && ec.l10n.format(null) === """##,
    );
    holds(r#"ec.l10n.formatCurrency(1234.5, "USD") === "$1,234.50" && ec.l10n.formatCurrency(5, "JPY") === "\u00a55""#);
    holds(
        r#"ec.l10n.formatCurrencyNoSymbol(1234.5, "USD") === "1,234.50" && ec.l10n.formatCurrency("", "USD") === """#,
    );
    holds(r#"ec.l10n.formatCurrency(2, "USD", 3) === "$2.000""#);
    holds(r#"ec.l10n.parseTimestamp("2024-03-05 14:07") !== null && ec.l10n.parseTimestamp("not a date") === null"#);
    holds(r#"ec.user.locale === "en_US" && ec.user.getLocale() === "en_US" && ec.user.nowTimestamp.length > 8"#);
    holds(
        r#"ec.context.get("who") === "Ada" && ec.context.get("nothing") === null && ec.context.formListFindId === null"#,
    );
    holds(
        r#"ec.screen.isRenderModeValid("csv") && ec.screen.isRenderModeValid("xsl-fo") && !ec.screen.isRenderModeValid("vuet")"#,
    );
    // With no request a full URL is needed, the conf webapp's, and the test conf has no webapp.
    holds(r#"ec.web.getWebappRootUrl("webroot", "", true, true) === "" && ec.web.getWebappRootUrl() === """#);
    clear_context_stack();
}

#[test]
fn an_order_by_with_commas_orders_by_each_field() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let _entered = context.enter();
    holds(r#"ec.entity.find("moqui.basic.EnumerationType").orderBy("-enumTypeId,^description").list().length === 0"#);
}

#[test]
fn flatten_nested_map_mask_and_year_list_as_java() {
    clear_context_stack();
    // Nested maps and the maps in a nested list put their entries in place; a later key replaces an earlier one.
    holds(
        r#"JSON.stringify(CollectionUtilities.flattenNestedMap({a: 1, info: {b: 2, a: 3}, rows: [{c: 4}, 5], d: null}))
                === '{"a":3,"b":2,"c":4,"d":null}'"#,
    );
    holds("CollectionUtilities.flattenNestedMap(null) === null");
    // The GL account code mask: the value holds no literals, and a short value is filled with the placeholder.
    holds("StringUtilities.masker('###-###-###', '0').valueToString('110000000') === '110-000-000'");
    holds("StringUtilities.masker('###-###-###', '0').valueToString('1200') === '120-000-000'");
    holds("StringUtilities.masker(\"UU-'#A\", null).valueToString('ab7') === 'AB-#7'");
    holds("StringUtilities.masker('', '0') === null && StringUtilities.masker(null) === null");
    holds("(() => { try { StringUtilities.masker('###', '0').valueToString('1x'); return false } catch (e) { return true } })()");
    // The formatter keeps its method when it is a context value, as a screen's `<set>` makes it.
    holds("(accountCodeFormatter = StringUtilities.masker('##-##', '0'), accountCodeFormatter.valueToString(12) === '12-00')");
    holds(
        "StringUtilities.getYearList(3).length === 3 && StringUtilities.getYearList(3)[0] === String(new Date().getFullYear())",
    );
    clear_context_stack();
}

#[test]
fn barcode_and_gs1_resource_helpers_preserve_validated_values() {
    clear_context_stack();
    holds(r#"ec.resource.barcodeSvg('EAN-13','4006381333931').startsWith('<svg')"#);
    holds(r#"ec.resource.barcodeSvg('QR','INV-123').includes('aria-label="INV-123"')"#);
    holds(r#"ec.resource.isValidGs1CheckDigit('4006381333931') && !ec.resource.isValidGs1CheckDigit('4006381333930')"#);
    holds(r#"ec.resource.parseGs1('(01)04006381333931(10)LOT').applicationIdentifiers['10'] === 'LOT'"#);
    holds(
        r#"(()=>{try {ec.resource.barcodeSvg('EAN-13','4006381333930');return false} catch(e){return String(e).includes('check digit')}})()"#,
    );
}
