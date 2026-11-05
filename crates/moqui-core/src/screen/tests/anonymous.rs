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

//! Anonymous screens (`require-authentication="anonymous-*"`): the anonymous login and inheritable grant of their
//! frames, through the actions, the link checks of the render (isolated, on a fresh stack) and the sections after
//! them, and nothing left once the render ends, also after an error or with a user logged in.

use std::path::PathBuf;

use moqui_lib::object::Value;

use super::World;
use crate::context::{ErrorKind, ExecutionContext};
use crate::screen::make_render;

const SERVICES: &str = r#"<services>
    <service verb="probe" noun="Anon">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", id); made.set("testMedium", ec.user.username ?? "anonymous");</script>
            <entity-create value-field="made"/>
        </actions>
    </service>
</services>"#;

fn probe(id: &str) -> String {
    format!(r#"<service-call name="probe#Anon"><field-map field-name="id" value="{id}"/></service-call>"#)
}

/// Screen, service and entity authorization on, `probe#Anon` (which needs a login) and the user `anonplain` in no
/// group: whatever a screen runs runs on an anonymous frame's grant. The screens:
/// - `Root` needs no login and allows nothing;
/// - `Root/Open` is anonymous-all: its actions run `probe#Anon`, then it links to `Locked` (a login and an allow) and
///   `Peek` (anonymous-view), checked on a fresh stack, then a section runs `probe#Anon` again;
/// - `Root/Open/Peek` runs `probe#Anon` in its actions;
/// - `Root/Broken` is anonymous-all with always-actions that fail.
struct Fixture {
    world: World,
    dir: PathBuf,
}

impl Fixture {
    fn open(name: &str) -> Self {
        let world = World::open(
            r#"<moqui-conf><artifact-execution-facade>
                <artifact-execution type="AT_XML_SCREEN" authz-enabled="true"/>
                <artifact-execution type="AT_SERVICE" authz-enabled="true"/>
                <artifact-execution type="AT_ENTITY" authz-enabled="true"/>
            </artifact-execution-facade></moqui-conf>"#,
        );
        world.user("anonplain", None);
        let dir = std::env::temp_dir().join(format!("moqui-screen-anonymous-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Root/Open")).unwrap();
        let write = |path: &str, text: &str| std::fs::write(dir.join(path), text).unwrap();
        write("anon.xml", SERVICES);
        write(
            "Root.xml",
            r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
        );
        write(
            "Root/Open.xml",
            &format!(
                r#"<screen require-authentication="anonymous-all">
                    <actions>{}</actions>
                    <widgets>
                        <link url="Locked" text="LockedLink" link-type="anchor"/>
                        <link url="Peek" text="PeekLink" link-type="anchor"/>
                        <section name="AfterLinks"><actions>{}</actions><widgets><label text="after links"/></widgets></section>
                        <subscreens-active/>
                    </widgets></screen>"#,
                probe("${idPrefix}-open"),
                probe("${idPrefix}-after")
            ),
        );
        write(
            "Root/Open/Locked.xml",
            r#"<screen><widgets><label text="locked"/></widgets></screen>"#,
        );
        write(
            "Root/Open/Peek.xml",
            &format!(
                r#"<screen require-authentication="anonymous-view"><actions>{}</actions>
                    <widgets><label text="peek"/></widgets></screen>"#,
                probe("${idPrefix}-peek")
            ),
        );
        write(
            "Root/Broken.xml",
            r#"<screen require-authentication="anonymous-all">
                <always-actions><script>throw new Error("broken always-actions")</script></always-actions>
                <widgets><label text="broken"/></widgets></screen>"#,
        );
        let ctx = world.factory.new_context();
        {
            let _entered = ctx.enter();
            let _authz = ctx.disable_authz();
            let failures = crate::service::lint(&[dir.join("anon.xml")]).unwrap();
            assert!(failures.is_empty(), "{failures:?}");
        }
        Self { world, dir }
    }

    fn render(&self, path: &str, prefix: &str) -> Result<crate::screen::RenderOutput, crate::context::Error> {
        make_render(&self.world.factory)
            .root_screen(self.dir.join("Root.xml").to_string_lossy().into_owned())
            .screen_path(path)
            .render_mode("html")
            .parameters([("idPrefix".to_string(), Value::string(prefix))].into())
            .render()
    }

    fn medium(&self, ctx: &ExecutionContext, id: &str) -> Option<String> {
        let _authz = ctx.disable_authz();
        self.world
            .facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get("testMedium").and_then(Value::as_str).map(str::to_string))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Nothing of an anonymous screen is left: no frame, no isolated check, the session not anonymous, the user as before.
fn assert_clean(ctx: &ExecutionContext, user: Option<&str>) {
    assert_eq!(ctx.artifact_stack_names(), "");
    assert!(
        !crate::user::is_anonymous().unwrap(),
        "the anonymous login outlived the render"
    );
    assert_eq!(crate::user::current_username().unwrap().as_deref(), user);
    let err =
        crate::service::call::sync("probe#Anon", &[("id".to_string(), Value::string("AFTER"))].into()).unwrap_err();
    let expected = if user.is_some() {
        ErrorKind::Unauthorized
    } else {
        ErrorKind::AuthenticationRequired
    };
    assert_eq!(ErrorKind::of(&err), expected, "{err}");
    ctx.message().clear();
}

/// The grant and login of an anonymous-all screen reach its actions and, past the link checks, which are decided on a
/// fresh stack and then give the stack back, the section after them.
#[test]
fn an_anonymous_screen_grants_through_its_checks_and_ends_its_login() {
    let fixture = Fixture::open("render");
    let ctx = fixture.world.factory.new_context();
    let _entered = ctx.enter();
    assert_clean(&ctx, None);
    let output = fixture.render("/Open", "R").unwrap();
    let html = output.body_text();
    assert_eq!(output.status, 200, "{html}");
    assert_eq!(fixture.medium(&ctx, "R-open").as_deref(), Some("anonymous"));
    assert_eq!(fixture.medium(&ctx, "R-after").as_deref(), Some("anonymous"), "{html}");
    // The check of Locked neither inherits Open's grant nor counts the anonymous login as one.
    assert!(html.contains("PeekLink") && html.contains("Open/Peek"), "{html}");
    assert!(html.contains("LockedLink") && !html.contains("Open/Locked"), "{html}");
    assert_clean(&ctx, None);
}

/// An anonymous-view screen below an anonymous-all one keeps all actions, and the nested login ends once.
#[test]
fn a_nested_anonymous_screen_inherits_all_and_ends_one_login() {
    let fixture = Fixture::open("nested");
    let ctx = fixture.world.factory.new_context();
    let _entered = ctx.enter();
    let output = fixture.render("/Open/Peek", "N").unwrap();
    assert_eq!(output.status, 200, "{}", output.body_text());
    assert_eq!(fixture.medium(&ctx, "N-open").as_deref(), Some("anonymous"));
    assert_eq!(fixture.medium(&ctx, "N-peek").as_deref(), Some("anonymous"));
    assert_clean(&ctx, None);
}

#[test]
fn an_anonymous_screen_keeps_the_logged_in_user() {
    let fixture = Fixture::open("user");
    let ctx = fixture.world.factory.new_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("anonplain").unwrap();
    let output = fixture.render("/Open", "U").unwrap();
    let html = output.body_text();
    assert_eq!(output.status, 200, "{html}");
    assert_eq!(fixture.medium(&ctx, "U-open").as_deref(), Some("anonplain"));
    assert_eq!(fixture.medium(&ctx, "U-after").as_deref(), Some("anonplain"));
    // Locked needs an allow anonplain does not have; Open's grant does not reach the check.
    assert!(html.contains("LockedLink") && !html.contains("Open/Locked"), "{html}");
    assert_clean(&ctx, Some("anonplain"));
    crate::user::logout().unwrap();
}

/// An error while the anonymous frames are entered (failing always-actions, a subscreen that is not there) ends the
/// login and pops the frames.
#[test]
fn an_anonymous_screen_that_fails_on_entry_ends_its_login() {
    let fixture = Fixture::open("error");
    let ctx = fixture.world.factory.new_context();
    let _entered = ctx.enter();
    let broken = fixture.render("/Broken", "B");
    assert!(
        broken.as_ref().map_or(true, |output| output.status >= 400),
        "{:?}",
        broken.map(|output| output.status)
    );
    ctx.message().clear();
    assert_clean(&ctx, None);
    let missing = fixture.render("/Open/Missing", "M");
    assert!(
        missing.as_ref().map_or(true, |output| output.status >= 400),
        "{:?}",
        missing.map(|output| output.status)
    );
    ctx.message().clear();
    assert_clean(&ctx, None);
}
