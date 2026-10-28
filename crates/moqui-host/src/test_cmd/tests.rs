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
    for (bad, says) in [
        (
            &["--serve", "--db", "existing"][..],
            "--serve runs on a scratch runtime",
        ),
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
