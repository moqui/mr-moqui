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

//! On-disk repositories in a cluster: refused on a member without shared storage, and with it two providers (two
//! "members" on one database and one directory, each on its own thread) that take turns and see each other's writes.

use std::time::{Duration, Instant};

use moqui_core::entity::EntityValue;
use moqui_core::native::ResourceProvider;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::object::Value;

use super::lease;
use crate::server::tests::{define, fixture_with, Fixture};
use crate::GitProvider;

/// Another member's row in `moqui.server.ClusterMember`, its heartbeat now: this node is then a cluster member.
fn another_member(fixture: &Fixture) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut row = EntityValue::make(&fixture.entity, "moqui.server.ClusterMember").unwrap();
    row.set("instanceId", Value::string("the-other-member"));
    row.set("clusterBaseUrl", Value::string("https://10.0.0.2:8444"));
    row.set("heartbeat", moqui_core::user::now_value());
    row.create(&fixture.entity).unwrap();
}

fn provider_of(fixture: &Fixture) -> GitProvider {
    crate::provider(fixture.context.factory()).unwrap()
}

/// A second provider over the same database and runtime directory: another member's.
fn second_provider(fixture: &Fixture) -> GitProvider {
    let resources = fixture.context.factory().resource();
    GitProvider::new(resources.entity_slot(), resources.runtime_dir().to_path_buf())
}

#[test]
fn a_cluster_member_without_shared_storage_makes_no_new_repository_on_disk_and_names_the_ones_it_has() {
    let fixture = fixture_with("shared-refused", moqui_core::context::framework_dir(), "");
    let provider = provider_of(&fixture);
    define(&fixture, "before", "local");
    define(&fixture, "mirror", "remote");
    define(&fixture, "kept", "db");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        // made while this node was alone
        provider.write_bytes("git://before/a.txt", b"one\n").unwrap();
    }
    another_member(&fixture);
    define(&fixture, "after", "local");
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    provider.forget_clustered();
    assert!(provider.clustered());
    for refused in [
        provider.write_bytes("git://after/a.txt", b"one\n").unwrap_err(),
        provider.init("after").unwrap_err(),
        provider.fetch("mirror").unwrap_err(),
    ] {
        assert!(refused.message().starts_with("GitNeedsSharedStorage: "), "{refused}");
    }
    assert!(!fixture.dir.join("after.git").exists(), "no directory was made");
    // one already there stays as it was (the node starts), a db repository is no concern
    provider.write_bytes("git://before/b.txt", b"two\n").unwrap();
    provider.write_bytes("git://kept/a.txt", b"db\n").unwrap();
    let mut named = provider.warn_unshared(fixture.context.factory());
    named.sort();
    assert_eq!(named, ["after", "before", "mirror"]);
}

