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

//! The `scanResolve` transition of a screen with a `scan-input` (`scan.rs`), and a screen's `density` in the menu data and
//! the html body.

use super::forms::{cookies, form_post, token_of, with_cookies};
use super::{basic, request, World};
use crate::screen::serve_at;

/// The resolver: an asset and a lot both named DUP, one asset A1, a location L1 that is not an allowed type of the field,
/// and the context it was given echoed in the detail. It answers no parsed fields, so the framework adds the GS1 parse.
const SERVICES: &str = r#"<services>
    <service verb="resolve" noun="Scan" authenticate="false">
        <in-parameters><parameter name="scanText"/><parameter name="allowedTypes" type="List"/><parameter name="scanId"/>
            <parameter name="facilityId"/><parameter name="locationId"/><parameter name="secret"/></in-parameters>
        <out-parameters><parameter name="matches" type="List"/><parameter name="parsed" type="Map"/></out-parameters>
        <actions><script><![CDATA[
            const where = 'at ' + (facilityId || '-') + '/' + (locationId || '-') + ' types ' + (allowedTypes || []).join('+') +
                ' scan ' + (scanId || '-') + (secret ? ' secret ' + secret : '')
            if (scanText === 'FAIL') ec.message.addError('Scanner says no')
            else if (scanText === 'DUP') matches = [{identityType: 'asset', id: 'A9', label: 'Asset A9', detail: where},
                {identityType: 'lot', id: 'LOT9', label: 'Lot 9'}, {identityType: 'location', id: 'L9'}, {identityType: 'asset'}]
            else if (scanText === 'A1' || scanText.indexOf('(01)') === 0) matches = [{identityType: 'asset', id: 'A1', label: 'Asset A1', detail: where}]
            else if (scanText === 'L1') matches = [{identityType: 'location', id: 'L1'}]
            else matches = []
        ]]></script></actions>
    </service>
</services>"#;

const SCREEN: &str = r#"<screen require-authentication="false" density="handheld">
    <transition name="move"><actions><message>Moved ${assetId} ${assetId_type} ${scanId}</message></actions>
        <default-response type="none"/></transition>
    <widgets>
        <form-single name="Move" transition="move">
            <field name="locationId"><default-field><text-line/></default-field></field>
            <field name="assetId"><default-field>
                <scan-input resolver="resolve#Scan" allowed-types="asset,lot">
                    <parameter name="facilityId" value="WH1"/><depends-on field="locationId"/>
                </scan-input>
            </default-field></field>
            <field name="plain"><default-field><text-line/></default-field></field>
        </form-single>
        <!-- two scans posting their scan id as the same name: refused -->
        <form-single name="Twice" transition="move">
            <field name="fromId"><default-field><scan-input resolver="resolve#Scan"/></default-field></field>
            <field name="toId"><default-field><scan-input resolver="resolve#Scan"/></default-field></field>
        </form-single>
    </widgets>
</screen>"#;

struct Site {
    dir: std::path::PathBuf,
    root: String,
    world: World,
}

