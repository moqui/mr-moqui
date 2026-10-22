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

use std::path::PathBuf;

fn runtime() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime")
}

/// The framework's and the tools component's skills, as the sync reads them.
fn shipped() -> SkillIndex {
    let base = runtime().join("base-component");
    let read = sync::read_dirs(&[
        ("framework".to_string(), base.join("framework/skill")),
        ("tools".to_string(), base.join("tools/skill")),
    ]);
    assert!(read.problems.is_empty(), "{:?}", read.problems);
    SkillIndex {
        skills: read.skills,
        aliases: read.aliases,
    }
}

#[test]
fn the_committed_retrieval_set_reaches_its_recall() {
    let text = std::fs::read_to_string(runtime().join("base-component/tools/test/skill-retrieval.json")).unwrap();
    let cases = retrieval::read_cases(&text).unwrap();
    let report = retrieval::evaluate(&shipped(), &cases);
    println!("{}", report.text("tools"));
    assert!(report.recall() >= 0.9, "{}", report.text("tools"));
    assert!(report.strong_false.is_empty(), "{}", report.text("tools"));
}

fn skill(text: &str) -> Skill {
    parse_skill(text).unwrap()
}

const USER: &str = "---\nname: create-user-account\ntitle: Creating a user account\ndescription: Use when the user asks to add a login.\nkind: procedure\naliases: [login, new user]\n---\nSteps.\n";

#[test]
fn front_matter_reads_lists_maps_and_comments() {
    let read = parse_skill(
        "---\nname: data-reads   # required\ntitle: \"Reading lists\"\ndescription: Use when reading rows.\nkind: guide\n\
         tags: [rows, Table]\naliases:\n  - count\n  - total\nunlocks: [write_ui]\nrequires: screen-use, dates\n\
         screens: [/apps/system/Security]\nrisk: irreversible\naudience: [assist]\nsurfaces: [drawer, tui]\nlocked: true\n\
         version: 3\noverrides: data-reads\nsections:\n  Where: The where grammar\n  artifacts-and-grep: Reading on\n\
         colour: blue\n---\nRead lists with data.\n\n## Where\nfield values.\n```\n## not a heading\n```\n\n## Artifacts and grep\nartifact=A1\n",
    )
    .unwrap();
    assert_eq!(read.name, "data-reads");
    assert_eq!(read.title, "Reading lists");
    assert_eq!(read.aliases, ["rows", "table", "count", "total"]);
    assert_eq!(read.unlocks, ["write_ui"]);
    assert_eq!(read.requires, ["screen-use", "dates"]);
    assert_eq!(read.declared_risk, Some(Risk::Irreversible));
    assert_eq!(read.agents, ["assist"]);
    assert_eq!(read.surfaces, [Surface::Drawer, Surface::Tui]);
    assert!(read.locked);
    assert_eq!(read.version, 3);
    assert_eq!(read.overrides.as_deref(), Some("data-reads"));
    let (intro, sections) = read.sections();
    assert_eq!(intro, "Read lists with data.");
    let slugs: Vec<&str> = sections.iter().map(|section| section.slug.as_str()).collect();
    assert_eq!(slugs, ["where", "artifacts-and-grep"]);
    assert!(sections[0].text.contains("## not a heading"));
    assert_eq!(read.purpose(&sections[0]), "The where grammar");
    // what an editor shows reads back as the same skill
    let again = parse_skill(&read.to_markdown()).unwrap();
    assert_eq!((again.aliases, again.surfaces, again.purposes, again.body), (read.aliases, read.surfaces, read.purposes, read.body));
}

