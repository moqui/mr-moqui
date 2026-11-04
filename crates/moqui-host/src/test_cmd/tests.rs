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

fn spec(name: &str, source: &str) -> SpecFile {
    SpecFile {
        name: name.to_string(),
        display: format!("c/test/{name}.spec.js"),
        source: source.to_string(),
    }
}

#[test]
fn args_parse() {
    let args: Vec<String> = [
        "comp-a", "--spec", "A,B", "--db", "snapshot", "--junit", "out", "--keep",
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect();
    let parsed = parse_test_args(&args).unwrap();
    assert_eq!(parsed.components, vec!["comp-a".to_string()]);
    assert_eq!(parsed.specs, vec!["A".to_string(), "B".to_string()]);
    assert_eq!(parsed.db, DbMode::Snapshot);
    assert!(parsed.db_explicit);
    assert_eq!(parsed.junit, Some(PathBuf::from("out")));
    assert!(parsed.keep);
    assert!(!parsed.acceptance);
    assert!(parse_test_args(&["--acceptance".to_string()]).unwrap().acceptance);
    assert!(parse_test_args(&["--slow".to_string()]).is_err());
    assert!(parse_test_args(&["--db".to_string(), "nope".to_string()]).is_err());
    assert!(parse_test_args(&["--bogus".to_string()]).is_err());
    let none = parse_test_args(&["--db".to_string(), "none".to_string()]).unwrap();
    assert_eq!(none.db, DbMode::None);
    assert!(none.db_explicit);
    let implicit = parse_test_args(&["comp-a".to_string()]).unwrap();
    assert_eq!(implicit.db, DbMode::Fresh);
    assert!(!implicit.db_explicit);
}

#[test]
fn a_database_free_selection_uses_none_unless_the_user_asked_for_a_database() {
    let component = |source: &str| TestComponent {
        name: "framework".to_string(),
        dir: PathBuf::from("framework"),
        specs: vec![spec("ScreenHtml", source)],
    };
    let free = component("spec({ database: false })");
    let needs = component("spec({})");
    let mut plain = parse_test_args(&["framework".to_string()]).unwrap();
    assert_eq!(
        database_mode(&plain, std::slice::from_ref(&free)).unwrap(),
        DbMode::None
    );
    assert_eq!(
        database_mode(&plain, std::slice::from_ref(&needs)).unwrap(),
        DbMode::Fresh
    );
    plain.db_explicit = true;
    plain.db = DbMode::Fresh;
    assert_eq!(
        database_mode(&plain, std::slice::from_ref(&free)).unwrap(),
        DbMode::Fresh
    );
    let none = parse_test_args(&["--db".to_string(), "none".to_string()]).unwrap();
    let err = database_mode(&none, &[needs]).unwrap_err().to_string();
    assert!(
        err.contains("framework/ScreenHtml") && err.contains("database: false"),
        "{err}"
    );
    assert_eq!(database_mode(&none, std::slice::from_ref(&free)).unwrap(), DbMode::None);
    let mut coverage = none.clone();
    coverage.coverage = true;
    assert!(database_mode(&coverage, &[free])
        .unwrap_err()
        .to_string()
        .contains("--coverage"));
    let serving = parse_test_args(&["--serve".to_string()]).unwrap();
    let served = component("spec({ database: false })");
    assert_eq!(database_mode(&serving, &[served]).unwrap(), DbMode::Fresh);
}

#[test]
fn agent_args_parse() {
    let args: Vec<String> = [
        "tools",
        "--agents",
        "--llm-profile",
        "local-qwen",
        "--runs",
        "3",
        "--seed",
        "7",
        "--min-pass",
        "0.5",
        "--write-baseline",
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect();
    let parsed = parse_test_args(&args).unwrap();
    assert!(parsed.agents && parsed.write_baseline);
    assert_eq!(parsed.llm_profile.as_deref(), Some("local-qwen"));
    assert_eq!((parsed.runs, parsed.seed, parsed.min_pass), (3, Some(7), Some(0.5)));
    let replay = parse_test_args(&["--agents".to_string(), "--replay".to_string(), "dir".to_string()]).unwrap();
    assert_eq!(replay.replay, Some(PathBuf::from("dir")));
    assert_eq!(replay.runs, 1);
    for bad in [["--runs", "0"], ["--seed", "x"], ["--min-pass", "2"]] {
        let bad: Vec<String> = bad.iter().map(|arg| (*arg).to_string()).collect();
        assert!(parse_test_args(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn spec_selection_adds_depends_on_in_suite_order() {
    let component = TestComponent {
        name: "c".to_string(),
        dir: PathBuf::from("c"),
        specs: vec![
            spec("First", "spec({})"),
            spec("Second", "spec({ dependsOn: ['First'] })"),
            spec("Other", "spec({})"),
            spec("Third", "spec({ dependsOn: ['Second'] })"),
        ],
    };
    let picked = select_specs(&component, &["Third".to_string()]).unwrap();
    let names: Vec<&str> = picked.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, vec!["First", "Second", "Third"]);
    assert!(select_specs(&component, &["Missing".to_string()]).is_err());
}

#[test]
fn depends_on_is_found_in_the_other_components_of_the_run() {
    let component = |name: &str, specs: Vec<SpecFile>| TestComponent {
        name: name.to_string(),
        dir: PathBuf::from(name),
        specs,
    };
    let components = vec![
        component(
            "mantle",
            vec![
                spec("Flow", "spec({})"),
                spec("Shared", "spec({})"),
                spec("Other", "spec({})"),
            ],
        ),
        component(
            "screens",
            vec![
                spec("Shared", "spec({})"),
                spec("Render", "spec({ dependsOn: ['Flow', 'Shared'] })"),
            ],
        ),
    ];
    let picked = select_across(&components, &["Render".to_string()]).unwrap();
    let names: Vec<(&str, Vec<&str>)> = picked
        .iter()
        .map(|component| {
            (
                component.name.as_str(),
                component.specs.iter().map(|spec| spec.name.as_str()).collect(),
            )
        })
        .collect();
    // Flow from mantle, which screens lacks; Shared from screens' own
    assert_eq!(
        names,
        vec![("mantle", vec!["Flow"]), ("screens", vec!["Shared", "Render"])]
    );
    let alone = select_across(&components[1..], &["Render".to_string()]).unwrap_err();
    assert!(
        alone
            .to_string()
            .contains("screens/Render depends on Flow, which is not a spec of screens"),
        "{alone}"
    );
    // a name in two components runs in both
    let both = select_across(&components, &["Shared".to_string()]).unwrap();
    assert_eq!(both.len(), 2);
}

#[test]
fn serve_args_parse() {
    let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_string()).collect::<Vec<String>>();
    let parsed = parse_test_args(&args(&["--serve", "comp-a", "--ready-file", "r.url"])).unwrap();
    assert_eq!(parsed.serve, Some("127.0.0.1:0".parse().unwrap()));
    assert_eq!(parsed.components, vec!["comp-a".to_string()]);
    assert_eq!(parsed.ready_file, Some(PathBuf::from("r.url")));
    let parsed = parse_test_args(&args(&["comp-a", "--serve", "0.0.0.0:18081"])).unwrap();
    assert_eq!(parsed.serve, Some("0.0.0.0:18081".parse().unwrap()));
    let parsed = parse_test_args(&args(&["--serve", "18081"])).unwrap();
    assert_eq!(parsed.serve, Some("127.0.0.1:18081".parse().unwrap()));
    // a runtime whose database is already in place (copied in, or loaded by `moqui load`) is served as it is
    let parsed = parse_test_args(&args(&["--serve", "--db", "existing"])).unwrap();
    assert_eq!(parsed.db, DbMode::Existing);
    for (bad, says) in [
        (&["--db", "none", "--serve"][..], "--db none does not go with --serve"),
        (&["--serve", "--db", "none"][..], "--db none does not go with --serve"),
        (&["--ready-file", "r"][..], "--ready-file goes with --serve"),
        (&["--serve", "--agents"][..], "does not go with --agents"),
    ] {
        let err = parse_test_args(&args(bad)).unwrap_err().to_string();
        assert!(err.contains(says), "{bad:?}: {err}");
    }
}

#[test]
fn suite_order_comes_first() {
    let dir = std::env::temp_dir().join(format!("moqui-test-discover-{}", std::process::id()));
    let test_dir = dir.join("test");
    std::fs::create_dir_all(&test_dir).unwrap();
    for name in ["A", "B", "C"] {
        std::fs::write(test_dir.join(format!("{name}.spec.js")), "spec({})").unwrap();
    }
    std::fs::write(test_dir.join("suite.js"), "suite(['C', 'A'])").unwrap();
    let specs = component_specs("c", &test_dir, "test").unwrap();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, vec!["C", "A", "B"]);
    assert_eq!(specs[0].display, "c/test/C.spec.js");
    std::fs::write(test_dir.join("suite.js"), "suite(['Nope'])").unwrap();
    assert!(component_specs("c", &test_dir, "test").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn specs_in_subdirectories() {
    let dir = std::env::temp_dir().join(format!("moqui-test-discover-sub-{}", std::process::id()));
    let test_dir = dir.join("test");
    for sub in ["order", "account/deep", "client/Orders", "client/process", "agent"] {
        std::fs::create_dir_all(test_dir.join(sub)).unwrap();
    }
    std::fs::write(test_dir.join("A.spec.js"), "spec({})").unwrap();
    std::fs::write(test_dir.join("order/C.spec.js"), "spec({})").unwrap();
    std::fs::write(test_dir.join("account/deep/B.spec.js"), "spec({})").unwrap();
    std::fs::write(test_dir.join("client/Orders/Walk.spec.js"), "spec({})").unwrap();
    std::fs::write(test_dir.join("client/process/Flow.spec.js"), "spec({})").unwrap();
    std::fs::write(test_dir.join("agent/Tasks.spec.js"), "spec({})").unwrap();
    std::fs::write(test_dir.join("suite.js"), "suite(['C'])").unwrap();
    // test: the subdirectories, not client or agent; suite order, then by name wherever the file is
    let specs = component_specs("c", &test_dir, "test").unwrap();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, vec!["C", "A", "B"]);
    assert_eq!(specs[0].display, "c/test/order/C.spec.js");
    assert_eq!(specs[2].display, "c/test/account/deep/B.spec.js");
    // test/client: its subdirectories, not process
    let client = component_specs("c", &test_dir.join("client"), "test/client").unwrap();
    let names: Vec<&str> = client.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, vec!["Walk"]);
    assert_eq!(client[0].display, "c/test/client/Orders/Walk.spec.js");
    // one name in two directories
    std::fs::write(test_dir.join("account/C.spec.js"), "spec({})").unwrap();
    let err = component_specs("c", &test_dir, "test").unwrap_err().to_string();
    assert!(
        err.contains("c/test/order/C.spec.js") && err.contains("c/test/account/C.spec.js"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The acceptance suite is a tree of its own: a regression run never sees it; an acceptance run takes only its specs,
/// with the regression specs in the pool `--spec` and `dependsOn` resolve against; a name is unique across both.
#[test]
fn the_acceptance_tree() {
    let runtime = std::env::temp_dir().join(format!("moqui-test-acceptance-{}", std::process::id()));
    let comp = runtime.join("component/comp-a");
    for sub in [
        "test/client",
        "acceptance/client",
        "acceptance/walks",
        "acceptance/agent",
    ] {
        std::fs::create_dir_all(comp.join(sub)).unwrap();
    }
    std::fs::write(comp.join("component.xml"), "<component name=\"comp-a\"/>").unwrap();
    std::fs::write(comp.join("test/Rule.spec.js"), "spec({})").unwrap();
    std::fs::write(comp.join("test/Fixtures.spec.js"), "spec({})").unwrap();
    std::fs::write(comp.join("test/client/Golden.spec.js"), "spec({})").unwrap();
    std::fs::write(
        comp.join("acceptance/Sweep.spec.js"),
        "spec({ dependsOn: ['Fixtures'] })",
    )
    .unwrap();
    std::fs::write(comp.join("acceptance/walks/Scenario.spec.js"), "spec({})").unwrap();
    std::fs::write(comp.join("acceptance/agent/Task.spec.js"), "spec({})").unwrap();
    std::fs::write(comp.join("acceptance/client/Walk.spec.js"), "spec({})").unwrap();
    let names = |components: &[TestComponent]| -> Vec<String> {
        components
            .iter()
            .flat_map(|component| component.specs.iter().map(|spec| spec.name.clone()))
            .collect()
    };
    // the regression suite never reads acceptance/
    let regression = discover_suite(&runtime, false, false).unwrap();
    assert_eq!(names(&regression), vec!["Fixtures", "Rule"]);
    // the acceptance pool: the regression specs, then the acceptance ones; a run takes only these
    let pool = discover_suite(&runtime, false, true).unwrap();
    assert_eq!(names(&pool), vec!["Fixtures", "Rule", "Scenario", "Sweep"]);
    assert_eq!(names(&acceptance_only(pool.clone())), vec!["Scenario", "Sweep"]);
    assert!(is_acceptance(&pool[0].specs[3]));
    assert_eq!(pool[0].specs[2].display, "comp-a/acceptance/walks/Scenario.spec.js");
    // --spec resolves dependsOn into the regression tree
    assert_eq!(
        names(&select_across(&pool, &["Sweep".to_string()]).unwrap()),
        vec!["Fixtures", "Sweep"]
    );
    // client
    assert_eq!(names(&discover_suite(&runtime, true, false).unwrap()), vec!["Golden"]);
    assert_eq!(
        names(&acceptance_only(discover_suite(&runtime, true, true).unwrap())),
        vec!["Walk"]
    );
    // one name in both trees
    std::fs::write(comp.join("acceptance/Rule.spec.js"), "spec({})").unwrap();
    let err = discover_suite(&runtime, false, true).unwrap_err().to_string();
    assert!(
        err.contains("comp-a/acceptance/Rule.spec.js") && err.contains("comp-a/test/Rule.spec.js"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(runtime);
}

/// A fixture spec runs where a served suite names it: a plain run leaves it out, unless a spec that stays depends on it.
#[test]
fn a_served_fixture_runs_only_where_it_is_served() {
    let dir = std::env::temp_dir().join(format!("moqui-test-fixtures-once-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("test/client")).unwrap();
    std::fs::write(
        dir.join("test/client/suite.js"),
        "suite(['Walk'], { server: { components: ['c'], specs: ['WalkFixtures', 'SharedFixtures', 'OtherFlow'] } })",
    )
    .unwrap();
    let mut components = vec![TestComponent {
        name: "c".to_string(),
        dir: dir.clone(),
        specs: vec![
            spec("Rule", "spec({ dependsOn: ['SharedFixtures'] })"),
            spec("WalkFixtures", "spec({})"),
            spec("SharedFixtures", "spec({})"),
        ],
    }];
    let left_out = drop_served_fixtures(&mut components);
    assert_eq!(left_out, vec!["c/WalkFixtures".to_string()]);
    let names: Vec<&str> = components[0].specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(names, vec!["Rule", "SharedFixtures"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The units of a run on workers: a suite.js chain is one unit and draws in what its specs depend on; a spec joins the
/// unit of what it depends on, across components; every other spec is its own. They are dealt longest first.
#[cfg(feature = "embedded-postgres")]
#[test]
fn worker_units_follow_the_suite_chain_and_depends_on() {
    let dir = std::env::temp_dir().join(format!("moqui-test-units-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("a/test")).unwrap();
    std::fs::write(dir.join("a/test/suite.js"), "suite(['Flow1', 'Flow2'])").unwrap();
    let components = vec![
        TestComponent {
            name: "a".to_string(),
            dir: dir.join("a"),
            specs: vec![
                spec("Flow1", "spec({})"),
                spec("Flow2", "spec({})"),
                spec("Report", "spec({ dependsOn: ['Flow2'] })"),
                spec("Alone", "spec({})"),
            ],
        },
        TestComponent {
            name: "b".to_string(),
            dir: dir.join("b"),
            specs: vec![
                spec("Screens", "spec({ dependsOn: ['Flow1'] })"),
                spec("Rule", "spec({})"),
            ],
        },
    ];
    let units = workers::units(&components, false);
    assert_eq!(
        workers::unit_names(&components, &units),
        vec![vec!["Flow1", "Flow2", "Report", "Screens"], vec!["Alone"], vec!["Rule"]]
    );
    let bins = workers::deal(units, 2);
    assert_eq!(bins.len(), 2);
    assert_eq!(bins[0].len(), 1);
    assert_eq!(bins[0][0].specs.len(), 4);
    let args = parse_test_args(&["--workers".to_string(), "4".to_string()]).unwrap();
    assert_eq!(args.workers, 4);
    let worker = workers::worker_args(&args, &components, &bins[0], Path::new("/tmp/w.json")).join(" ");
    assert_eq!(
        worker,
        "test a b --spec a/Flow1,a/Flow2,a/Report,b/Screens --db snapshot --results-json /tmp/w.json"
    );
    assert!(parse_test_args(&["--workers".to_string(), "0".to_string()]).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
