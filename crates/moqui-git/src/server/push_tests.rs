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

//! The push client against this crate's own receive-pack, over a `git://` connection (one stateful conversation, as
//! over SSH), and the parsing of push mirror settings.

use std::io::BufWriter;
use std::net::TcpStream;

use super::push::{self, mapped, parse_ref_maps, parse_ssh_url, receive_pack_command, PushOutcome, RefMap, SshUrl};
use super::tests::{daemon, define, fixture, settled, Fixture, WRITER};

fn map(from: &str, to: &str) -> RefMap {
    RefMap {
        from: from.into(),
        to: to.into(),
    }
}

#[test]
fn ref_maps_name_branches_and_prefixes() {
    assert_eq!(
        parse_ref_maps("").unwrap(),
        vec![map("refs/heads/*", "refs/heads/*"), map("refs/tags/*", "refs/tags/*")]
    );
    assert_eq!(
        parse_ref_maps("master:mirror, refs/tags/*:refs/tags/*").unwrap(),
        vec![
            map("refs/heads/master", "refs/heads/mirror"),
            map("refs/tags/*", "refs/tags/*")
        ]
    );
    assert_eq!(
        parse_ref_maps("+main").unwrap(),
        vec![map("refs/heads/main", "refs/heads/main")]
    );
    assert!(parse_ref_maps("refs/heads/*:mirror").is_err());
    let id = gix::ObjectId::null(gix::hash::Kind::Sha1);
    let refs = vec![
        ("refs/heads/master".to_string(), id),
        ("refs/heads/topic".to_string(), id),
        ("refs/tags/v1".to_string(), id),
    ];
    let names: Vec<String> = mapped(&parse_ref_maps("master:mirror").unwrap(), &refs)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["refs/heads/mirror"]);
    let names: Vec<String> = mapped(&parse_ref_maps("refs/tags/*:refs/tags/*").unwrap(), &refs)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["refs/tags/v1"]);
}

#[test]
fn ssh_urls_in_both_forms() {
    assert_eq!(
        parse_ssh_url("git@github.com:owner/repo.git").unwrap(),
        SshUrl {
            user: Some("git".into()),
            host: "github.com".into(),
            path: "owner/repo.git".into()
        }
    );
    assert_eq!(
        parse_ssh_url("ssh://git@example.com:2222/owner/repo.git").unwrap(),
        SshUrl {
            user: Some("git".into()),
            host: "example.com:2222".into(),
            path: "/owner/repo.git".into()
        }
    );
    assert!(parse_ssh_url("https://github.com/owner/repo.git").is_err());
    assert!(parse_ssh_url("github.com").is_err());
    assert_eq!(receive_pack_command("a'b.git"), r"git-receive-pack 'a'\''b.git'");
}

/// Push `maps` of repository `src` to `dst` over a fresh connection to `base`.
fn push_once(fixture: &Fixture, base: &str, maps: &str) -> PushOutcome {
    let _entered = fixture.context.enter();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    let src = provider.repo("src").unwrap();
    let refs = mapped(&parse_ref_maps(maps).unwrap(), &src.store.list_refs("refs/").unwrap());
    let stream = TcpStream::connect(base.strip_prefix("git://").unwrap()).unwrap();
    super::pktline::data(&mut &stream, b"git-receive-pack /dst.git\0host=localhost\0").unwrap();
    let mut input = stream.try_clone().unwrap();
    let mut output = BufWriter::new(stream);
    let outcome = push::push(src.store.as_ref(), &refs, &mut input, &mut output).unwrap();
    drop(output);
    outcome
}

fn head_of(fixture: &Fixture, id: &str, name: &str) -> Option<gix::ObjectId> {
    let _entered = fixture.context.enter();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    provider.repo(id).unwrap().store.read_ref(name).unwrap()
}

fn write(fixture: &Fixture, text: &str) {
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(WRITER).unwrap();
    fixture
        .context
        .factory()
        .resource()
        .write_bytes("git://src/a.md", text.as_bytes())
        .unwrap();
    moqui_core::user::logout().unwrap();
}

#[test]
fn a_push_mirror_sends_what_the_server_lacks_under_the_mapped_name() {
    let fixture = fixture("push-mirror");
    define(&fixture, "src", "db");
    define(&fixture, "dst", "db");
    write(&fixture, "one\n");
    write(&fixture, "two\n");
    settled(&fixture);
    let base = daemon(fixture.context.factory(), WRITER);

    let first = push_once(&fixture, &base, "main:mirror");
    settled(&fixture);
    assert_eq!(first.pushed, ["refs/heads/mirror"]);
    assert!(first.refused.is_empty(), "{first:?}");
    let main = head_of(&fixture, "src", "refs/heads/main");
    assert!(main.is_some());
    assert_eq!(head_of(&fixture, "dst", "refs/heads/mirror"), main);
    assert_eq!(head_of(&fixture, "dst", "refs/heads/main"), None);

    let again = push_once(&fixture, &base, "main:mirror");
    settled(&fixture);
    assert_eq!((again.pushed.len(), again.current, again.objects), (0, 1, 0));

    write(&fixture, "three\n");
    settled(&fixture);
    let next = push_once(&fixture, &base, "main:mirror");
    settled(&fixture);
    assert_eq!(next.pushed, ["refs/heads/mirror"]);
    // the commit, its tree and the new blob: the server had the rest
    assert_eq!(next.objects, 3, "{next:?}");
    assert_eq!(
        head_of(&fixture, "dst", "refs/heads/mirror"),
        head_of(&fixture, "src", "refs/heads/main")
    );
}