#[test]
fn with_shared_storage_two_members_take_turns_and_read_each_others_writes() {
    let fixture = fixture_with(
        "shared-turns",
        moqui_core::context::framework_dir(),
        r#"<git-server shared-storage="true" shared-lock-wait-seconds="1"/>"#,
    );
    another_member(&fixture);
    define(&fixture, "shared", "local");
    let (first, second) = (provider_of(&fixture), second_provider(&fixture));
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    first.forget_clustered();
    assert!(first.clustered());
    // each reads what the other wrote, though each opened the repository before
    first.write_bytes("git://shared/a.txt", b"from the first\n").unwrap();
    assert_eq!(second.read_bytes("git://shared/a.txt").unwrap(), b"from the first\n");
    second.write_bytes("git://shared/b.txt", b"from the second\n").unwrap();
    assert_eq!(first.read_bytes("git://shared/b.txt").unwrap(), b"from the second\n");

    // a member mid-write (the repository's lease held on another thread): the other's write waits for it
    let entity = fixture.entity.clone();
    let hold = |millis: u64| {
        let entity = entity.clone();
        let (taken, wait_taken) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let held = lease::take(
                &entity,
                &lease::repository_name("shared"),
                lease::TTL,
                Duration::from_secs(5),
            )
            .unwrap();
            taken.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(millis));
            drop(held);
        });
        wait_taken.recv().unwrap();
        thread
    };
    let holder = hold(400);
    let began = Instant::now();
    second.write_bytes("git://shared/c.txt", b"after the wait\n").unwrap();
    assert!(began.elapsed() >= Duration::from_millis(300), "{:?}", began.elapsed());
    holder.join().unwrap();
    // past shared-lock-wait-seconds the write is refused, and nothing is written
    let holder = hold(2_500);
    let refused = first.write_bytes("git://shared/d.txt", b"never\n").unwrap_err();
    assert!(refused.message().starts_with("GitLeaseBusy: "), "{refused}");
    holder.join().unwrap();
    assert!(first.read_bytes("git://shared/d.txt").is_err());

    // both writing at once: every commit kept, none lost to the other
    let factory = std::sync::Arc::clone(fixture.context.factory());
    let writers: Vec<_> = [first.clone(), second.clone()]
        .into_iter()
        .enumerate()
        .map(|(member, provider)| {
            let factory = std::sync::Arc::clone(&factory);
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                for n in 0..5 {
                    provider
                        .write_bytes(&format!("git://shared/m{member}-{n}.txt"), b"x\n")
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    for member in 0..2 {
        for n in 0..5 {
            assert_eq!(
                second.read_bytes(&format!("git://shared/m{member}-{n}.txt")).unwrap(),
                b"x\n"
            );
        }
    }
    assert_eq!(first.history("git://shared/m0-4.txt").unwrap().len(), 1);
}

#[test]
fn a_fork_keeps_its_sources_lease_until_its_transaction_ends_and_a_delete_takes_it() {
    let fixture = fixture_with(
        "shared-fork",
        moqui_core::context::framework_dir(),
        r#"<git-server shared-lock-wait-seconds="1"/>"#,
    );
    define(&fixture, "source", "db");
    let provider = provider_of(&fixture);
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    provider.write_bytes("git://source/a.txt", b"one\n").unwrap();
    let entity = fixture.entity.clone();
    let free_elsewhere = |name: &'static str| {
        let entity = entity.clone();
        std::thread::spawn(move || {
            lease::try_take(&entity, &lease::repository_name(name), lease::TTL)
                .unwrap()
                .is_some()
        })
        .join()
        .unwrap()
    };
    let transactions = fixture.entity.transactions_for(crate::GIT_REPOSITORY).unwrap();
    transactions
        .run(TransactionAttribute::UseOrBegin, None, || {
            provider.fork("source", "fork", None, None)?;
            // another member's delete of the source waits: the fork's row is not committed yet
            assert!(!free_elsewhere("source"), "held until the fork's transaction ends");
            Ok(())
        })
        .unwrap();
    assert!(free_elsewhere("source"), "let go when it committed");

    // a delete looks for forks under the lease: held by another member, it waits, then finds the fork
    let held = {
        let entity = entity.clone();
        let (taken, wait_taken) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let lease = lease::take(
                &entity,
                &lease::repository_name("source"),
                lease::TTL,
                Duration::from_secs(5),
            )
            .unwrap();
            taken.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(2_500));
            drop(lease);
        });
        wait_taken.recv().unwrap();
        thread
    };
    let refused = provider.delete_repository("source").unwrap_err();
    assert!(refused.message().starts_with("GitLeaseBusy: "), "{refused}");
    held.join().unwrap();
    let refused = provider.delete_repository("source").unwrap_err();
    assert!(refused.message().starts_with("GitHasForks: "), "{refused}");
}

/// With no context entered (a background thread), a provider reads the conf and the cluster's members through its own
/// factory; and a node once seen to be a member stays one when every peer's heartbeat is down, so it still refuses a
/// new on-disk repository its peers could not reach.
#[test]
fn with_no_context_the_providers_factory_answers_and_membership_does_not_lapse() {
    let fixture = fixture_with(
        "shared-no-context",
        moqui_core::context::framework_dir(),
        r#"<git-server shared-storage="true"/>"#,
    );
    let provider = provider_of(&fixture);
    let on_thread = |provider: &GitProvider| {
        let provider = provider.clone();
        std::thread::spawn(move || (provider.shared_storage(), provider.clustered()))
            .join()
            .unwrap()
    };
    assert_eq!(on_thread(&provider), (true, false), "alone");
    another_member(&fixture);
    provider.forget_clustered();
    assert_eq!(
        on_thread(&provider),
        (true, true),
        "read through the provider's factory"
    );
    // the peer's heartbeat stops: still a member
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.server.ClusterMember")
            .unwrap()
            .eq("instanceId", Value::string("the-other-member"))
            .update_all([(
                "heartbeat".to_string(),
                moqui_core::user::stamp_value(jiff::Timestamp::now().as_millisecond() - 3_600_000),
            )])
            .unwrap();
    }
    *moqui_lib::sync::lock(&provider.inner.clustered) = None;
    assert!(provider.clustered(), "a member whose peers are all down is still one");
}
