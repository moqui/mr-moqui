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

//! The pure parts of the group server connection: the addresses a token goes to, the remotes a server owns, the
//! `myaddons.xml` edits, and the REST contract of each kind of server: the framework's and an application's (Smithy's
//! operations under its `restBase`, plans/design-dev-aide.md 21, mr-pass design-smithy §15.5). The calls against a
//! server are the host's integration test (`crates/moqui-host/tests/it/devaide_group.rs`), with the instance as its own
//! group server; a Smithy server is checked here by its contract only, as mr-pass and mr-mantle do not load there.

use serde_json::json;

use super::client::{first_list, normalize_base, ssh_host_port, Application, Server};
use super::connect::{repository_name, with_repository, without_repository};
use super::review::{create_call, fork_call, list_call, merge_request_item, propose_args, via, Opening, Via};

fn server(offer: serde_json::Value) -> Server {
    Server {
        id: "S1".into(),
        label: "Team".into(),
        base: "https://team.example.com".into(),
        user_id: "u1".into(),
        username: "ann".into(),
        offer,
        token: Some("k".into()),
    }
}

#[test]
fn a_servers_address_is_https_or_this_machine() {
    assert_eq!(
        normalize_base("https://Team.Example.com/").unwrap(),
        "https://team.example.com"
    );
    assert_eq!(
        normalize_base("https://h.example.com:8443/moqui").unwrap(),
        "https://h.example.com:8443/moqui"
    );
    assert_eq!(
        normalize_base("http://localhost:8080").unwrap(),
        "http://localhost:8080"
    );
    for bad in [
        "http://team.example.com",
        "team.example.com",
        "https://u@h.example.com",
        "https://h.example.com/?x=1",
    ] {
        assert!(normalize_base(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_token_goes_only_to_the_servers_own_address() {
    let server = server(json!({}));
    assert_eq!(
        server.url("/rest/s1/moqui/hosting/offer").unwrap(),
        "https://team.example.com/rest/s1/moqui/hosting/offer"
    );
    assert!(server.url("https://team.example.com/x").is_ok());
    assert!(server.url("https://evil.example.com/rest").is_err());
    assert!(server.url("http://team.example.com/rest").is_err());
}

#[test]
fn a_remote_is_the_servers_by_its_https_address_or_its_ssh_host_and_port() {
    let server = server(json!({ "git": { "sshHost": "team.example.com", "sshPort": "2222" } }));
    assert!(server.matches_remote("https://team.example.com/git/acme-site.git"));
    assert!(server.matches_remote("https://ann@team.example.com/git/acme-site.git"));
    assert!(server.matches_remote("ssh://ann@team.example.com:2222/acme-site.git"));
    assert!(!server.matches_remote("ssh://ann@team.example.com/acme-site.git"));
    assert!(!server.matches_remote("https://github.com/moqui/mr-moqui.git"));
    assert!(!server.matches_remote("git@github.com:moqui/mr-moqui.git"));
    assert_eq!(
        ssh_host_port("git@Host.example.com:a/b.git"),
        Some(("host.example.com".into(), "22".into()))
    );
    assert_eq!(
        ssh_host_port("ssh://u@[::1]:2222/a.git"),
        Some(("::1".into(), "2222".into()))
    );
    assert_eq!(ssh_host_port("https://h/a.git"), None);
}

#[test]
fn connect_writes_one_repository_into_myaddons() {
    let url = "https://team.example.com/git/${name}.git";
    let made = with_repository(None, "team", url, true).unwrap();
    assert_eq!(
        made,
        "<addons default-repository=\"team\">\n    <repository name=\"team\">\n        <location type=\"git\" url=\"https://team.example.com/git/${name}.git\"/>\n    </repository>\n</addons>\n"
    );
    let existing = "<addons>\n    <component name=\"acme\" repository=\"team\"/>\n</addons>\n";
    let added = with_repository(Some(existing), "team", url, false).unwrap();
    assert!(
        added.starts_with(
            "<addons>\n    <component name=\"acme\" repository=\"team\"/>\n    <repository name=\"team\">"
        ),
        "{added}"
    );
    // stored again: replaced, not doubled
    let again = with_repository(Some(&added), "team", "https://other.example.com/git/${name}.git", true).unwrap();
    assert_eq!(again.matches("<repository name=\"team\"").count(), 1, "{again}");
    assert!(
        again.contains("other.example.com") && again.contains("default-repository=\"team\""),
        "{again}"
    );
    let (removed, had) = without_repository(&again, "team");
    assert!(had);
    assert_eq!(removed, existing);
    assert!(with_repository(Some("<oops/>"), "team", url, false).is_err());
    assert_eq!(repository_name("Team Server", ""), "team-server");
    assert_eq!(repository_name("", "https://moqui.org"), "moqui-org");
}

#[test]
fn push_to_review_names_the_target_and_its_options() {
    assert_eq!(
        propose_args("origin", "main", "due-date", "Add a due date"),
        [
            "push",
            "-o",
            "topic=due-date",
            "-o",
            "title=Add a due date",
            "origin",
            "HEAD:refs/for/main"
        ]
    );
    assert_eq!(
        propose_args("origin", "main", "", ""),
        ["push", "origin", "HEAD:refs/for/main"]
    );
}

/// A plain server's repositories go through the framework's REST; a repository Smithy keeps, on a server that offers
/// Smithy's REST, through Smithy's operations.
#[test]
fn each_server_kind_has_its_rest_contract() {
    let plain = server(json!({ "rest": { "repositories": "/rest/s1/moqui/git" } }));
    let repository = json!({ "repositoryId": "acme-site" });
    assert_eq!(via(&plain, &repository), Via::Framework);
    let opening = Opening {
        source: "due-date".into(),
        target: "main".into(),
        title: "Add a due date".into(),
        ..Opening::default()
    };
    let list = list_call(&Via::Framework, "/rest/s1/moqui/git", "acme-site", "mine", "ann");
    assert_eq!(list.path, "/rest/s1/moqui/git/acme-site/mergeRequests");
    assert_eq!(list.query, [("filter", "mine".to_string())]);
    let create = create_call(&Via::Framework, "/rest/s1/moqui/git", "acme-site", &opening);
    assert!(create.post);
    assert_eq!(create.path, "/rest/s1/moqui/git/acme-site/mergeRequests");
    assert_eq!(
        create.body,
        Some(json!({ "sourceRef": "due-date", "targetRef": "main", "title": "Add a due date" }))
    );
    assert!(fork_call(&Via::Framework, "acme-site").is_none());

    let smithy_server =
        server(json!({ "application": { "name": "smithy", "restBase": "/rest/s1/pass/v1/sites/S/smithy/" } }));
    let kept = json!({ "repositoryId": "R1", "keptBy": { "application": "smithy" } });
    let smithy = via(&smithy_server, &kept);
    assert_eq!(
        smithy,
        Via::Application(Application {
            name: "smithy".into(),
            rest_base: "/rest/s1/pass/v1/sites/S/smithy".into()
        })
    );
    // a repository Smithy does not keep stays on the framework's REST of the same server
    assert_eq!(via(&smithy_server, &repository), Via::Framework);
    let list = list_call(&smithy, "/rest/s1/moqui/git", "R1", "review", "ann");
    assert_eq!(list.path, "/rest/s1/pass/v1/sites/S/smithy/merge-requests");
    assert_eq!(
        list.query,
        [
            ("repositoryId", "R1".to_string()),
            ("state", "open".to_string()),
            ("reviewerHandle", "ann".to_string())
        ]
    );
    let create = create_call(&smithy, "/rest/s1/moqui/git", "R1", &opening);
    assert_eq!(
        create.path,
        "/rest/s1/pass/v1/sites/S/smithy/repositories/R1/merge-requests"
    );
    let body = create.body.unwrap();
    assert_eq!(body["body"], json!(null));
    assert_eq!(body["title"], "Add a due date");
    assert!(body["operationId"].as_str().is_some_and(|id| !id.is_empty()));
    let fork = fork_call(&smithy, "R1").unwrap();
    assert_eq!(fork.path, "/rest/s1/pass/v1/sites/S/smithy/repositories/R1/forks");

    // the list of either shape reads the same
    let framework = json!({ "mergeRequestList": [{ "mergeRequestNum": 3, "title": "T", "statusId": "GmrOpen", "authorName": "Ann" }] });
    let application =
        json!({ "result": { "items": [{ "number": 3, "title": "T", "state": "open", "authorHandle": "ann" }] } });
    let one = merge_request_item(&first_list(&framework)[0]);
    let two = merge_request_item(&first_list(&application)[0]);
    assert_eq!(one["mergeRequestNum"], 3);
    assert_eq!(two["mergeRequestNum"], 3);
    assert_eq!(one["author"], "Ann");
    assert_eq!(two["state"], "open");
}
