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

use moqui_lib::node::MNode;

use super::{merge_extend, Extended, ScreenDef, SectionKind};
use crate::context::ExecutionContextFactory;

fn names_in(node: &MNode) -> Vec<String> {
    node.children()
        .iter()
        .map(|child| {
            child
                .attribute("text")
                .or_else(|| child.attribute("name"))
                .unwrap_or_else(|| child.name())
        })
        .collect()
}

/// `widgets-extend where="prepend|append"` puts its widgets first or last inside the named element: a section's
/// widgets, a container-box's body, a container itself (plans/design-dev-aide.md F16).
#[test]
fn widgets_extend_prepends_and_appends_inside_the_named_element() {
    let screen = MNode::parse_text(
        "s",
        r#"<screen><widgets>
                <container name="Facts"><label text="a"/><label text="b"/></container>
                <section name="More"><widgets><label text="c"/></widgets></section>
                <container-box name="Box"><box-header/><box-body><label text="d"/></box-body></container-box>
            </widgets></screen>"#,
    )
    .unwrap();
    let extend = MNode::parse_text(
        "e",
        r#"<screen-extend>
                <widgets-extend name="Facts" where="prepend"><label text="first"/></widgets-extend>
                <widgets-extend name="Facts" where="append"><label text="last"/></widgets-extend>
                <widgets-extend name="More" where="append"><label text="c2"/></widgets-extend>
                <widgets-extend name="Box" where="prepend"><label text="d0"/></widgets-extend>
                <widgets-extend name="Facts" where="after"><label text="beside"/></widgets-extend>
            </screen-extend>"#,
    )
    .unwrap();
    merge_extend(&screen, &extend, "test", &mut Extended::default());
    let widgets = screen.first_named("widgets").unwrap();
    let facts = widgets.first_named("container").unwrap();
    assert_eq!(names_in(&facts), ["first", "a", "b", "last"]);
    assert_eq!(names_in(&widgets), ["Facts", "beside", "More", "Box"]);
    let more = widgets.first_named("section").unwrap().first_named("widgets").unwrap();
    assert_eq!(names_in(&more), ["c", "c2"]);
    let body = widgets
        .first_named("container-box")
        .unwrap()
        .first_named("box-body")
        .unwrap();
    assert_eq!(names_in(&body), ["d0", "d"]);
}

