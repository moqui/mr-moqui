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

//! The kinds: templates expand only their named placeholders, a secret becomes a file named by its `*_FILE` setting
//! and is refused anywhere `docker inspect` shows.

use std::collections::BTreeMap;

use zeroize::Zeroizing;

use crate::kinds::{self, ContainerInput};

#[test]
fn a_template_expands_only_its_named_placeholders() {
    let values = BTreeMap::from([("instanceName", "demo".to_string())]);
    assert_eq!(kinds::expand("${instanceName}-data", &values).unwrap(), "demo-data");
    assert_eq!(kinds::expand("${databaseHost}", &values).unwrap(), "");
    let refused = kinds::expand("${ec.user.userId}", &values).unwrap_err();
    assert!(refused.message().contains("not one of"), "{refused}");
    assert!(kinds::expand("${instanceName", &values).is_err());
}

fn postgres(secret: &[u8]) -> ContainerInput {
    ContainerInput {
        kind: "postgres".to_string(),
        name: "pg-main".to_string(),
        image: "postgres:18".to_string(),
        env: vec![("POSTGRES_USER".to_string(), "postgres".to_string())],
        run_as: Some("999:999".to_string()),
        secrets: vec![(
            "postgres-password".to_string(),
            Some("POSTGRES_PASSWORD_FILE".to_string()),
            Zeroizing::new(secret.to_vec()),
        )],
        ..ContainerInput::default()
    }
}

#[test]
fn a_secret_is_a_file_named_by_its_setting_and_never_in_what_inspect_shows() {
    let spec = kinds::container_spec(postgres(b"a-generated-credential")).unwrap();
    assert!(spec.env.contains(&(
        "POSTGRES_PASSWORD_FILE".to_string(),
        "/run/secrets/postgres-password".to_string()
    )));
    let file = &spec.secrets[0];
    assert_eq!((file.uid, file.gid, file.mode), (999, 999, 0o400));
    spec.check().unwrap();
    // a template that would put the value in a setting is refused before anything is sent
    let mut leaky = postgres(b"a-generated-credential");
    leaky.env.push((
        "DATABASE_URL".to_string(),
        "postgres://u:a-generated-credential@db/x".to_string(),
    ));
    let refused = kinds::container_spec(leaky).unwrap().check().unwrap_err();
    assert!(refused.message().contains("environment (DATABASE_URL)"), "{refused}");
    assert!(!refused.message().contains("a-generated-credential"), "{refused}");
    // OpenSearch's wrapped entrypoint names the file, never the value
    let mut search = postgres(b"a-generated-credential-aA1");
    search.kind = "opensearch".to_string();
    search.secrets[0].0 = "opensearch-admin-password".to_string();
    search.secrets[0].1 = None;
    let spec = kinds::container_spec(search).unwrap();
    assert!(spec
        .entrypoint
        .join(" ")
        .contains("/run/secrets/opensearch-admin-password"));
    spec.check().unwrap();
}