impl Site {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("test")).unwrap();
        let services = dir.join("test/ScanServices.xml");
        std::fs::write(&services, SERVICES).unwrap();
        std::fs::write(dir.join("Scan.xml"), SCREEN).unwrap();
        let world = World::open("");
        {
            let ctx = world.factory.new_context();
            let _entered = ctx.enter();
            let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
            assert!(failures.is_empty(), "{failures:?}");
        }
        let root = dir.join("Scan.xml").to_string_lossy().into_owned();
        Self { dir, root, world }
    }

    fn scan(&self, body: &str) -> (u16, serde_json::Value) {
        let response = serve_at(&self.world.factory, form_post("/scanResolve", body), &self.root);
        let text = response.body_text();
        let json = serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"));
        (response.status, json)
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn the_scan_resolver_runs_for_the_named_field_with_its_types_and_context() {
    let site = Site::new("resolve");
    let base = "scanForm=Move&scanField=assetId";

    // one match, with the declared context (the parameter and the depends-on field) and the scan id; secret is not declared
    let (status, answer) = site.scan(&format!(
        "{base}&scanText=A1&scanId=S-1&facilityId=WH1&locationId=B-07&secret=x"
    ));
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["scanId"], "S-1");
    assert_eq!(answer["scanText"], "A1");
    let matches = answer["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1, "{answer}");
    assert_eq!(matches[0]["identityType"], "asset");
    assert_eq!(matches[0]["id"], "A1");
    assert_eq!(matches[0]["detail"], "at WH1/B-07 types asset+lot scan S-1", "{answer}");
    assert_eq!(answer["options"][0]["value"], "A1");
    assert_eq!(
        answer["options"][0]["label"],
        "Asset A1 · at WH1/B-07 types asset+lot scan S-1 (asset)"
    );
    assert!(answer["parsed"].is_null(), "not a GS1 code: {answer}");

    // several: the location is not an allowed type and the one with no id is not a match; a match with no label shows its id
    let (_, answer) = site.scan(&format!("{base}&scanText=DUP"));
    let ids: Vec<&str> = answer["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["A9", "LOT9"], "{answer}");
    assert_eq!(answer["matches"][1]["detail"], "");

    // none: an allowed-type filter can leave nothing
    let (status, answer) = site.scan(&format!("{base}&scanText=L1"));
    assert_eq!(status, 200);
    assert_eq!(answer["matches"], serde_json::json!([]));

    // the GS1 fields of the text, which the resolver did not answer; trailing line ends of a scanner are not the text
    let (_, answer) = site.scan(&format!("{base}&scanText=%2801%2904006381333931%2810%29LOT-7%0D"));
    assert_eq!(answer["scanText"], "(01)04006381333931(10)LOT-7");
    assert_eq!(answer["parsed"]["applicationIdentifiers"]["10"], "LOT-7", "{answer}");
    assert_eq!(answer["parsed"]["applicationIdentifiers"]["01"], "04006381333931");

    // a drop-down client (tui) searches with term
    let (_, answer) = site.scan(&format!("{base}&term=A1"));
    assert_eq!(answer["options"][0]["value"], "A1", "{answer}");

    // the resolver's error
    let (status, answer) = site.scan(&format!("{base}&scanText=FAIL"));
    assert_eq!(status, 400, "{answer}");
    assert!(
        answer["errors"].as_str().unwrap().contains("Scanner says no"),
        "{answer}"
    );

    // no text, no field, a field that is not a scan-input, a form that is not there
    let (status, _) = site.scan(base);
    assert_eq!(status, 400);
    let (status, _) = site.scan("scanText=A1");
    assert_eq!(status, 400);
    let (status, answer) = site.scan("scanForm=Move&scanField=plain&scanText=A1");
    assert_eq!(status, 404, "{answer}");
    let (status, _) = site.scan("scanForm=Nope&scanField=assetId&scanText=A1");
    assert_eq!(status, 404);
    let (status, answer) = site.scan("scanForm=Twice&scanField=fromId&scanText=A1");
    assert_eq!(status, 400, "{answer}");
    assert!(answer["errors"].as_str().unwrap().contains("fromId, toId"), "{answer}");

    // a GET is the read it is (the terminal client searches with one), and needs no token
    let mut get = request("GET", "/scanResolve");
    get.query = vec![
        ("scanForm".into(), "Move".into()),
        ("scanField".into(), "assetId".into()),
        ("term".into(), "A1".into()),
    ];
    let response = serve_at(&site.world.factory, get, &site.root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(
        response.body_text().contains(r#""value" : "A1""#),
        "{}",
        response.body_text()
    );

    // a POST of a session that has its token must carry it, as the page's own posts do
    let mut jar = Vec::new();
    let page = serve_at(&site.world.factory, request("GET", "/"), &site.root);
    cookies(&page, &mut jar);
    let token = token_of(&page.body_text());
    let post = |body: &str| {
        serve_at(
            &site.world.factory,
            with_cookies(form_post("/scanResolve", body), &jar),
            &site.root,
        )
    };
    let refused = post(&format!("{base}&scanText=A1"));
    assert!(refused.status >= 400, "{} {}", refused.status, refused.body_text());
    let taken = post(&format!("{base}&scanText=A1&moquiSessionToken={token}"));
    assert_eq!(taken.status, 200, "{}", taken.body_text());
}

/// `density="handheld"` reaches the shell in the menu data entry of the screen, and the html page as a body class.
#[test]
fn a_handheld_screen_says_so_in_its_menu_data_and_its_html_body() {
    let dir = std::env::temp_dir().join(format!("moqui-density-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/App")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false">
    <transition name="menuData" read-only="true"><actions><script><![CDATA[
        const menuDataList = sri.getMenuData(sri.getScreenUrlInfo().extraPathNameList)
        if (menuDataList != null) ec.web.sendJsonResponse(menuDataList)
    ]]></script></actions><default-response type="none"/></transition>
    <subscreens default-item="App"/>
    <widgets><label text="density=${sri.getScreenUrlInfo().density}"/><subscreens-active/></widgets>
</screen>"#,
    );
    write(
        "Root/App.xml",
        r#"<screen require-authentication="true" default-menu-title="App">
            <subscreens default-item="Desk">
                <subscreens-item name="Desk" location="{dir}/Root/App/Desk.xml"/>
                <subscreens-item name="Hand" location="{dir}/Root/App/Hand.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/App/Desk.xml",
        r#"<screen require-authentication="true"><widgets><label text="desk"/></widgets></screen>"#,
    );
    write(
        "Root/App/Hand.xml",
        r#"<screen require-authentication="true" density="handheld"><widgets><label text="hand"/></widgets></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    world.user("ada", None);
    let get = |path: &str| {
        let mut get = request("GET", path);
        get.set_header("authorization", basic("ada"));
        serve_at(&world.factory, get, &root).body_text()
    };
    let menu: serde_json::Value = serde_json::from_str(&get("/menuData/App/Hand")).unwrap();
    assert_eq!(menu[1]["name"], "Hand", "{menu}");
    assert_eq!(menu[1]["density"], "handheld", "{menu}");
    assert!(menu[0].get("density").is_none(), "{menu}");
    let desk: serde_json::Value = serde_json::from_str(&get("/menuData/App/Desk")).unwrap();
    assert!(desk[1].get("density").is_none(), "{desk}");
    // the density of the deepest screen on the path that sets one, for the html body class
    assert!(get("/App/Hand").contains("density=handheld"));
    assert!(get("/App/Desk").contains("density="));
    assert!(!get("/App/Desk").contains("density=handheld"));
    let _ = std::fs::remove_dir_all(&dir);
}