/// Two components' extensions of one screen apply in the order they are merged, the component load order.
#[test]
fn extensions_apply_in_order() {
    let screen = MNode::parse_text("s", r#"<screen><widgets><container name="C"/></widgets></screen>"#).unwrap();
    for text in ["one", "two"] {
        let extend = MNode::parse_text(
            "e",
            &format!(r#"<screen-extend><widgets-extend name="C" where="append"><label text="{text}"/></widgets-extend></screen-extend>"#),
        )
        .unwrap();
        merge_extend(&screen, &extend, "test", &mut Extended::default());
    }
    let container = screen.first_named("widgets").unwrap().first_named("container").unwrap();
    assert_eq!(names_in(&container), ["one", "two"]);
}

/// A screen-extend's `shell-providers` merge into the screen's by element and key (plans/archive/Comms-Framework.md §3.3): an
/// equal key replaces in place, a new one is added at the end, a keymap merges its keys by `keys`, labels never key, and
/// a later component's extension applies over an earlier one's.
#[test]
fn shell_providers_merge_by_element_and_key() {
    let screen = MNode::parse_text(
        "s",
        r#"<screen><shell-providers>
                <quick-create kind="email" label="Email" screen="Inbox/Compose"/>
                <palette-source group="Rooms" label="Rooms" service="a.find#Rooms"/>
                <today-source service="a.get#Today" label="Today"/>
                <notification-topic topic="ChatMention"/>
                <keymap scope="app"><key keys="g i" label="Inbox" screen="Inbox"/></keymap>
                <inbox-source name="framework.email" service="a.find#EmailThreads"/>
                <page-action label="Customize" agent="dev"/>
            </shell-providers></screen>"#,
    )
    .unwrap();
    let first = MNode::parse_text(
        "e1",
        r#"<screen-extend><shell-providers>
                <palette-source group="Rooms" label="Chat rooms" service="b.find#Rooms"/>
                <keymap scope="app"><key keys="g i" label="Mail" screen="Mail"/><key keys="g c" label="Chat" screen="Chat"/></keymap>
                <inbox-source name="boss.internal" service="b.find#InboxThreads" sequence="10"/>
                <quick-create kind="task" label="Task" screen="Tasks/New"/>
                <page-action label="Customize again" agent="dev"/>
            </shell-providers></screen-extend>"#,
    )
    .unwrap();
    let second = MNode::parse_text(
        "e2",
        r#"<screen-extend><shell-providers>
                <inbox-source name="boss.internal" service="c.find#InboxThreads"/>
                <notification-topic topic="ChatMention"/>
            </shell-providers></screen-extend>"#,
    )
    .unwrap();
    for extend in [&first, &second] {
        merge_extend(&screen, extend, "test", &mut Extended::default());
    }
    let providers = crate::screen::shell::ShellProviders::of(&screen, "test").unwrap();
    let quick: Vec<&str> = providers.quick_create.iter().map(|link| link.kind.as_str()).collect();
    assert_eq!(quick, ["email", "task"]);
    assert_eq!(providers.palette_sources.len(), 1);
    assert_eq!(providers.palette_sources[0].service, "b.find#Rooms");
    assert_eq!(providers.notification_topics, ["ChatMention"]);
    let keys: Vec<(&str, &str)> = providers.keymaps[0]
        .keys
        .iter()
        .map(|key| (key.keys.as_str(), key.label.as_str()))
        .collect();
    assert_eq!(keys, [("g i", "Mail"), ("g c", "Chat")]);
    let inbox: Vec<(&str, &str)> = providers
        .providers
        .iter()
        .map(|provider| (provider.name.as_str(), provider.service.as_str()))
        .collect();
    assert_eq!(
        inbox,
        [
            ("framework.email", "a.find#EmailThreads"),
            ("boss.internal", "c.find#InboxThreads")
        ]
    );
    assert_eq!(
        providers.page_actions.len(),
        1,
        "a page-action is keyed by screen and agent, not its label"
    );
    assert_eq!(providers.page_actions[0].label, "Customize again");
}

/// A screen with no `shell-providers` takes the extension's whole; `ext-action`s merge by name.
#[test]
fn an_extension_adds_shell_providers_and_ext_items_to_a_screen_without_them() {
    let screen = MNode::parse_text("s", r#"<screen><widgets/></screen>"#).unwrap();
    let extend = MNode::parse_text(
        "e",
        r#"<screen-extend><shell-providers><record-link name="boss.records" service="b.link#Record"/></shell-providers>
            <ext-action name="boss.file" slot="message-actions" label="File" transition="file"/></screen-extend>"#,
    )
    .unwrap();
    merge_extend(&screen, &extend, "test", &mut Extended::default());
    let providers = crate::screen::shell::ShellProviders::of(&screen, "test").unwrap();
    assert_eq!(providers.providers[0].hook, "record-link");
    assert_eq!(crate::screen::ext::of(&screen, "test").actions[0].name, "boss.file");
}

/// A fragment is parsed once per thread for its readers, and `node` is still a tree of the caller's own.
#[test]
fn readers_of_a_fragment_share_one_parse_on_a_thread() {
    let fragment = super::XmlFragment::of(&MNode::parse_text("t", r#"<widgets><label text="a"/></widgets>"#).unwrap());
    let first = fragment.shared_node().unwrap();
    first.set_attribute("seen", "1");
    assert!(fragment.shared_node().unwrap().attribute_is("seen", "1"));
    assert!(!fragment.node().unwrap().has_attribute("seen"));
    let copy = fragment.clone();
    assert!(copy.shared_node().unwrap().attribute_is("seen", "1"));
    let other = std::thread::spawn(move || copy.shared_node().unwrap().has_attribute("seen"));
    assert!(!other.join().unwrap());
}
use crate::screen::ScreenFacade;

const BASE: &str = r#"<screen require-authentication="false" default-menu-title="Base">
        <parameter name="a"/>
        <transition name="t1"><default-response type="none"/></transition>
        <subscreens default-item="One"><subscreens-item name="One" location="x/One.xml" menu-title="One"/></subscreens>
        <pre-actions><script>base_pre()</script></pre-actions>
        <actions><script>base_actions()</script></actions>
        <widgets>
            <container id="box"><label text="inside"/></container>
            <section name="main"><widgets><label text="main"/></widgets></section>
            <form-single name="Edit"><field name="f1"/></form-single>
        </widgets>
    </screen>"#;

const EXTEND: &str = r#"<screen-extend default-menu-title="Extended" menu-image="icon">
        <parameter name="b" required="true"/>
        <transition name="t2"><default-response type="none"/></transition>
        <subscreens><subscreens-item name="Two" location="x/Two.xml" menu-title="Two"/></subscreens>
        <actions-extend type="pre-actions" when="before"><script>extend_pre_before()</script></actions-extend>
        <actions-extend type="actions" when="replace"><script>extend_actions()</script></actions-extend>
        <actions-extend type="always-actions"><script>extend_always()</script></actions-extend>
        <widgets-extend name="box" where="before"><label text="before-box"/></widgets-extend>
        <widgets-extend name="main" where="after"><label text="after-main"/></widgets-extend>
        <section name="main"><widgets><label text="replaced"/></widgets></section>
        <form-single name="Edit"><field name="f2"/></form-single>
    </screen-extend>"#;

#[test]
fn a_screen_extend_merges_attributes_actions_widgets_and_replaces_sections() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let base = MNode::parse_text("Base.xml", BASE).unwrap();
    let extend = MNode::parse_text("Base.xml", EXTEND).unwrap();
    let mut extended = Extended::default();
    merge_extend(&base, &extend, "Base.xml", &mut extended);
    let def = ScreenDef::build(&facade, context.factory(), &base, "Base.xml", 0, extended, Vec::new()).unwrap();

    assert_eq!(def.default_menu_title, "Extended");
    assert_eq!(def.menu_image.as_deref(), Some("icon"));
    assert!(def.parameter("a").is_some() && def.parameter("b").unwrap().required);
    assert!(def.has_transition("t1") && def.has_transition("t2"));
    assert!(def.subscreens.get("One").is_some() && def.subscreens.get("Two").is_some());
    // The base subscreens default stays when the extend names none.
    assert_eq!(def.subscreens.default_item.as_deref(), Some("One"));

    let pre = def.pre_actions.as_ref().unwrap().xml.to_string();
    assert!(
        pre.find("extend_pre_before").unwrap() < pre.find("base_pre").unwrap(),
        "{pre}"
    );
    let actions = def.root.actions.as_ref().unwrap().xml.to_string();
    assert!(
        actions.contains("extend_actions") && !actions.contains("base_actions"),
        "{actions}"
    );
    assert!(def.always_actions.as_ref().unwrap().xml.contains("extend_always"));

    let source = def.source.text();
    let before = source.find("before-box").unwrap();
    assert!(before < source.find("id=\"box\"").unwrap(), "{source}");
    // The extend replaced the section by name, and a widgets-extend after it adds its widgets.
    let main = def.sections.get("main").unwrap();
    assert!(main.widgets.as_ref().unwrap().text().contains("replaced"));
    assert!(source.contains("after-main"));
    // Same-named forms keep the base form and list the extension beside it.
    let form = def.forms.get("Edit").unwrap();
    assert!(!form.list && form.extensions.len() == 1);
    assert!(form.extensions[0].text().contains("f2"));
}

#[test]
fn sections_forms_trees_includes_and_settings_are_read() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-model-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Shared.xml"),
        r#"<screen require-authentication="false">
                <transition name="shared"><default-response type="none"/></transition>
                <widgets><section name="common"><actions><script>common_actions()</script></actions><widgets><label text="c"/>
                    <form-single name="SharedIn"><field name="a"><default-field><display/></default-field></field></form-single></widgets></section>
                    <form-single name="SharedOut"><field name="b"><default-field><display/></default-field></field></form-single></widgets>
            </screen>"#,
    )
    .unwrap();
    let shared = dir.join("Shared.xml").to_string_lossy().into_owned();
    let model = format!(
        r#"<screen require-authentication="anonymous-view" begin-transaction="true" transaction-timeout="90"
                    include-child-content="true" history="false" menu-image="fa-home" menu-image-type="icon"
                    server-static="qvt, html" render-modes="html,xsl-fo" login-path="/In" track-artifact-hit="false">
                <macro-template type="html" location="x/Html.js"/>
                <web-settings mime-type="text/x-test" character-encoding="ISO-8859-1" require-encryption="false"/>
                <parameter name="p1" required="true" from="x"/>
                <transition-include name="shared" location="{shared}"/>
                <transition name="svc" read-only="true" authz-action="all">
                    <condition><expression>ok</expression></condition>
                    <path-parameter name="id"/>
                    <conditional-response type="screen-last" save-current-screen="true"/>
                    <default-response url="Next" url-type="plain" parameter-map="{{a: 1}}" save-parameters="true"/>
                    <error-response type="none"/>
                </transition>
                <subscreens default-item="A" always-use-full-path="true">
                    <conditional-default condition="x == 1" item="B"/>
                    <subscreens-item name="A" location="a/A.xml" menu-title="Item A" menu-index="3" menu-include="false" no-sub-path="true"/>
                </subscreens>
                <widgets>
                    <subscreens-menu id="m" type="tab"/>
                    <section name="outer" condition="flag"><actions><set field="o" from="1"/></actions>
                        <widgets><section-iterate name="rows" list="things" entry="thing" key="k" paginate="true">
                            <widgets><label text="${{thing}}"/></widgets></section-iterate></widgets>
                        <fail-widgets><label text="none"/></fail-widgets>
                    </section>
                    <section-include name="common" location="{shared}"/>
                    <section-include name="dyn${{x}}" location="${{y}}"/>
                    <form-list name="List1" list="things"/>
                    <tree name="Tree1"/>
                </widgets>
                <fail-widgets><label text="root-fail"/></fail-widgets>
            </screen>"#
    );
    std::fs::write(dir.join("Model.xml"), model).unwrap();
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let def = facade
        .screen_def(context.factory(), &dir.join("Model.xml").to_string_lossy())
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(def.authentication, super::Authentication::AnonymousView);
    assert!(def.begin_transaction && def.transaction_timeout == Some(90));
    assert!(def.include_child_content && !def.history && !def.track_artifact_hit);
    assert_eq!(def.menu_image.as_deref(), Some("fa-home"));
    assert_eq!(def.menu_image_type, "icon");
    assert_eq!(def.login_path.as_deref(), Some("/In"));
    assert_eq!(def.server_static, ["qvt", "html"]);
    assert!(def.is_server_static("qvt") && !def.is_server_static("csv"));
    assert!(def.allows_render_mode("html") && def.allows_render_mode("html-print") && !def.allows_render_mode("csv"));
    assert_eq!(def.macro_template_location("html"), Some("x/Html.js"));
    assert_eq!(def.web.mime_type.as_deref(), Some("text/x-test"));
    assert_eq!(def.web.character_encoding.as_deref(), Some("ISO-8859-1"));
    assert!(!def.web.require_encryption && def.web.allow_web_request);
    assert!(def.has_tab_menu);
    assert_eq!(def.default_menu_title, "Model");

    // transition-include copies the transition and runs it under this screen.
    let shared = def.transition("shared", None).unwrap();
    assert_eq!(shared.screen_location, def.location);
    let svc = def.transition("svc", None).unwrap();
    assert!(svc.read_only && svc.authz_action == "all" && svc.condition.is_some());
    assert_eq!(svc.default_response.url_type, "plain");
    assert_eq!(svc.default_response.parameter_map.as_deref(), Some("{a: 1}"));
    assert!(svc.default_response.save_parameters);
    assert_eq!(svc.conditional_responses[0].kind, super::ResponseKind::ScreenLast);
    assert!(svc.conditional_responses[0].save_current_screen);

    let a = def.subscreens.get("A").unwrap();
    assert!(a.no_sub_path && !a.menu_include && a.menu_index == Some(3));
    assert!(def.subscreens.always_use_full_path);
    assert_eq!(def.subscreens.conditional_defaults.len(), 1);

    let outer = def.sections.get("outer").unwrap();
    assert!(matches!(outer.kind, SectionKind::Section));
    assert!(outer.condition.is_some() && outer.actions.is_some());
    assert!(outer.fail_widgets.as_ref().unwrap().text().contains("none"));
    match &def.sections.get("rows").unwrap().kind {
        SectionKind::Iterate {
            list,
            entry,
            key,
            paginate,
        } => {
            assert_eq!(
                (list.as_str(), entry.as_str(), key.as_deref(), *paginate),
                ("things", "thing", Some("k"), true)
            );
        }
        other => panic!("{other:?}"),
    }
    // An included section is copied in; a name or location with ${} waits for the render.
    assert!(def.sections.get("common").unwrap().actions.is_some());
    // Only what the included section holds comes along.
    assert!(def.forms.contains_key("SharedIn") && !def.forms.contains_key("SharedOut"));
    assert!(matches!(
        def.sections.get("dyn${x}").unwrap().kind,
        SectionKind::Include { .. }
    ));
    assert!(def.forms.get("List1").unwrap().list);
    assert!(def.trees.contains_key("Tree1"));
    assert!(def.root.fail_widgets.is_some());
    // The widget subtree comes back as an MNode for the macro walker.
    let widgets = outer.widgets.as_ref().unwrap().node().unwrap();
    assert_eq!(widgets.name(), "widgets");
    assert_eq!(def.screen_node().unwrap().name(), "screen");
}

#[test]
fn a_screen_of_only_subscreens_placeholders_is_server_static() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let node = MNode::parse_text(
        "P.xml",
        "<screen><widgets><subscreens-menu id=\"m\"/><subscreens-active/></widgets></screen>",
    )
    .unwrap();
    let def = ScreenDef::build(
        &facade,
        context.factory(),
        &node,
        "P.xml",
        0,
        Extended::default(),
        Vec::new(),
    )
    .unwrap();
    assert!(def.is_server_static("qvt"));
    assert_eq!(
        super::pretty_menu_name("a/FindCustomerOrder.xml"),
        "Find Customer Order"
    );
}

/// A screen's own `<condition>` is its root section's, and decides whether the screen is offered only when the screen has
/// no `fail-widgets` (screen/menu.rs `has_offer_condition`).
#[test]
fn a_screen_condition_is_its_root_sections_and_offers_it_without_fail_widgets() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let build = |text: &str| {
        let node = MNode::parse_text("C.xml", text).unwrap();
        ScreenDef::build(
            &facade,
            context.factory(),
            &node,
            "C.xml",
            0,
            Extended::default(),
            Vec::new(),
        )
        .unwrap()
    };
    let offered = build(
        "<screen><condition><expression>appRoot == 'Here'</expression></condition><widgets><label text=\"x\"/></widgets></screen>",
    );
    assert!(matches!(
        &offered.root.condition,
        Some(super::Condition::Expression(expression)) if expression == "appRoot == 'Here'"
    ));
    assert!(crate::screen::menu::has_offer_condition(&offered));
    let choosing = build(
        "<screen><condition><expression>appRoot != 'Here'</expression></condition><widgets/><fail-widgets><label text=\"y\"/></fail-widgets></screen>",
    );
    assert!(choosing.root.condition.is_some() && !crate::screen::menu::has_offer_condition(&choosing));
    assert!(!crate::screen::menu::has_offer_condition(&build(
        "<screen><widgets/></screen>"
    )));
}

