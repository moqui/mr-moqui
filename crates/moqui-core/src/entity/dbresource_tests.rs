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

use crate::entity::EntityFacade;

fn facade() -> (EntityFacade, crate::context::ExecutionContext) {
    let facade = crate::testing::open();
    facade.assume_db_tables();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    (facade, context)
}

/// `ResourceRef::size` of a `dbresource://` file is its data length, for the file and for one found among a
/// directory's children, whose bytes are read when asked for.
#[test]
fn a_db_resource_ref_has_its_size_and_a_child_its_bytes() {
    let (facade, context) = facade();
    let _entered = context.enter();
    facade
        .write_db_resource("dbresource://files/a·b.txt", "héllo".as_bytes())
        .unwrap();
    facade.write_db_resource("dbresource://files/empty.txt", b"").unwrap();
    let resources = context.factory().resource();
    let file = resources.get("dbresource://files/a·b.txt").unwrap();
    assert_eq!(file.size(), Some(6));
    assert_eq!(resources.get("dbresource://files/empty.txt").unwrap().size(), Some(0));
    let directory = resources.get("dbresource://files").unwrap();
    assert_eq!(directory.size(), None);
    let child = directory.find_child_file("a·b.txt");
    assert_eq!(child.size(), Some(6));
    assert_eq!(child.read_text().unwrap(), "héllo");
    let listed: Vec<_> = directory
        .children()
        .unwrap()
        .iter()
        .map(|child| (child.size(), child.read_bytes().unwrap().len()))
        .collect();
    assert!(
        listed.contains(&(Some(6), 6)) && listed.contains(&(Some(0), 0)),
        "{listed:?}"
    );
    assert_eq!(directory.find_child_file("missing.txt").size(), None);
}

/// `ResourceFacade::open` and `content_address` of `dbresource://…#version`: a large version by range from its content
/// blob, a small one by its digest, each with the tag its source has; and a pinned response
/// (`web::send_pinned_resource_response`) of `#01` recorded before the file was deleted and made again, whose `#01`
/// is then other bytes, is refused (409), as is the location without `#version`, never sent under the old tag.
#[test]
fn a_version_is_pinned_by_its_content_address_and_a_recreated_one_is_refused() {
    use crate::context::ContentKind;
    let (facade, context) = facade();
    let _entered = context.enter();
    let resources = context.factory().resource();
    let large = "dbresource://maps/world.pmtiles";
    let first: Vec<u8> = (0..100_000u32).map(|n| (n * 7 % 251) as u8).collect();
    facade.write_db_resource(large, &first).unwrap();
    facade.write_db_resource(large, b"replaced").unwrap();
    let opened = resources.open(&format!("{large}#01")).unwrap().unwrap();
    let address = resources.content_address(&format!("{large}#01")).unwrap().unwrap();
    assert_eq!(address.kind, ContentKind::ContentBlob);
    assert_eq!((address.size, opened.stat.size), (100_000, 100_000));
    assert_eq!(opened.source.tag(), Some(address.tag.clone()));
    assert_eq!(opened.source.read_at(99_990, 10).unwrap(), first[99_990..]);
    let head = resources.content_address(large).unwrap().unwrap();
    assert_eq!((head.kind, head.size), (ContentKind::Digest, 8));
    assert_eq!(resources.open(large).unwrap().unwrap().source.tag(), Some(head.tag));

    let small = "dbresource://maps/style.json";
    facade.write_db_resource(small, b"{\"v\":1}").unwrap();
    let pinned = resources.content_address(&format!("{small}#01")).unwrap().unwrap();
    assert_eq!(pinned.kind, ContentKind::Digest);
    assert_eq!(
        pinned.tag,
        format!("sha256-{}", crate::resource::blob::blob_id(b"{\"v\":1}"))
    );
    let send = |location: &str| {
        let _web = crate::web::install(Default::default());
        let sent = crate::web::send_pinned_resource_response(location, &pinned.tag, pinned.size, true, None);
        (sent, crate::web::take_response())
    };
    let (sent, response) = send(&format!("{small}#01"));
    assert!(sent);
    assert_eq!((response.status, response.body.as_slice()), (200, &b"{\"v\":1}"[..]));
    assert_eq!(response.header("ETag"), Some(format!("\"{}\"", pinned.tag).as_str()));
    // the same name and version name, other bytes of the same size
    assert!(resources.delete(small).unwrap());
    facade.write_db_resource(small, b"{\"v\":2}").unwrap();
    let (sent, response) = send(&format!("{small}#01"));
    assert!(!sent);
    assert_eq!(response.status, 409);
    let (sent, response) = send(small);
    assert!(!sent, "a location without #version on storage that keeps versions");
    assert_eq!(response.status, 500);
}
