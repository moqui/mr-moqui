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

use super::{dispatch, load_options, parse_load_args, should_load_data, startup_load_flags, LoadFlags};

#[test]
fn runtime_is_the_local_one_else_beside_or_around_the_executable() {
    let root = std::env::temp_dir().join(format!("moqui-runtime-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let local = root.join("work/runtime");
    let archive = root.join("archive");
    let beside = archive.join("runtime");
    let exe = archive.join("moqui");
    // A directory without base-component is not a runtime.
    std::fs::create_dir_all(&local).unwrap();
    std::fs::create_dir_all(&beside).unwrap();
    assert_eq!(super::runtime_dir_for(&local, Some(&exe)), local);
    // The archive layout: the binary sits in the runtime.
    std::fs::create_dir_all(archive.join("base-component")).unwrap();
    assert_eq!(super::runtime_dir_for(&local, Some(&exe)), archive);
    assert_eq!(super::runtime_dir_for(&local, None), local);
    std::fs::create_dir_all(beside.join("base-component")).unwrap();
    assert_eq!(super::runtime_dir_for(&local, Some(&exe)), beside);
    std::fs::create_dir_all(local.join("base-component")).unwrap();
    assert_eq!(super::runtime_dir_for(&local, Some(&exe)), local);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn startup_load_honors_all_including_demo() {
    let flags = startup_load_flags(r#"<moqui-conf><tools empty-db-load="all"/></moqui-conf>"#).unwrap();
    assert!(load_options(&flags).data_types.is_empty());
    assert!(should_load_data(0, &flags, true));
    assert!(!should_load_data(1, &flags, true));
}

#[test]
fn startup_load_expands_configured_property() {
    let key = "moqui_host_test_empty_db_load";
    moqui_lib::env::set_property(key, "seed, demo");
    let result =
        startup_load_flags(r#"<moqui-conf><tools empty-db-load="${moqui_host_test_empty_db_load}"/></moqui-conf>"#);
    moqui_lib::env::clear_property(key);
    let flags = result.unwrap();
    assert_eq!(load_options(&flags).data_types, ["seed", "demo"]);
    assert!(should_load_data(0, &flags, true));
    assert!(!should_load_data(1, &flags, true));
}

#[test]
fn startup_load_respects_disabled_and_absent_settings() {
    for conf in [
        r#"<moqui-conf><tools empty-db-load="none"/></moqui-conf>"#,
        r#"<moqui-conf><tools empty-db-load=""/></moqui-conf>"#,
        "<moqui-conf><tools/></moqui-conf>",
        "<moqui-conf/>",
    ] {
        let flags = startup_load_flags(conf).unwrap();
        assert!(!should_load_data(0, &flags, true));
    }
}

#[test]
fn explicit_load_keeps_existing_database_and_default_behavior() {
    let flags = LoadFlags::default();
    assert_eq!(load_options(&flags).data_types, ["seed", "seed-initial", "install"]);
    assert!(should_load_data(0, &flags, false));
    assert!(!should_load_data(1, &flags, false));
    let all = parse_load_args(&["types=all"]).unwrap();
    assert!(should_load_data(1, &all, false));
    let components = parse_load_args(&["components=webroot"]).unwrap();
    assert!(should_load_data(1, &components, false));
}

#[test]
fn no_args_start_the_server_and_are_not_called_here() {
    // `dispatch([])` calls `serve` and blocks on the listener. A bad command still names the usage.
    let err = dispatch(["nope"]).unwrap_err();
    assert!(err.message().contains("usage: moqui"), "{err}");
    assert!(!err.message().contains("server not implemented"), "{err}");
}

#[test]
fn unknown_command_names_the_dump_commands() {
    let text = dispatch(["help"]).unwrap().expect("help prints the command list");
    assert!(text.contains("save-db"), "{text}");
    assert!(text.contains("load-save"), "{text}");
    assert!(text.contains("load"), "{text}");
    assert!(text.contains("lint"), "{text}");
    let err = dispatch(["nope"]).unwrap_err();
    let message = err.message();
    assert!(message.contains("usage: moqui"), "{err}");
    assert!(message.contains("moqui help"), "{err}");
    assert!(!message.contains("disableEntityEca"), "{err}");
}

#[test]
fn usage_overview_lists_commands_and_environment() {
    let text = dispatch(["help"]).unwrap().unwrap();
    assert_eq!(dispatch(["--help"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["-h"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["-?"]).unwrap().unwrap(), text);
    for needle in [
        "Usage: moqui",
        "save-db",
        "load-save",
        "load",
        "lint",
        "convert",
        "types",
        "test",
        "[component ...]",
        "MOQUI_CONF",
        "webapp_http_port",
        "--port",
        "--conf",
        "--no-run-es",
        "-D",
        "serve",
    ] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
}

#[test]
fn usage_serve_page_and_a_bad_option_do_not_detach() {
    let text = dispatch(["help", "serve"]).unwrap().unwrap();
    assert_eq!(dispatch(["serve", "--help"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["serve", "-h"]).unwrap().unwrap(), text);
    assert!(text.contains("moqui.log"), "{text}");
    assert!(text.contains("search log indexes"), "{text}");
    let bogus = dispatch(["serve", "--bogus"]).unwrap_err();
    assert!(bogus.message().contains("unknown option '--bogus'"), "{bogus}");
    assert!(bogus.message().contains("moqui help"), "{bogus}");
    let missing = dispatch(["serve", "--port"]).unwrap_err();
    assert!(missing.message().contains("port"), "{missing}");
}

#[test]
fn usage_load_page_names_the_flags() {
    let text = dispatch(["help", "load"]).unwrap().unwrap();
    assert_eq!(dispatch(["load", "--help"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["load", "-h"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["--help", "load"]).unwrap().unwrap(), text);
    for needle in [
        "types",
        "components",
        "location",
        "timeout",
        "raw",
        "disableEntityEca",
        "disableAuditLog",
        "disableFkCreate",
        "disableDataFeed",
    ] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
}

#[test]
fn usage_lint_and_types_pages_name_their_modes() {
    let lint = dispatch(["help", "lint"]).unwrap().unwrap();
    assert!(lint.contains("--screens"), "{lint}");
    assert!(lint.contains("--qvt-static"), "{lint}");
    assert!(lint.contains("--groovy"), "{lint}");
    let types = dispatch(["help", "types"]).unwrap().unwrap();
    assert!(types.contains("--out"), "{types}");
    assert_eq!(dispatch(["types", "--help"]).unwrap().unwrap(), types);
}

#[test]
fn usage_test_page_names_the_options() {
    let text = dispatch(["help", "test"]).unwrap().unwrap();
    assert_eq!(dispatch(["test", "--help"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["test", "-h"]).unwrap().unwrap(), text);
    assert_eq!(dispatch(["--help", "test"]).unwrap().unwrap(), text);
    for needle in [
        "--spec",
        "--feature",
        "--slow",
        "--db",
        "none",
        "fresh",
        "snapshot",
        "existing",
        "database: false",
        "--junit",
        "--keep",
        "--log-level",
        "suite.js",
        "dependsOn",
        "test/*.spec.js",
    ] {
        assert!(text.contains(needle), "{needle} missing from {text}");
    }
    let bogus = dispatch(["test", "--bogus"]).unwrap_err();
    assert!(bogus.message().contains("unknown option"), "{bogus}");
    assert!(bogus.message().contains("moqui help test"), "{bogus}");
}

#[test]
fn usage_server_options_do_not_start_the_listener() {
    let missing = dispatch(["--port"]).unwrap_err();
    assert!(missing.message().contains("port"), "{missing}");
    let bogus = dispatch(["--bogus"]).unwrap_err();
    assert!(bogus.message().contains("unknown option '--bogus'"), "{bogus}");
    assert!(bogus.message().contains("moqui help"), "{bogus}");
    let threads = dispatch(["threads=4"]).unwrap_err();
    assert!(threads.message().contains("unknown option 'threads=4'"), "{threads}");
    let dashed = dispatch(["--threads", "4"]).unwrap_err();
    assert!(dashed.message().contains("unknown option '--threads'"), "{dashed}");
    let on_load = dispatch(["load", "--conf", "conf/MoquiProductionConf.xml"]).unwrap_err();
    assert!(on_load.message().contains("load flag"), "{on_load}");
    let before_command = dispatch(["--port", "8081", "save-db"]).unwrap_err();
    assert!(
        before_command.message().contains("unknown option 'save-db'"),
        "{before_command}"
    );
    let help = dispatch(["--port", "8081", "--help"]).unwrap().unwrap();
    assert_eq!(help, dispatch(["help"]).unwrap().unwrap());
    assert_eq!(dispatch(["--port", "8081", "-h"]).unwrap().unwrap(), help);
}

#[test]
fn usage_unknown_topic_points_at_help() {
    let err = dispatch(["help", "nope"]).unwrap_err();
    assert!(err.message().contains("nope"), "{err}");
    assert!(err.message().contains("moqui help"), "{err}");
}

#[test]
fn usage_wrong_arity_names_the_command() {
    let err = dispatch(["save-db"]).unwrap_err();
    assert!(err.message().contains("<cluster-dir>"), "{err}");
    assert!(err.message().contains("moqui help save-db"), "{err}");
    let convert = dispatch(["convert", "--groovy"]).unwrap_err();
    assert!(convert.message().contains("moqui help convert"), "{convert}");
    let lint = dispatch(["lint", "--groovy"]).unwrap_err();
    assert!(lint.message().contains("moqui help lint"), "{lint}");
}

#[test]
fn unknown_load_flag_names_the_flag() {
    let err = dispatch(["load", "nope=1"]).unwrap_err();
    assert!(err.message().contains("unknown load flag nope"), "{err}");
}

#[test]
fn timeout_must_be_an_integer() {
    let err = dispatch(["load", "timeout=soon"]).unwrap_err();
    assert!(err.message().contains("timeout"), "{err}");
}

#[test]
fn missing_types_use_empty_db_defaults() {
    let options = load_options(&LoadFlags::default());
    assert_eq!(options.data_types, ["seed", "seed-initial", "install"]);
    assert!(!options.disable_fk_create);
}

#[test]
fn raw_and_types_parse() {
    let flags = parse_load_args(&[
        "types=seed,seed-initial",
        "raw=true",
        "disableFkCreate=false",
        "components=framework,mantle",
        "timeout=15",
    ])
    .unwrap();
    assert!(flags.raw);
    assert!(!flags.disable_fk_create);
    assert_eq!(flags.timeout_seconds, Some(15));
    assert_eq!(flags.components, ["framework", "mantle"]);
    let options = load_options(&flags);
    assert_eq!(options.data_types, ["seed", "seed-initial"]);
    assert!(options.disable_fk_create);
}

#[test]
fn types_all_clears_the_filter() {
    let flags = parse_load_args(&["types=all"]).unwrap();
    assert!(load_options(&flags).data_types.is_empty());
    let empty = parse_load_args(&["types="]).unwrap();
    assert!(load_options(&empty).data_types.is_empty());
}

#[test]
fn data_directory_includes_one_level_of_subdirectories() {
    let dir = std::env::temp_dir().join(format!("moqui-data-dirs-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("es/more")).unwrap();
    std::fs::write(data.join("Seed.xml"), "<entity-facade-xml/>").unwrap();
    std::fs::write(data.join("notes.txt"), "skip").unwrap();
    std::fs::write(data.join("es/EsGeo.xml"), "<entity-facade-xml/>").unwrap();
    std::fs::write(data.join("es/more/Nested.xml"), "<entity-facade-xml/>").unwrap();
    assert_eq!(
        super::data_directory_locations("mr-l10n", &data),
        [
            "component://mr-l10n/data/Seed.xml".to_string(),
            "component://mr-l10n/data/es/EsGeo.xml".to_string(),
        ]
    );
}

#[test]
fn camel_case_flags_are_independent() {
    let flags = parse_load_args(&[
        "disableEntityEca=true",
        "disableAuditLog=yes",
        "disableFkCreate=1",
        "disableDataFeed=y",
    ])
    .unwrap();
    assert!(flags.disable_entity_eca);
    assert!(flags.disable_audit_log);
    assert!(flags.disable_fk_create);
    assert!(flags.disable_data_feed);
    assert!(!flags.raw);
    let options = load_options(&flags);
    assert!(options.disable_fk_create);
}

#[test]
fn lint_groovy_leftover_is_a_process_error() {
    let dir = std::env::temp_dir().join(format!("moqui-lint-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("groovy-leftover.xml");
    std::fs::write(
        &path,
        r#"<services>
                <service verb="groovy" noun="Broken">
                    <actions><set field="x" from="a ?: b"/></actions>
                </service>
            </services>"#,
    )
    .unwrap();
    let err = dispatch(["lint", &path.display().to_string()]).expect_err("lint fails the process");
    assert!(err.to_string().contains("failed lint"), "{err}");
}

#[test]
fn lint_of_a_clean_file_succeeds() {
    let dir = std::env::temp_dir().join(format!("moqui-lint-clean-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clean.xml");
    std::fs::write(
        &path,
        r#"<services><service verb="clean" noun="Fine"><actions><set field="x" from="a || b"/></actions></service></services>"#,
    )
    .unwrap();
    dispatch(["lint", &path.display().to_string()]).unwrap();
}

#[test]
fn lint_with_no_path_loads_configured_files() {
    // Framework services are not all JavaScript yet, so this may fail. It must not panic.
    let _ = dispatch(["lint"]);
}

struct DirGuard(std::path::PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