#[test]
fn a_transition_name_is_also_a_pattern_for_a_request_with_a_method() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let node = MNode::parse_text(
        "T.xml",
        r#"<screen><transition name="exact"><default-response type="none"/></transition>
                <transition name="report_(csv|xml)" method="get"><default-response type="none"/></transition></screen>"#,
    )
    .unwrap();
    let def = ScreenDef::build(
        &facade,
        context.factory(),
        &node,
        "T.xml",
        0,
        Extended::default(),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(
        def.transition("exact", Some("post")).map(|t| t.name.as_str()),
        Some("exact")
    );
    assert_eq!(
        def.transition("report_csv", Some("GET")).map(|t| t.name.as_str()),
        Some("report_(csv|xml)")
    );
    assert!(
        def.transition("report_csv", Some("post")).is_none(),
        "the method must match"
    );
    assert!(
        def.transition("report_csv", None).is_none(),
        "a lookup with no method takes names only"
    );
    assert!(def.transition("report_pdf", Some("get")).is_none());
}

#[test]
fn a_lookup_with_no_method_prefers_any_then_get_over_the_first_declared() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let node = MNode::parse_text(
        "T.xml",
        r#"<screen><transition name="image" method="post"><default-response type="none"/></transition>
                <transition name="image" method="get" read-only="true"><default-response type="none"/></transition>
                <transition name="oneClick" method="post"><default-response type="none"/></transition>
                <transition name="oneClick" read-only="true"><default-response type="none"/></transition>
                <transition name="save" method="post"><default-response type="none"/></transition></screen>"#,
    )
    .unwrap();
    let def = ScreenDef::build(
        &facade,
        context.factory(),
        &node,
        "T.xml",
        0,
        Extended::default(),
        Vec::new(),
    )
    .unwrap();
    let method = |name: &str, request: Option<&str>| def.transition(name, request).map(|t| t.method.as_str());
    assert_eq!(method("image", None), Some("get"));
    assert_eq!(method("image", Some("post")), Some("post"));
    assert_eq!(method("oneClick", None), Some("any"));
    assert_eq!(method("oneClick", Some("post")), Some("post"));
    assert_eq!(
        method("save", None),
        Some("post"),
        "the first declared is the last fallback"
    );
}

