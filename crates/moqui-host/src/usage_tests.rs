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

use super::{is_server_invocation, parse_server_args};

fn parse(args: &[&str]) -> super::ServerOptions {
    parse_server_args(args).unwrap_or_else(|err| panic!("{err}"))
}

fn message(args: &[&str]) -> String {
    parse_server_args(args)
        .expect_err("expected an error")
        .message()
        .to_string()
}

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().copied().map(str::to_string).collect()
}

#[test]
fn usage_port_forms_set_the_http_port() {
    let off = parse(&["--http-port=off"]);
    assert_eq!(off.value(super::HTTP_LISTEN_PORT), Some("off"));
    assert_eq!(off.value(super::HTTP_PORT), None);
    assert_eq!(parse(&["--http-port", "0"]).value(super::HTTP_LISTEN_PORT), Some("0"));
    assert_eq!(parse(&["--port", "8081"]).value(super::HTTP_PORT), Some("8081"));
    assert_eq!(parse(&["--port=8081"]).value(super::HTTP_PORT), Some("8081"));
    assert_eq!(parse(&["-port=8081"]).value(super::HTTP_PORT), Some("8081"));
    assert_eq!(parse(&["port=8081"]).value(super::HTTP_PORT), Some("8081"));
    assert_eq!(parse(&["-p", "9"]).value(super::HTTP_PORT), Some("9"));
    assert_eq!(parse(&["--port", "0"]).value(super::HTTP_PORT), Some("0"));
    assert_eq!(parse(&["port=08"]).value(super::HTTP_PORT), Some("8"));
    assert_eq!(parse(&["--port", " 8081 "]).value(super::HTTP_PORT), Some("8081"));
}

#[test]
fn usage_port_errors_name_the_problem() {
    let missing = message(&["--port"]);
    assert!(missing.contains("--port needs a port number"), "{missing}");
    assert!(missing.contains("moqui help"), "{missing}");
    let nope = message(&["--port", "nope"]);
    assert!(nope.contains("http-port nope is not a port"), "{nope}");
    let empty = message(&["port="]);
    assert!(empty.contains("http-port is not a port"), "{empty}");
    let too_big = message(&["port=65536"]);
    assert!(too_big.contains("http-port 65536 is not a port"), "{too_big}");
    let dashed = message(&["-p=8081"]);
    assert!(dashed.contains("unknown option '-p=8081'"), "{dashed}");
    let spaced = message(&["-port", "8081"]);
    assert!(spaced.contains("unknown option '-port'"), "{spaced}");
}

#[test]
fn usage_https_port_forms_set_the_https_listen_port() {
    let listen = super::HTTPS_LISTEN_PORT;
    assert_eq!(parse(&["--https-port", "9443"]).value(listen), Some("9443"));
    assert_eq!(parse(&["--https-port=9443"]).value(listen), Some("9443"));
    assert_eq!(parse(&["https-port=off"]).value(listen), Some("off"));
    assert_eq!(
        parse(&["--https-port", "9443"]).value("moqui_conf__webapp_list__webapp__WEBROOT__https_port"),
        None
    );
    let nope = message(&["--https-port", "false"]);
    assert!(nope.contains("https-listen-port false is not a port or off"), "{nope}");
    let missing = message(&["--https-port"]);
    assert!(missing.contains("--https-port needs a port number or off"), "{missing}");
}

#[test]
fn usage_conf_forms_set_moqui_conf() {
    let file = "conf/MoquiProductionConf.xml";
    assert_eq!(parse(&["--conf", file]).value("MOQUI_CONF"), Some(file));
    assert_eq!(
        parse(&["--conf=conf/MoquiProductionConf.xml"]).value("MOQUI_CONF"),
        Some(file)
    );
    assert_eq!(
        parse(&["-conf=conf/MoquiProductionConf.xml"]).value("MOQUI_CONF"),
        Some(file)
    );
    assert_eq!(
        parse(&["conf=conf/MoquiProductionConf.xml"]).value("MOQUI_CONF"),
        Some(file)
    );
    assert_eq!(parse(&["-c", file]).value("MOQUI_CONF"), Some(file));
    let missing = message(&["--conf"]);
    assert!(missing.contains("--conf needs a file"), "{missing}");
    let empty = message(&["conf="]);
    assert!(empty.contains("conf file is empty"), "{empty}");
    let dashed = message(&["-c=conf/MoquiProductionConf.xml"]);
    assert!(dashed.contains("unknown option"), "{dashed}");
    let other = message(&["confidential=x"]);
    assert!(other.contains("unknown option 'confidential=x'"), "{other}");
}

#[test]
fn usage_no_run_es_sets_opensearch_run_false() {
    assert_eq!(
        parse(&["no-run-es"]).value("moqui_conf__search_facade__cluster__run"),
        Some("false")
    );
    assert_eq!(
        parse(&["-no-run-es"]).value("moqui_conf__search_facade__cluster__run"),
        Some("false")
    );
    assert_eq!(
        parse(&["--no-run-es"]).value("moqui_conf__search_facade__cluster__run"),
        Some("false")
    );
}

#[test]
fn usage_create_or_update_is_opt_in() {
    assert_eq!(parse(&[]).value("create-or-update"), None);
    assert_eq!(
        parse(&["-D", "create-or-update=true"]).value("create-or-update"),
        Some("true")
    );
    assert_eq!(
        parse(&["-Dcreate-or-update=false"]).value("create-or-update"),
        Some("false")
    );
}