#[test]
fn front_matter_errors_name_what_is_wrong() {
    let error = |text: &str| parse_skill(text).unwrap_err();
    assert!(error("no front matter").contains("front matter"));
    assert!(error("---\nname: Bad Name\ndescription: Use when x.\n---\n").contains("not kebab-case"));
    assert!(error("---\nname: no-description\n---\n").contains("no description"));
    assert!(error("---\nname: x\ndescription: Lists rows.\n---\n").contains("Use when"));
    let long = format!("---\nname: x\ndescription: Use when {}\n---\n", "y".repeat(200));
    assert!(error(&long).contains("over 160"));
    let many = error("---\nname: x\ndescription: Use when x.\nkind: recipe\nrisk: maybe\nsurfaces: [web]\nlocked: perhaps\nversion: two\n---\n");
    for part in ["kind is recipe", "risk is maybe", "web is not a surface", "locked is perhaps", "version is two"] {
        assert!(many.contains(part), "{many}");
    }
    assert!(error("---\nname: x\ndescription: Use when x.\n").contains("closing ---"));
}

#[test]
fn a_bad_file_is_left_out_by_name_and_the_others_are_read() {
    let dir = std::env::temp_dir().join(format!("moqui-skills-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("more")).unwrap();
    std::fs::write(dir.join("create-user-account.md"), USER).unwrap();
    std::fs::write(dir.join("more/dates.md"), "---\nname: dates\ndescription: Use when a date is a phrase.\n---\nbody\n").unwrap();
    std::fs::write(dir.join("broken.md"), "---\nname: broken\n---\nno description\n").unwrap();
    std::fs::write(dir.join("again.md"), USER).unwrap();
    std::fs::write(dir.join("aliases.json"), r#"{"customer": ["client", "buyer"]}"#).unwrap();
    let read = sync::read_dirs(&[("example".to_string(), dir.clone()), ("missing".to_string(), dir.join("missing"))]);
    let _ = std::fs::remove_dir_all(&dir);
    let names: Vec<&str> = read.skills.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, ["create-user-account", "dates"]);
    assert_eq!(read.skills[0].location, "component://example/skill/again.md", "the first file of a name wins");
    assert_eq!(read.skills[1].location, "component://example/skill/more/dates.md");
    assert_eq!(read.skills[0].provenance, Provenance::Component);
    assert_eq!(read.problems.len(), 2, "{:?}", read.problems);
    assert!(read.problems.iter().any(|problem| problem.contains("component://example/skill/broken.md") && problem.contains("no description")));
    assert!(read.problems.iter().any(|problem| problem.contains("create-user-account.md") && problem.contains("already defined")));
    let index = SkillIndex {
        skills: read.skills,
        aliases: read.aliases,
    };
    assert_eq!(index.alias_words("Customer"), ["client", "buyer"]);
    assert_eq!(index.alias_words("buyer"), ["customer"]);
}

fn row(text: &str, provenance: Provenance, owner: Option<&str>) -> Skill {
    Skill {
        provenance,
        owner: owner.map(str::to_string),
        skill_id: format!("{}-{}", provenance.as_str(), owner.unwrap_or("site")),
        ..skill(text)
    }
}

#[test]
fn personal_skills_shadow_unlocked_site_skills_and_files_need_overrides() {
    let file = row(USER, Provenance::Component, None);
    let mine = row(&USER.replace("Steps.", "My steps."), Provenance::User, Some("U1"));
    let theirs = row(&USER.replace("Steps.", "Their steps."), Provenance::User, Some("U2"));
    let chosen = |candidates: Vec<Skill>, user: Option<&str>| resolve(candidates, user)[0].body.clone();
    assert_eq!(chosen(vec![file.clone(), mine.clone(), theirs.clone()], Some("U1")), "My steps.");
    assert_eq!(chosen(vec![file.clone(), mine.clone(), theirs.clone()], Some("U3")), "Steps.");
    assert_eq!(chosen(vec![file.clone(), mine.clone()], None), "Steps.", "no user, no personal skill");
    let locked = Skill { locked: true, ..file.clone() };
    assert_eq!(chosen(vec![locked, mine.clone()], Some("U1")), "Steps.", "a locked skill is not shadowed");
    // a site row replaces a file only with overrides
    let site = row(&USER.replace("Steps.", "Site steps."), Provenance::Admin, None);
    assert_eq!(chosen(vec![file.clone(), site.clone()], None), "Steps.");
    let overriding = Skill { overrides: Some("create-user-account".to_string()), ..site.clone() };
    assert_eq!(chosen(vec![file.clone(), overriding], None), "Site steps.");
    // a learned row never takes a name a file has, not even as a personal one
    let learned = row(&USER.replace("Steps.", "Learned."), Provenance::Learned, None);
    assert_eq!(chosen(vec![file.clone(), learned], None), "Steps.");
    let learned_mine = row(&USER.replace("Steps.", "Learned mine."), Provenance::Learned, Some("U1"));
    assert_eq!(chosen(vec![file.clone(), learned_mine], Some("U1")), "Steps.");
    // drafts and retired rows do not count
    let draft = Skill { status_id: DRAFT.to_string(), ..mine };
    assert_eq!(chosen(vec![file, draft], Some("U1")), "Steps.");
}

#[test]
fn only_a_skill_a_person_wrote_lowers_a_risk() {
    let reversible = USER.replace("kind: procedure", "kind: procedure\nrisk: reversible");
    assert_eq!(row(&reversible, Provenance::Admin, None).risk(), Risk::Reversible);
    assert_eq!(row(&reversible, Provenance::Learned, None).risk(), Risk::Confirm);
    let irreversible = USER.replace("kind: procedure", "kind: procedure\nrisk: irreversible");
    assert_eq!(row(&irreversible, Provenance::Learned, None).risk(), Risk::Irreversible, "raising is anyone's");
    assert_eq!(row(USER, Provenance::Framework, None).risk(), Risk::Confirm);
}

#[test]
fn retrieval_scores_names_aliases_stems_and_pages_and_strong_needs_more_than_a_shared_word() {
    let index = shipped();
    let top = |text: &str, page: Option<&str>| retrieval::rank(&index, text, page).first().map(|found| found.name.clone());
    assert_eq!(top("creating user accounts", None).as_deref(), Some("create-user-account"), "query stems");
    let login = retrieval::rank(&index, "add a login for Jane", None);
    assert_eq!(login[0].name, "create-user-account");
    assert!(login[0].strong && login[0].why.iter().any(|why| why == "alias:login"), "{login:?}");
    let shared = retrieval::score(&index, index.get("create-user-account").unwrap(), "who manages the account", None);
    assert!(shared.lexical > 0 && !shared.strong, "one shared word of three is not strong: {shared:?}");
    let on_page = retrieval::score(
        &index,
        &Skill { screens: vec!["/apps/system/Security/UserAccount".to_string()], ..index.get("create-user-account").unwrap().clone() },
        "add one",
        Some("/qapps/system/Security/UserAccount/UserAccountList?x=1"),
    );
    assert!(on_page.why.contains(&"page".to_string()), "{on_page:?}");
    let mut sorted = retrieval_stop_words();
    sorted.sort_unstable();
    assert_eq!(sorted, retrieval_stop_words(), "the stop words are sorted for the binary search");
}

fn retrieval_stop_words() -> Vec<&'static str> {
    retrieval::STOP_WORDS.to_vec()
}

#[test]
fn the_semantic_branch_fuses_by_reciprocal_rank_and_is_never_strong() {
    let index = shipped();
    let embedder = crate::search::semantic::HashEmbedder::new(64);
    let cache = std::sync::Mutex::new(std::collections::HashMap::new());
    let fused = retrieval::rank_hybrid(&index, "hashes the password of a login", None, Some(&embedder), Some(&cache));
    assert_eq!(fused[0].name, "create-user-account");
    assert!(fused[0].semantic.is_some() && fused[0].score < 1.0, "{:?}", fused[0]);
    assert!(fused.iter().filter(|found| found.lexical == 0).all(|found| !found.strong));
    assert_eq!(cache.lock().unwrap().len(), index.skills.len(), "each skill embedded once");
    let again = retrieval::rank_hybrid(&index, "hashes the password of a login", None, Some(&embedder), Some(&cache));
    assert_eq!(again, fused);
}

#[test]
fn the_catalog_lists_pins_then_unlocking_guides_then_relevance_then_use() {
    let mut index = shipped();
    index.skills.retain(|skill| skill.unlocks.is_empty());
    for (at, skill) in index.skills.iter_mut().enumerate() {
        skill.use_count = at as i64;
    }
    index.skills.push(skill("---\nname: canvas-guide\ndescription: Use when you draw.\nkind: guide\nunlocks: [write_ui]\n---\nx\n"));
    let pins = vec!["approvals".to_string()];
    let tools = vec!["screen".to_string(), "write_ui".to_string()];
    let catalog = catalog(
        &index,
        &CatalogInput {
            pins: &pins,
            first_text: "add a login for Jane",
            page: None,
            tools: &tools,
        },
    );
    let names: Vec<&str> = catalog.entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(&names[..3], ["approvals", "canvas-guide", "create-user-account"], "{names:?}");
    assert_eq!(catalog.entries.len() + catalog.more, index.skills.len());
    let text = prompt::catalog_text(&catalog);
    assert!(text.contains("- create-user-account: Use when the user asks to add a login, a user or a user account for someone. (procedure)"), "{text}");
}

/// The catalog of the shipped skills fits its budget, every line whole, and the same first message gives the same bytes.
#[test]
fn the_shipped_catalog_fits_its_budget_and_is_the_same_for_the_same_message() {
    let index = shipped();
    let tools = vec!["write_ui".to_string()];
    let input = CatalogInput {
        pins: &[],
        first_text: "show the open user accounts",
        page: Some("/apps/system/Security/UserAccount/UserAccountList"),
        tools: &tools,
    };
    let text = prompt::catalog_text(&catalog(&index, &input));
    assert!(text.chars().count() <= prompt::caps::CATALOG, "{} chars:\n{text}", text.chars().count());
    assert_eq!(text, prompt::catalog_text(&catalog(&index, &input)));
    for skill in &index.skills {
        assert!(text.contains(&format!("- {}: {}", skill.name, skill.description)), "{text}");
    }
}

#[test]
fn mentions_name_skills_of_the_index_once() {
    let index = shipped();
    assert_eq!(
        mention::mentioned(&index, "@screen-use then @dates#phrases, mail bob@example.com, @nobody and @screen-use again"),
        ["screen-use", "dates#phrases"]
    );
    assert!(mention::mentioned(&index, "no mention").is_empty());
    assert_eq!(tail_page("<context>\nnow 2026\npage /apps/system/Security?x=1 \"Sec\"\n</context>").as_deref(), Some("/apps/system/Security"));
}

mod flow {
    //! The index on the in-memory database with the run engine and the scripted fake: the files synced into rows,
    //! the catalog frozen in the prefix, a later skill only in the tail, loads recorded, `@` mentions.

    use std::sync::Arc;
    use std::time::Duration;

    use moqui_lib::object::Value;
    use serde_json::json;

    use super::super::*;
    use crate::context::{ExecutionContext, ExecutionContextFactory};
    use crate::llm::prompt::{build_tail, TailSpec};
    use crate::llm::provider::fake::{self, Step};
    use crate::llm::run::{start_run, wait_for, RunState, StartRequest};

    const USER: &str = "skill.tester";

    fn bed(name: &str) -> (ExecutionContext, Arc<ExecutionContextFactory>, EntityFacade, String) {
        let endpoint = format!("skills-{name}");
        let conf = format!(
            r#"<moqui-conf><llm-facade>
                <endpoint id="{endpoint}" protocol="fake"/>
                <model id="{endpoint}" endpoint="{endpoint}" name="fake-1" context-window="32768"/>
                <bind feature="assist" model="{endpoint}" reasoning="none"/>
            </llm-facade></moqui-conf>"#
        );
        let ctx = ExecutionContextFactory::test_context_in(
            &conf,
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        fake::clear(&endpoint);
        for (entity, fields) in [
            ("moqui.security.UserAccount", vec![("userId", USER), ("username", USER)]),
            (
                "moqui.agent.AgentDefinition",
                vec![("agentId", "assist"), ("agentName", "Assist"), ("featureId", "assist"), ("surfaces", "drawer")],
            ),
        ] {
            let mut row = facade.make_value(entity).unwrap();
            for (field, text) in fields {
                row.set(field, Value::string(text));
            }
            row.create(&facade).unwrap();
        }
        let factory = Arc::clone(ctx.factory());
        (ctx, factory, facade, endpoint)
    }

    fn tool(id: &str, name: &str, arguments: serde_json::Value) -> fake::Reply {
        fake::Reply::steps(vec![
            Step::Tool {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
                pieces: 1,
            },
            Step::Finish("tool_calls".to_string()),
        ])
    }

    fn turn(factory: &Arc<ExecutionContextFactory>, conversation: Option<&str>, text: &str, tail: String) -> String {
        let started = start_run(
            factory,
            &StartRequest {
                user_id: USER.to_string(),
                conversation_id: conversation.map(str::to_string),
                text: text.to_string(),
                tail: Some(tail),
                ..StartRequest::default()
            },
        )
        .unwrap();
        assert_eq!(wait_for(factory, &started.run_id, Duration::from_secs(15)).unwrap(), RunState::Completed);
        started.conversation_id
    }

    fn conversation_field(facade: &EntityFacade, id: &str, field: &str) -> Option<String> {
        let row = facade
            .find(crate::llm::run::CONVERSATION)
            .unwrap()
            .eq("conversationId", Value::string(id))
            .one()
            .unwrap()
            .unwrap();
        text_of(&row, field)
    }

    #[test]
    fn the_catalog_is_frozen_later_skills_go_in_the_tail_and_loads_and_mentions_are_recorded() {
        let (ctx, factory, facade, endpoint) = bed("flow");
        let _entered = ctx.enter();
        let audience = Audience {
            user_id: Some(USER),
            agent_id: Some("assist"),
            surface: Some(Surface::Drawer),
        };
        // the framework's files are synced into rows on first use, once
        let index = SkillIndex::for_audience(&factory, &facade, &audience);
        let dates = index.get("dates").expect("the framework's skills are in the index");
        assert!(!dates.skill_id.is_empty() && dates.provenance == Provenance::Framework);
        assert!(dates.location.starts_with("component://framework/skill/"), "{}", dates.location);
        let again = sync::sync(&factory, &facade).unwrap();
        assert_eq!((again.added, again.changed), (0, 0), "{again:?}");
        // the vue-sfc skill is synced too, and left out of the index of an agent that does not write Vue SFC canvases
        assert!(index.get("vue-sfc").is_none());
        assert_eq!(again.unchanged, index.skills.len() + 1);

        // turn 1: the model loads dates; the catalog is frozen with the conversation
        fake::script(&endpoint, vec![tool("c1", "skill", json!({"name": "dates"})), fake::Reply::text("Last month.")]);
        let tail = build_tail(&TailSpec::default());
        let conversation = turn(&factory, None, "what happened last month", tail);
        let listed = conversation_field(&facade, &conversation, "skillCatalog").unwrap();
        // after the guides of the tools it offers (write_ui's), relevance to the first message
        let first = listed.split(", ").find(|name| index.get(name).is_some_and(|skill| skill.unlocks.is_empty()));
        assert_eq!(first, Some("dates"), "{listed}");
        let slugs = loaded_in(&facade, &conversation, "dates").unwrap().unwrap();
        assert!(slugs.contains(&"phrases".to_string()), "the whole body came: {slugs:?}");
        assert_eq!(loaded_in(&facade, &conversation, "services").unwrap(), None);
        let refreshed = SkillIndex::for_audience(&factory, &facade, &audience);
        assert_eq!(refreshed.get("dates").unwrap().use_count, 1);

        // a site skill published after the conversation started: not in its catalog, so a later match is in the tail
        let mut row = facade.make_value(SKILL).unwrap();
        row.set_sequenced_id_primary(&facade).unwrap();
        skill_of("---\nname: vacation-requests\ndescription: Use when someone asks for time off.\naliases: [vacation, leave]\n---\nSteps.\n")
            .fill_row(&mut row);
        row.set("statusId", Value::string(PUBLISHED));
        row.set("provenance", Value::string("admin"));
        row.create(&facade).unwrap();
        let index = SkillIndex::for_audience(&factory, &facade, &audience);
        assert_eq!(tail_skills(&facade, &index, Some(&conversation), "book a vacation for Friday"), ["vacation-requests"]);
        assert!(tail_skills(&facade, &index, Some(&conversation), "dates of last month").is_empty(), "listed and loaded");
        assert!(tail_skills(&facade, &index, None, "book a vacation").is_empty(), "a first message has no tail skills");

        // turn 2 with that tail, and turn 3 with a mention: the prefix stays byte-identical
        fake::script(&endpoint, vec![fake::Reply::text("Booked."), fake::Reply::text("Dialogs.")]);
        let tail = build_tail(&TailSpec {
            skills: tail_skills(&facade, &index, Some(&conversation), "book a vacation for Friday"),
            ..TailSpec::default()
        });
        turn(&factory, Some(&conversation), "book a vacation for Friday", tail);
        turn(&factory, Some(&conversation), "@screen-use how do dialogs work", build_tail(&TailSpec::default()));
        let sent = fake::requests(&endpoint);
        assert_eq!(sent.len(), 4);
        for request in &sent[1..] {
            assert_eq!(request["system"], sent[0]["system"], "the catalog is frozen with the prefix");
        }
        assert!(!sent[0]["system"].as_str().unwrap().contains("vacation-requests"));
        let messages = sent[2]["messages"].as_array().unwrap();
        let last_user = messages.iter().rev().find(|message| message["role"] == "user").unwrap();
        assert!(last_user["text"].as_str().unwrap().contains("skills that may fit: vacation-requests"), "{last_user}");
        // the mention appended a skill call and its result after the message
        let messages = sent[3]["messages"].as_array().unwrap();
        let at = messages.iter().rposition(|message| message["role"] == "user").unwrap();
        assert!(messages[at]["text"].as_str().unwrap().starts_with("@screen-use how do dialogs work"));
        assert_eq!(messages[at + 1]["tool_calls"][0]["name"], "skill");
        assert_eq!(messages[at + 1]["tool_calls"][0]["arguments"], json!({"name": "screen-use"}));
        assert!(messages[at + 2]["text"].as_str().unwrap().starts_with("skill screen-use: Working on screens"));
        assert!(loaded_in(&facade, &conversation, "screen-use").unwrap().is_some());

        // a personal row shadows the file of its name for its owner only, until the file's row is locked
        let mut mine = facade.make_value(SKILL).unwrap();
        mine.set_sequenced_id_primary(&facade).unwrap();
        skill_of("---\nname: dates\ndescription: Use when I say a date.\n---\nMy way.\n").fill_row(&mut mine);
        mine.set("statusId", Value::string(PUBLISHED));
        mine.set("provenance", Value::string("user"));
        mine.set("ownerUserId", Value::string(USER));
        mine.create(&facade).unwrap();
        let body = |user: Option<&str>| {
            SkillIndex::for_audience(&factory, &facade, &Audience { user_id: user, ..audience })
                .get("dates")
                .unwrap()
                .body
                .clone()
        };
        assert_eq!(body(Some(USER)), "My way.");
        assert_ne!(body(Some("someone.else")), "My way.");
        let mut file_row = facade.find(SKILL).unwrap().eq("skillId", Value::string(&dates.skill_id)).one().unwrap().unwrap();
        file_row.set("locked", Value::string("Y"));
        file_row.update(&facade).unwrap();
        assert_ne!(body(Some(USER)), "My way.", "a locked skill is not shadowed");
        // and a sync keeps the admin's lock
        sync::sync(&factory, &facade).unwrap();
        assert_ne!(body(Some(USER)), "My way.");
    }

    fn skill_of(text: &str) -> Skill {
        parse_skill(text).unwrap()
    }
}