/// `menu-include`: true and false in any case are literals, a near-literal is taken for what it says (with a warning),
/// an expression is kept, and one that does not parse leaves the item out (logged once, at load).
#[test]
fn menu_include_literals_near_literals_and_expressions() {
    let of = |text: &str| super::menu_include_of(Some(text), "test.xml:1");
    assert_eq!(super::menu_include_of(None, "test.xml:1"), (true, None));
    for (text, include) in [("true", true), ("TRUE", true), ("False", false), (" false ", false)] {
        assert_eq!(of(text), (include, None), "{text}");
    }
    for (text, include) in [("Y", true), ("yes", true), ("N", false), ("no", false), ("off", false)] {
        assert_eq!(of(text), (include, None), "{text}");
    }
    assert_eq!(
        of("ec.user.hasPermission('X')"),
        (true, Some("ec.user.hasPermission('X')".to_string()))
    );
    assert_eq!(of("ec.user.hasPermission("), (false, None));
}

/// A transition's `service-call` shortcut is a multi call for a post with `_isMulti=true` only when it writes
/// `multi="parameter"` or a multi form of the screen posts to it: a `form-list` with `multi="true"` (its own or the
/// one it extends), or a `row-selection` action not in `list-mode`. Any other shortcut stays one call, so a posted
/// `<name>_0` cannot stand in for an id the screen pinned (SEC-RTE-81). A transition taken by `transition-include`
/// becomes a multi call when a multi form of the including screen posts to it.
#[test]
fn a_shortcut_is_a_multi_call_only_when_written_or_a_multi_form_posts_to_it() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shared = dir.join("Shared.xml").to_string_lossy().into_owned();
    std::fs::write(
        &shared,
        r#"<screen require-authentication="false">
                <transition name="saveShared"><service-call name="save#Shared"/><default-response type="none"/></transition>
                <widgets>
                    <form-list name="SharedMulti" list="rows" multi="true" transition="fromOther">
                        <field name="x"/><field name="y"/></form-list>
                    <form-list name="SelBase" list="rows">
                        <row-selection id-field="rowId" parameter="selIds"><action><form-single name="SelAct" transition="fromInherited"/></action></row-selection>
                    </form-list>
                </widgets></screen>"#,
    )
    .unwrap();
    let screen = format!(
        r#"<screen require-authentication="false">
                <parameter name="screenParam" from="other"/>
                <always-actions><set field="ownerId" from="ec.user.userId"/><if condition="true"><set field="nested.part" value="x"/></if></always-actions>
                <transition name="plain"><parameter name="transitionParam" value="v"/><parameter name="plainParam"/><service-call name="update#Account"/><default-response type="none"/></transition>
                <transition name="written"><service-call name="update#Account" multi="parameter"/><default-response type="none"/></transition>
                <transition name="never"><service-call name="update#Account" multi="false"
                    in-map="Object.assign({{}}, {{ownerId: ec.user.userId, 'quoted': 1, short}}, ctx ? a : b)"><field-map field-name="fromField" from="x"/></service-call><default-response type="none"/></transition>
                <transition name="fromList"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="fromBase"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="fromSelection"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="fromListMode"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="fromSingle"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="fromOther"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="fromInherited"><service-call name="update#Row"/><default-response type="none"/></transition>
                <transition name="queued"><service-call name="update#Row" async="true"/><default-response type="none"/></transition>
                <transition-include name="saveShared" location="{shared}"/>
                <widgets>
                    <form-list name="Rows" list="rows" transition="fromList" multi="true"><field name="rowA"/></form-list>
                    <form-list name="Cross" list="rows" extends="{shared}#SharedMulti"><field name="z"/></form-list>
                    <form-list name="CrossSel" list="rows" extends="{shared}#SelBase"/>
                    <form-list name="Auto" list="rows" transition="fromAuto" multi="true"><auto-fields-entity entity-name="x"/></form-list>
                    <form-list name="Base" list="rows" transition="fromBase" multi="true"/>
                    <form-list name="Extending" list="rows" extends="Base"/>
                    <form-list name="SharedRows" list="rows" transition="saveShared" multi="true"/>
                    <form-list name="Queued" list="rows" transition="queued" multi="true"/>
                    <form-list name="Selected" list="rows">
                        <row-selection id-field="rowId"><action><form-single name="Act" transition="fromSelection"/></action></row-selection>
                        <field name="rowId"><default-field><display/></default-field></field></form-list>
                    <form-list name="Listed" list="rows">
                        <row-selection id-field="rowId" list-mode="true"><action><form-single name="ActList" transition="fromListMode"/></action></row-selection>
                        <field name="rowId"><default-field><display/></default-field></field></form-list>
                    <form-single name="One" transition="fromSingle"/>
                </widgets>
            </screen>"#
    );
    std::fs::write(dir.join("Multi.xml"), screen).unwrap();
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let def = facade
        .screen_def(context.factory(), &dir.join("Multi.xml").to_string_lossy())
        .unwrap();
    let source = facade.screen_def(context.factory(), &shared).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let multi = |name: &str| {
        def.transition(name, Some("post"))
            .map(|transition| transition.multi_parameter)
    };
    let set = |names: &[&str]| {
        Some(
            names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<std::collections::BTreeSet<_>>(),
        )
    };
    assert_eq!(def.multi_rows.get("fromList").cloned().flatten(), set(&["rowA"]));
    assert_eq!(
        def.multi_rows.get("fromOther").cloned().flatten(),
        set(&["x", "y", "z"]),
        "fields through a cross-screen extends"
    );
    assert_eq!(
        def.multi_rows.get("fromInherited").cloned().flatten(),
        set(&["selIds"]),
        "a row-selection through extends"
    );
    assert_eq!(def.multi_rows.get("fromSelection").cloned().flatten(), set(&["rowId"]));
    assert_eq!(
        def.multi_rows.get("fromAuto"),
        Some(&None),
        "assembled fields are not known"
    );
    assert_eq!(def.pinned_names, set(&["nested", "ownerId", "screenParam"]).unwrap());
    let plain = def.transition("plain", Some("post")).unwrap();
    assert_eq!(plain.pinned_names, set(&["transitionParam"]).unwrap());
    let never = def.transition("never", Some("post")).unwrap();
    assert_eq!(
        never.in_map_pins,
        set(&["fromField", "ownerId", "quoted", "short"]).unwrap()
    );
    for name in [
        "written",
        "fromList",
        "fromBase",
        "fromSelection",
        "saveShared",
        "fromOther",
        "fromInherited",
    ] {
        assert_eq!(multi(name), Some(true), "{name}");
    }
    for name in ["plain", "never", "fromListMode", "fromSingle", "queued"] {
        assert_eq!(multi(name), Some(false), "{name}");
    }
    assert!(
        !source.transition("saveShared", Some("post")).unwrap().multi_parameter,
        "the included screen's own transition keeps its default"
    );
}

