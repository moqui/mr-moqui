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

//! Saved finds and the user's columns of a form-list through the implicit `formSaveFind` and `formSelectColumns`
//! transitions, and the `screen.user.page.size` preference.

use moqui_lib::object::Value;

use super::forms::{cookies, form_post, token_of, with_cookies};
use super::{basic, request, World};
use crate::entity::EntityValue;
use crate::screen::serve_at;
use crate::web::WebRequest;

const SCREEN: &str = r#"<screen require-authentication="false">
    <actions>
        <script>rows = [{a: 'A1', b: 'B1', c: 'C1'}]</script>
    </actions>
    <widgets>
        <label text="a=${a || ''} size=${pageSize || ''}"/>
        <form-list name="Rows" list="rows" select-columns="true" saved-finds="true">
            <field name="a"><header-field title="Col A"><text-find/></header-field><default-field><display/></default-field></field>
            <field name="b"><header-field title="Col B"/><default-field><display/></default-field></field>
            <field name="c"><header-field title="Col C"/><default-field><display/></default-field></field>
        </form-list>
    </widgets>
</screen>"#;

struct Site {
    dir: std::path::PathBuf,
    root: String,
    world: World,
}

impl Site {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-saved-finds-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Find.xml"), SCREEN).unwrap();
        let world = World::open("");
        world.user("ada", None);
        let root = dir.join("Find.xml").to_string_lossy().into_owned();
        Self { dir, root, world }
    }

    fn send(&self, request: WebRequest) -> (u16, String, String) {
        self.send_as("ada", request)
    }

    fn send_as(&self, username: &str, mut request: WebRequest) -> (u16, String, String) {
        request.set_header("authorization", basic(username));
        let response = serve_at(&self.world.factory, request, &self.root);
        let location = response.header("location").unwrap_or_default().to_string();
        (response.status, location, response.body_text())
    }

    fn get(&self, query: &[(&str, &str)]) -> String {
        let mut get = request("GET", "/");
        get.query = query
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let (status, _, body) = self.send(get);
        assert_eq!(status, 200, "{body}");
        body
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.post_as("ada", path, body)
    }

    fn post_as(&self, username: &str, path: &str, body: &str) -> (u16, String) {
        let (status, location, text) = self.send_as(username, form_post(path, body));
        assert_eq!(status, 302, "{text}");
        (status, location)
    }

    /// The rows of `entity` with this field value.
    fn rows(&self, entity: &str, field: &str, value: &str) -> Vec<EntityValue> {
        self.world
            .facade
            .find(entity)
            .unwrap()
            .eq(field, Value::string(value))
            .list()
            .unwrap()
            .into_values()
    }

    /// Ada saves a find of `a=foo` with this description: its ID.
    fn save_find(&self, description: &str) -> String {
        let (_, location) = self.post(
            "/formSaveFind",
            &format!("formLocation=Rows&a=foo&_findDescription={}", encode(description)),
        );
        let id = location
            .split("formListFindId=")
            .nth(1)
            .map(|rest| rest.split('&').next().unwrap_or("").to_string())
            .unwrap_or_else(|| panic!("no saved find in {location}"));
        assert!(!id.is_empty());
        id
    }

    fn schedule_body(id: &str) -> String {
        format!(
            "formListFindId={id}&ScheduleFind=Y&screenPath=/Find&cronSelected={}",
            encode("0 0 6 * * ?")
        )
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The header titles of the form-list in the order the page shows them.
fn titles(html: &str) -> Vec<&'static str> {
    let html = &html[html.find("<table").unwrap_or(0)..];
    let mut found: Vec<(usize, &'static str)> = ["Col A", "Col B", "Col C"]
        .into_iter()
        .filter_map(|title| html.find(&format!(">{title}<")).map(|at| (at, title)))
        .collect();
    found.sort();
    found.into_iter().map(|(_, title)| title).collect()
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[test]
fn the_column_dialog_saves_the_users_columns_and_reset_restores_the_form() {
    let site = Site::new("columns");
    assert_eq!(titles(&site.get(&[])), vec!["Col A", "Col B", "Col C"]);
    let tree = r#"[{"id":"hidden","order":0,"children":[{"id":"b","order":0}]},
        {"id":"column_0","order":1,"children":[{"id":"c","order":0}]},{"id":"column_1","order":2,"children":[{"id":"a","order":0}]}]"#;
    let (_, location) = site.post(
        "/formSelectColumns",
        &format!("formLocation=Rows&SaveColumns=Save&columnsTree={}", encode(tree)),
    );
    assert!(!location.contains("columnsTree"), "{location}");
    let html = site.get(&[]);
    assert_eq!(titles(&html), vec!["Col C", "Col A"], "{html}");
    assert!(html.contains(">C1<") && !html.contains(">B1<"), "{html}");
    site.post("/formSelectColumns", "formLocation=Rows&ResetColumns=Reset");
    assert_eq!(titles(&site.get(&[])), vec!["Col A", "Col B", "Col C"]);
}

#[test]
fn a_saved_find_keeps_its_parameters_and_can_be_the_default_then_deleted() {
    let site = Site::new("finds");
    let id = site.save_find("Mine");
    // The find dialog lists it, in html and in qvt (whose template the Vue compiler takes).
    let html = site.get(&[("a", "foo")]);
    assert!(
        html.contains("<ul class=\"saved-finds\">") && html.contains(">Mine</a>"),
        "{html}"
    );
    assert!(
        html.contains("name=\"DeleteFind\"") && html.contains("name=\"MakeDefault\""),
        "{html}"
    );
    let qvt = site.get(&[("renderMode", "qvt"), ("a", "foo")]);
    assert!(qvt.contains("Mine") && qvt.contains("name=\"DeleteFind\""), "{qvt}");
    super::qvt::assert_compiles(&["saved finds".to_string()], &[qvt]);
    // The saved find's parameters apply when the request has none of its own; a parameter of the request wins.
    assert!(site.get(&[("formListFindId", &id)]).contains("a=foo"));
    assert!(site.get(&[("formListFindId", &id), ("a", "bar")]).contains("a=bar"));
    assert!(!site.get(&[]).contains("a=foo"));
    // A render with no request names it as render#ScheduledScreen does, and its parameters apply.
    {
        let context = site.world.factory.new_context();
        let _entered = context.enter();
        crate::script::set_context_name("root", Value::string(site.root.clone())).unwrap();
        crate::script::set_context_name("findId", Value::string(id.clone())).unwrap();
        let text = crate::script::eval_value(
            "ec.screen.makeRender().rootScreen(root).screenPath('').renderMode('text').parameters({formListFindId: findId}).render()",
        )
        .unwrap();
        let text = moqui_lib::object::to_plain_string(&text);
        assert!(text.contains("a=foo"), "{text}");
    }
    // Made the default for the screen, it applies with no formListFindId; _clear turns it off for a request.
    site.post(
        "/formSaveFind",
        &format!("formLocation=Rows&formListFindId={id}&MakeDefault=Y"),
    );
    assert!(site.get(&[]).contains("a=foo"));
    assert!(!site.get(&[("formListFindId", "_clear")]).contains("a=foo"));
    site.post(
        "/formSaveFind",
        &format!("formLocation=Rows&formListFindId={id}&DeleteFind=Y"),
    );
    assert!(!site.get(&[]).contains("a=foo"));
    assert!(!site.get(&[("formListFindId", &id)]).contains("a=foo"));
}

#[test]
fn a_page_size_is_kept_as_the_users_preference() {
    let site = Site::new("page-size");
    assert!(site.get(&[]).contains("size=<"), "no size yet");
    site.get(&[("pageSize", "7")]);
    assert!(site.get(&[]).contains("size=7"));
}

#[test]
fn a_fenced_render_takes_the_page_size_but_does_not_keep_it() {
    let site = Site::new("page-size-fenced");
    {
        let fence = crate::entity::fence::enter("an agent's render");
        assert!(site.get(&[("pageSize", "9")]).contains("size=9"), "the request's size applies");
        let tried: Vec<String> = fence.violations().iter().map(crate::entity::fence::Violation::describe).collect();
        assert!(tried.is_empty(), "no write is tried: {tried:?}");
    }
    assert!(site.get(&[]).contains("size=<"), "the user's preference is not changed");
    assert!(site.rows("moqui.security.UserPreference", "preferenceKey", "screen.user.page.size").is_empty());
}

#[test]
fn a_template_expression_in_a_description_stays_text_in_the_scheduled_email_subject() {
    let site = Site::new("schedule-subject");
    let description = r"Report ${6 * 7} \${6 * 7} \x";
    let id = site.save_find(description);
    site.post("/formSaveFind", &Site::schedule_body(&id));
    let scheduled = site.rows("moqui.screen.ScreenScheduled", "formListFindId", &id);
    assert_eq!(scheduled.len(), 1);
    let subject = moqui_lib::object::to_plain_string(scheduled[0].get("emailSubject").unwrap());
    // render#ScheduledScreen expands the subject: the description is the text it was, and the timestamp is expanded.
    let context = site.world.factory.new_context();
    let _entered = context.enter();
    crate::script::set_context_name("subject", Value::string(subject)).unwrap();
    let expanded = crate::script::eval_value("ec.resource.expand(subject, null)").unwrap();
    let expanded = moqui_lib::object::to_plain_string(&expanded);
    let timestamp = expanded
        .strip_prefix(&format!("{description} "))
        .unwrap_or_else(|| panic!("{expanded}"));
    assert!(!timestamp.is_empty() && !timestamp.contains('$'), "{expanded}");
}

/// The qvt Saved Finds dialog shows the active find and a form that schedules it, as Java's macros do, and once it is
/// scheduled shows the schedule instead. A mode the dialog does not offer is refused.
#[test]
fn the_saved_finds_dialog_schedules_the_active_find_then_shows_its_schedule() {
    let site = Site::new("schedule-dialog");
    let id = site.save_find("Weekly");
    assert!(
        !site.get(&[("renderMode", "qvt")]).contains("_SCHED"),
        "no active find, no schedule form"
    );
    let qvt = site.get(&[("renderMode", "qvt"), ("formListFindId", &id)]);
    assert!(qvt.contains("Active Saved Find:") && qvt.contains("Weekly"), "{qvt}");
    assert!(
        qvt.contains("_SCHED\"") && qvt.contains("name=\"ScheduleFind\"") && qvt.contains("name=\"cronSelected\""),
        "{qvt}"
    );
    assert!(qvt.contains("0 0 6 ? * MON-FRI") && qvt.contains("xsl-fo"), "{qvt}");
    super::qvt::assert_compiles(&["schedule form".to_string()], &[qvt]);

    site.post("/formSaveFind", &format!("{}&renderMode=foo", Site::schedule_body(&id)));
    assert!(site
        .rows("moqui.screen.ScreenScheduled", "formListFindId", &id)
        .is_empty());
    site.post(
        "/formSaveFind",
        &format!("{}&renderMode=xsl-fo", Site::schedule_body(&id)),
    );
    assert_eq!(
        site.rows("moqui.screen.ScreenScheduled", "formListFindId", &id).len(),
        1
    );
    let qvt = site.get(&[("renderMode", "qvt"), ("formListFindId", &id)]);
    assert!(qvt.contains("Scheduled for") && qvt.contains("PDF"), "{qvt}");
    assert!(!qvt.contains("_SCHED"), "{qvt}");
    super::qvt::assert_compiles(&["scheduled".to_string()], &[qvt]);
}

#[test]
fn a_find_is_scheduled_or_made_a_default_only_by_a_user_it_belongs_to() {
    let site = Site::new("not-mine");
    site.world.user("bob", None);
    site.world.user("cy", Some("FIND_GROUP"));
    let id = site.save_find("Ada's");
    site.post_as("bob", "/formSaveFind", &Site::schedule_body(&id));
    site.post_as(
        "bob",
        "/formSaveFind",
        &format!("formLocation=Rows&formListFindId={id}&MakeDefault=Y"),
    );
    assert!(site
        .rows("moqui.screen.ScreenScheduled", "formListFindId", &id)
        .is_empty());
    assert!(site
        .rows("moqui.screen.form.FormListFindUserDefault", "userId", "bob")
        .is_empty());
    // Shared with a group, a member may make it their default.
    let mut shared = site
        .world
        .facade
        .make_value("moqui.screen.form.FormListFindUserGroup")
        .unwrap();
    shared.set("formListFindId", Value::string(id.clone()));
    shared.set("userGroupId", Value::string("FIND_GROUP"));
    shared.create(&site.world.facade).unwrap();
    site.post_as(
        "cy",
        "/formSaveFind",
        &format!("formLocation=Rows&formListFindId={id}&MakeDefault=Y"),
    );
    assert_eq!(
        site.rows("moqui.screen.form.FormListFindUserDefault", "userId", "cy")
            .len(),
        1
    );
    // Its owner may schedule it.
    site.post("/formSaveFind", &Site::schedule_body(&id));
    assert_eq!(
        site.rows("moqui.screen.ScreenScheduled", "formListFindId", &id).len(),
        1
    );
}

#[test]
fn a_cookie_session_saves_a_find_or_columns_only_by_a_post_with_the_session_token_and_a_user() {
    let site = Site::new("token");
    let factory = &site.world.factory;
    let mut jar = Vec::new();
    let page = serve_at(factory, request("GET", "/"), &site.root);
    cookies(&page, &mut jar);
    let token = token_of(&page.body_text());
    for (path, body) in [
        ("/formSaveFind", "formLocation=Rows&a=foo&_findDescription=x"),
        ("/formSelectColumns", "formLocation=Rows&ResetColumns=Reset"),
    ] {
        let posted = serve_at(factory, with_cookies(form_post(path, body), &jar), &site.root);
        assert_eq!(posted.status, 401, "{path}: {}", posted.body_text());
        let mut get = with_cookies(request("GET", path), &jar);
        get.query = vec![("formLocation".to_string(), "Rows".to_string())];
        let got = serve_at(factory, get, &site.root);
        assert_eq!(got.status, 405, "{path}: {}", got.body_text());
    }
    // With the token the post is taken, but there is no user to save the find for.
    let posted = serve_at(
        factory,
        with_cookies(
            form_post(
                "/formSaveFind",
                &format!("formLocation=Rows&a=foo&_findDescription=x&moquiSessionToken={token}"),
            ),
            &jar,
        ),
        &site.root,
    );
    assert_eq!(posted.status, 302, "{}", posted.body_text());
    let finds = site
        .world
        .facade
        .find("moqui.screen.form.FormListFind")
        .unwrap()
        .list()
        .unwrap();
    assert!(finds.into_values().is_empty());
}

const DYNAMIC: &str = r#"<screen require-authentication="false">
    <parameter name="entityName"/>
    <parameter name="formLocationExtension"/>
    <parameter name="mark"/>
    <actions>
        <script>rows = []</script>
    </actions>
    <widgets>
        <form-list name="DataViewList" list="rows" dynamic="true" saved-finds="true" select-columns="true">
            <auto-fields-entity entity-name="${entityName}" field-type="find-display"/>
        </form-list>
    </widgets>
</screen>"#;

const ENUM_TYPE: &str = "moqui.basic.EnumerationType";
const USER_GROUP: &str = "moqui.security.UserGroup";
/// A service name the key reads. It is not run; the '#' is part of the name, as a Moqui service name has one.
const STATUS_SERVICE: &str = "org.moqui.impl.StatusServices.get#StatusColors";

/// The `formLocation` values the html posts, in document order.
fn posted_locations(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("name=\"formLocation\"") {
        let after = &rest[at..];
        let Some(value_at) = after.find("value=\"") else { break };
        let start = value_at + "value=\"".len();
        let Some(end) = after[start..].find('"') else { break };
        out.push(after[start..start + end].to_string());
        rest = &after[start + end..];
    }
    out
}

fn stored_location(site: &Site, id: &str) -> String {
    let rows = site.rows("moqui.screen.form.FormListFind", "formListFindId", id);
    assert_eq!(rows.len(), 1, "{id}");
    moqui_lib::object::to_plain_string(rows[0].get("formLocation").unwrap())
}

fn dyn_get(site: &Site, query: &[(&str, &str)]) -> String {
    let mut get = request("GET", "/dyn");
    get.query = query
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    let (status, _, body) = site.send(get);
    assert_eq!(status, 200, "{body}");
    body
}

fn dyn_post(site: &Site, path: &str, body: &str) -> String {
    let (status, location, text) = site.send(form_post(path, body));
    assert_eq!(status, 302, "{text}");
    location
}

#[test]
fn a_dynamic_saved_find_is_keyed_by_its_entity_or_service() {
    let dir = std::env::temp_dir().join(format!("moqui-saved-finds-dynamic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dyn_path = dir.join("Dyn.xml");
    std::fs::write(&dyn_path, DYNAMIC).unwrap();
    let dyn_path = dyn_path.to_string_lossy().into_owned();
    let root_xml = format!(
        r#"<screen require-authentication="false">
            <subscreens default-item="dyn"><subscreens-item name="dyn" location="{dyn_path}"/></subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#
    );
    let root = dir.join("Root.xml");
    std::fs::write(&root, root_xml).unwrap();
    let world = World::open("");
    world.user("ada", None);
    let site = Site {
        dir,
        root: root.to_string_lossy().into_owned(),
        world,
    };

    // Two renders of the same form, two keys. The column dialog still posts the bare name.
    let enum_html = dyn_get(&site, &[("entityName", ENUM_TYPE), ("mark", "1")]);
    let enum_locations = posted_locations(&enum_html);
    assert!(
        enum_locations
            .iter()
            .any(|location| location.ends_with(&format!(".form_list$DataViewList#{ENUM_TYPE}"))),
        "{enum_locations:?}"
    );
    assert!(
        enum_locations.iter().any(|location| location == "DataViewList"),
        "{enum_locations:?}"
    );
    let group_html = dyn_get(&site, &[("entityName", USER_GROUP), ("mark", "1")]);
    let group_locations = posted_locations(&group_html);
    assert!(
        group_locations
            .iter()
            .any(|location| location.ends_with(&format!(".form_list$DataViewList#{USER_GROUP}"))),
        "{group_locations:?}"
    );

    // The context extension wins over the entity.
    let extended = dyn_get(
        &site,
        &[
            ("formLocationExtension", "kept.extension"),
            ("entityName", ENUM_TYPE),
            ("mark", "1"),
        ],
    );
    let extended_locations = posted_locations(&extended);
    assert!(
        extended_locations
            .iter()
            .any(|location| location.ends_with(".form_list$DataViewList#kept.extension")),
        "{extended_locations:?}"
    );
    // The literal null is missing, so the entity form keeps the entity name.
    let missing = dyn_get(
        &site,
        &[
            ("formLocationExtension", "null"),
            ("entityName", ENUM_TYPE),
            ("mark", "1"),
        ],
    );
    let missing_locations = posted_locations(&missing);
    assert!(
        missing_locations
            .iter()
            .any(|location| location.ends_with(&format!(".form_list$DataViewList#{ENUM_TYPE}"))),
        "{missing_locations:?}"
    );
    assert!(
        !missing_locations.iter().any(|location| location.contains("#null")),
        "{missing_locations:?}"
    );

    // A full-location post is kept. A bare name of a dynamic form is keyed by the entity in the context.
    let full = format!("{dyn_path}.form_list$DataViewList#{USER_GROUP}");
    let full_redirect = dyn_post(
        &site,
        "/dyn/formSaveFind",
        &format!("formLocation={}&_findDescription=Full&mark=1", encode(&full)),
    );
    let full_id = full_redirect
        .split("formListFindId=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    assert_eq!(stored_location(&site, full_id), full);
    let bare_redirect = dyn_post(
        &site,
        "/dyn/formSaveFind",
        &format!(
            "formLocation=DataViewList&entityName={}&_findDescription=Bare&mark=1",
            encode(ENUM_TYPE)
        ),
    );
    let bare_id = bare_redirect
        .split("formListFindId=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    assert!(stored_location(&site, bare_id).ends_with(&format!(".form_list$DataViewList#{ENUM_TYPE}")));

    let enum_list = dyn_get(&site, &[("entityName", ENUM_TYPE), ("mark", "1")]);
    assert!(enum_list.contains(">Bare</a>"), "{enum_list}");
    assert!(!enum_list.contains(">Full</a>"), "{enum_list}");
    let group_list = dyn_get(&site, &[("entityName", USER_GROUP), ("mark", "1")]);
    assert!(group_list.contains(">Full</a>"), "{group_list}");
    assert!(!group_list.contains(">Bare</a>"), "{group_list}");
    // qvt of the screen itself (a subscreen's qvt answer is the active-subscreen component). The saved find's
    // form posts the full location.
    let mut qvt_get = request("GET", "/");
    qvt_get.query = vec![
        ("renderMode".to_string(), "qvt".to_string()),
        ("entityName".to_string(), ENUM_TYPE.to_string()),
    ];
    qvt_get.set_header("authorization", basic("ada"));
    let qvt = serve_at(&site.world.factory, qvt_get, &dyn_path);
    assert_eq!(qvt.status, 200, "{}", qvt.body_text());
    assert!(
        qvt.body_text()
            .contains(&format!(".form_list$DataViewList#{ENUM_TYPE}")),
        "{}",
        qvt.body_text()
    );

    // Columns stay on the bare location. A posted '#' is stripped.
    let tree = r#"[{"id":"column_0","order":1,"children":[{"id":"description","order":0}]}]"#;
    dyn_post(
        &site,
        "/dyn/formSelectColumns",
        &format!(
            "formLocation={}&SaveColumns=Save&columnsTree={}",
            encode(&full),
            encode(tree)
        ),
    );
    let bare = format!("{dyn_path}.form_list$DataViewList");
    assert!(site
        .rows("moqui.screen.form.FormConfig", "formLocation", &full)
        .is_empty());
    assert_eq!(
        site.rows("moqui.screen.form.FormConfig", "formLocation", &bare).len(),
        1
    );

    // A null service name is missing, so the entity name is the key. A null context extension is missing too.
    {
        let context = site.world.factory.new_context();
        let _entered = context.enter();
        let node = moqui_lib::node::MNode::parse_text(
            "form",
            r#"<form-list name="Both"><auto-fields-service service-name="${serviceName}"/><auto-fields-entity entity-name="${entityName}"/></form-list>"#,
        )
        .unwrap();
        crate::script::set_context_name("serviceName", Value::string("null")).unwrap();
        crate::script::set_context_name("entityName", Value::string(ENUM_TYPE)).unwrap();
        let keyed = crate::screen::saved_finds::saved_find_location("screen.form_list$Both", true, &node);
        assert_eq!(keyed, format!("screen.form_list$Both#{ENUM_TYPE}"));
        crate::script::set_context_name("formLocationExtension", Value::string("null")).unwrap();
        crate::script::set_context_name("serviceName", Value::string(STATUS_SERVICE)).unwrap();
        let keyed = crate::screen::saved_finds::saved_find_location("screen.form_list$Both", true, &node);
        assert_eq!(keyed, format!("screen.form_list$Both#{STATUS_SERVICE}"));
        assert_eq!(
            crate::screen::saved_finds::saved_find_location("screen.form_list$Rows", false, &node),
            "screen.form_list$Rows"
        );
    }

    // A non-dynamic form posted as its name stays on that name, with no extension. The first world holds the
    // cookie-property lock until it drops, and a second world takes that same lock.
    drop(site);
    let plain = Site::new("plain-key");
    let (_, plain_redirect) = plain.post(
        "/formSaveFind",
        &format!(
            "formLocation=Rows&entityName={}&a=foo&_findDescription=Plain",
            encode(ENUM_TYPE)
        ),
    );
    let plain_id = plain_redirect
        .split("formListFindId=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let plain_location = stored_location(&plain, plain_id);
    assert!(plain_location.ends_with(".form_list$Rows"), "{plain_location}");
    assert!(!plain_location.contains('#'), "{plain_location}");
}
