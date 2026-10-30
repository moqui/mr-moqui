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