/// A screen's action block holds the scope a service call holds (SEC-RTE-92): one that disables authorization, entity
/// ECA rules and tarpit counting and throws, or never enables authorization, leaves the render's state as it was, and a
/// caller's own disable is still in force after a block that enables it.
#[test]
fn an_action_blocks_authorization_toggles_end_with_it() {
    crate::script::clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let block = |script: &str| super::Actions::of(format!("<actions><script>{script}</script></actions>"));
    let failed = block(
        "ec.artifactExecution.disableAuthz(); ec.artifactExecution.disableEntityEca(); \
         ec.artifactExecution.disableTarpit(); throw new Error('down')",
    )
    .run();
    assert!(failed.is_err(), "the block threw");
    assert!(
        !context.authz_disabled(),
        "a block that threw left authorization disabled"
    );
    assert!(
        !context.entity_eca_disabled(),
        "a block that threw left entity ECA rules disabled"
    );
    assert!(
        !context.tarpit_disabled(),
        "a block that threw left tarpit counting disabled"
    );
    block("ec.artifactExecution.disableAuthz()").run().unwrap();
    assert!(
        !context.authz_disabled(),
        "a block that never enabled authorization left it disabled"
    );
    let _held = context.disable_authz();
    let eca = context.disable_entity_eca();
    block("ec.artifactExecution.enableAuthz(); ec.artifactExecution.enableEntityEca()")
        .run()
        .unwrap();
    assert!(context.authz_disabled(), "a block ended the render's own disable");
    assert!(
        context.entity_eca_disabled(),
        "a block ended the render's own disableEntityEca"
    );
    if !eca {
        context.enable_entity_eca();
    }
}
