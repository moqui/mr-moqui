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

//! Host decimals in a screen: `/`, a truthy condition, and `<` call `valueOf` and throw. `toNumber` makes a
//! float for the progress width and the comparison, and is falsy for null and zero so an empty budget stays hidden.
//! `ProjectSummary.xml` uses these expressions.

use crate::context::ExecutionContextFactory;
use crate::screen::make_render;

const SCREEN: &str = r#"<screen require-authentication="false"><actions><script><![CDATA[
    project = { name: "Bridge", totalTimeAllowed: null, totalClientCostAllowed: toDecimal(0) }
    pstats = {
        estimatedCompletePercent: toDecimal("40"),
        estimatedRemainingPercent: toDecimal("10"),
        timeBudgetCompletePercent: null,
        clientBudgetUsedPercent: toDecimal(0),
        clientBudgetEstimateDiff: toDecimal("-2.5")
    }
    shown = { totalClientCostAllowed: toDecimal("12.50"), diff: toDecimal("-2.5"), under: toDecimal("1") }
]]></script></actions><widgets>
    <label text="Project ${project.name}"/>
    <section name="TimeBudgetGraphs" condition="toNumber(project.totalTimeAllowed)">
        <widgets><label text="TIME SHOWN"/></widgets>
    </section>
    <section name="CostBudgetGraphs" condition="toNumber(project.totalClientCostAllowed)">
        <widgets><label text="COST SHOWN"/></widgets>
    </section>
    <section name="ShownCost" condition="toNumber(shown.totalClientCostAllowed)">
        <widgets>
            <label text="${toNumber(shown.diff) &lt; 0 ? 'over' : 'under'}"/>
            <label text="${toNumber(shown.under) &lt; 0 ? 'over' : 'under'}"/>
        </widgets>
    </section>
    <render-mode><text type="qvt" template="true"><![CDATA[
        <q-linear-progress :value="${toNumber(pstats.estimatedCompletePercent)/100}" :buffer="${toNumber(pstats.estimatedRemainingPercent)/100}"/>
        <q-linear-progress :value="${toNumber(pstats.timeBudgetCompletePercent)/100}"/>
        <q-linear-progress :value="${toNumber(pstats.clientBudgetUsedPercent)/100}"/>
    ]]></text></render-mode>
</widgets></screen>"#;

#[test]
fn a_screen_of_host_decimals_renders_without_value_of() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let dir = std::env::temp_dir().join(format!("moqui-decimal-screen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let screen = dir.join("Decimal.xml");
    std::fs::write(&screen, SCREEN).unwrap();
    let output = make_render(context.factory())
        .root_screen(screen.to_string_lossy().into_owned())
        .render_mode("qvt")
        .render()
        .unwrap_or_else(|err| panic!("{err}"));
    let body = output.body_text();
    assert_eq!(output.status, 200, "{body}");
    assert!(body.contains("Bridge"), "{body}");
    assert!(body.contains(":value=\"0.4\""), "{body}");
    assert!(body.contains(":buffer=\"0.1\""), "{body}");
    assert!(body.contains(":value=\"0\""), "{body}");
    assert!(body.contains("over"), "{body}");
    assert!(body.contains("under"), "{body}");
    assert!(!body.contains("TIME SHOWN") && !body.contains("COST SHOWN"), "{body}");
    assert!(!body.contains("valueOf"), "{body}");
    assert!(
        context.message().errors().is_empty(),
        "{:?}",
        context.message().errors()
    );
    let summary = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/component/MrSimpleScreens/screen/SimpleScreens/Project/ProjectSummary.xml"),
    )
    .unwrap();
    assert!(
        !summary.contains("Percent/100"),
        "a percent is still divided as a decimal"
    );
    assert!(
        !summary.contains("condition=\"project.total"),
        "a budget condition is still the decimal"
    );
    assert!(summary.contains("toNumber(pstats.clientBudgetEstimateDiff)"));
    let _ = std::fs::remove_dir_all(&dir);
}