#[test]
fn usage_dash_d_records_underscore_properties() {
    assert_eq!(
        parse(&["-D", "entity_ds_db_conf=postgres"]).value("entity_ds_db_conf"),
        Some("postgres")
    );
    assert_eq!(
        parse(&["-Dentity_ds_db_conf=postgres"]).value("entity_ds_db_conf"),
        Some("postgres")
    );
    let dotted = message(&["-D", "entity.ds.db_conf=x"]);
    assert!(dotted.contains("entity_ds_db_conf"), "{dotted}");
    assert!(dotted.contains("underscore"), "{dotted}");
    let attached = message(&["-Dentity.ds.db_conf=x"]);
    assert!(attached.contains("entity_ds_db_conf"), "{attached}");
    let alone = message(&["-D"]);
    assert!(alone.contains("-D needs name=value"), "{alone}");
    let bare = message(&["-D", "entity_ds_db_conf"]);
    assert!(bare.contains("name=value"), "{bare}");
    let digit = message(&["-D", "1bad=x"]);
    assert!(digit.contains("not a valid property name"), "{digit}");
    let empty_name = message(&["-D", "=x"]);
    assert!(empty_name.contains("not a valid property name"), "{empty_name}");
    let opts = parse(&["-D", "entity_ds_db_conf=postgres", "-Dentity_ds_db_conf=mysql"]);
    assert_eq!(opts.value("entity_ds_db_conf"), Some("mysql"));
    assert_eq!(
        parse(&["-D", "entity_ds_db_conf="]).value("entity_ds_db_conf"),
        Some("")
    );
}

#[test]
fn usage_threads_is_an_unknown_option() {
    let dashed = message(&["--threads", "4"]);
    assert!(dashed.contains("unknown option '--threads'"), "{dashed}");
    let equals = message(&["threads=4"]);
    assert!(equals.contains("unknown option 'threads=4'"), "{equals}");
}

#[test]
fn usage_later_assignment_replaces_an_earlier_one() {
    let opts = parse(&["--port", "8081", "-p", "9", "port=8443"]);
    assert_eq!(opts.value(super::HTTP_PORT), Some("8443"));
    let cleared = parse(&[
        "--port",
        "8081",
        "-D",
        "moqui_conf__webapp_list__webapp__WEBROOT__http_port=",
    ]);
    assert_eq!(cleared.value(super::HTTP_PORT), Some(""));
    let restored = parse(&[
        "-D",
        "moqui_conf__webapp_list__webapp__WEBROOT__http_port=1",
        "--port",
        "8081",
    ]);
    assert_eq!(restored.value(super::HTTP_PORT), Some("8081"));
    let search = parse(&["--no-run-es", "-D", "moqui_conf__search_facade__cluster__run=true"]);
    assert_eq!(search.value("moqui_conf__search_facade__cluster__run"), Some("true"));
}

#[test]
fn usage_combined_server_options() {
    let opts = parse(&[
        "--port",
        "8081",
        "--conf",
        "conf/MoquiProductionConf.xml",
        "--no-run-es",
        "-D",
        "entity_ds_db_conf=postgres",
    ]);
    assert_eq!(opts.value(super::HTTP_PORT), Some("8081"));
    assert_eq!(opts.value("MOQUI_CONF"), Some("conf/MoquiProductionConf.xml"));
    assert_eq!(opts.value("moqui_conf__search_facade__cluster__run"), Some("false"));
    assert_eq!(opts.value("entity_ds_db_conf"), Some("postgres"));
    assert!(parse(&[]).value(super::HTTP_PORT).is_none());
}

#[test]
fn serve_is_a_command_and_its_options_are_the_servers() {
    let page = super::command_help("serve").expect("serve help");
    assert!(page.contains("moqui serve"), "{page}");
    assert!(page.contains("moqui.log"), "{page}");
    assert!(page.contains("search log indexes"), "{page}");
    assert!(!is_server_invocation(&owned(&["serve"])));
    assert!(!is_server_invocation(&owned(&["serve", "--port", "8081"])));
    assert_eq!(parse(&["--port", "8081"]).value(super::HTTP_PORT), Some("8081"));
    let unknown = message(&["--bogus"]);
    assert!(unknown.contains("unknown option '--bogus'"), "{unknown}");
    assert!(super::command_usage("serve")
        .message()
        .contains("moqui serve [options]"));
}

#[test]
fn usage_server_invocation_is_options_not_commands() {
    assert!(is_server_invocation(&[]));
    assert!(is_server_invocation(&owned(&["--port", "8081"])));
    assert!(is_server_invocation(&owned(&["threads=4"])));
    assert!(is_server_invocation(&owned(&["no-run-es"])));
    assert!(is_server_invocation(&owned(&["foo=bar"])));
    assert!(!is_server_invocation(&owned(&["nope"])));
    assert!(!is_server_invocation(&owned(&["test"])));
    assert!(!is_server_invocation(&owned(&["test", "--spec", "Basics"])));
    assert!(!is_server_invocation(&owned(&["load", "types=seed"])));
    assert!(!is_server_invocation(&owned(&["load", "--conf", "x"])));
}
