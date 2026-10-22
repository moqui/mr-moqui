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

//! `moqui lint --screens --qvt-static` over the framework, tools and webroot screens: the compatibility report of
//! `plans/archive/QvtServerStatic.md`. The verdict of the Gate 3 fixture is checked in here. The report changes no default: nothing in the tree is
//! marked server-static for qvt because a report is clean.

use std::path::PathBuf;

use moqui_host::qvt_report::{qvt_static_reports, render_reports};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "slow: cargo test-slow"]
fn the_report_over_every_screen_and_the_verdict_of_the_gate_3_fixture() {
    let reports = qvt_static_reports(
        &repo().join("runtime"),
        &repo().join("runtime/base-component/framework"),
    )
    .unwrap();
    let text = render_reports(&reports);
    assert!(reports.len() > 100, "{} screens", reports.len());
    let find = |suffix: &str| {
        reports
            .iter()
            .find(|(location, _)| location.ends_with(suffix))
            .unwrap_or_else(|| panic!("no report for {suffix}"))
            .1
            .as_ref()
            .unwrap_or_else(|err| panic!("{suffix}: {err}"))
    };

    // The Gate 3 fixture: ArtifactGroupDetail has a form-single, form-lists with display-entity, entity-options and a section-iterate in a field,
    // dialogs, links with parameter maps, and actions that are only finds. Every expression it writes is in the browser host, and its always-actions
    // and pre-actions have no side effect, so the report is clean and the fixture can be marked server-static for qvt.
    let fixture = find("tools/screen/System/Security/ArtifactGroup/ArtifactGroupDetail.xml");
    // The lint command is the static half. Clean means the full check ran.
    assert!(!fixture.is_clean(), "{}", fixture.render());
    assert!(
        fixture.eligible
            && fixture.violations.is_empty()
            && !fixture.full_check
            && fixture.template_bytes > 5000
            && !fixture.marked,
        "{}",
        fixture.render()
    );
    assert_eq!(fixture.mode, "qvt");
    assert!(
        fixture.render().contains("verdict static half only"),
        "{}",
        fixture.render()
    );
    assert!(!fixture.render().contains("verdict clean"), "{}", fixture.render());

    // A screen that reads a server-only part of ec in a widget says so, with the screen, the element and the operation.
    let service = find("tools/screen/Tools/Service/ServiceDetail.xml");
    assert!(
        service
            .violations
            .iter()
            .any(|violation| violation.kind == "ec" && violation.detail.contains("ec.resource.getLocationText")),
        "{}",
        service.render()
    );
    assert!(
        service
            .violations
            .iter()
            .all(|violation| violation.screen.ends_with("ServiceDetail.xml")),
        "{}",
        service.render()
    );
    // Work moved into the actions is read from the package: UserAccountDetail reads its clock and permission there.
    let user = find("tools/screen/System/Security/UserAccount/UserAccountDetail.xml");
    assert!(
        user.violations.iter().all(|violation| violation.kind != "ec"),
        "{}",
        user.render()
    );
    // A dynamic form keeps a screen a server merge, and the report says it is not eligible.
    let auto = find("tools/screen/Tools/AutoScreen/AutoFind.xml");
    assert!(
        !auto.eligible && auto.reasons.iter().any(|reason| reason.contains("dynamic")),
        "{}",
        auto.render()
    );
    assert!(text.lines().last().unwrap_or("").contains("screens:"), "{text}");
    // Nothing is marked because a report is clean: the fixture screen file does not name server-static (the gate marks it with a screen-extend in its own runtime).
    let source = std::fs::read_to_string(
        repo().join("runtime/base-component/tools/screen/System/Security/ArtifactGroup/ArtifactGroupDetail.xml"),
    )
    .unwrap();
    assert!(!source.contains("server-static"));
}
