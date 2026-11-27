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
use serde_json::json;

const ROOT: &str = "https://stac.example.test/catalog.json";

#[test]
fn the_root_lists_its_releases_newest_first() {
    let root = json!({
        "type": "Catalog", "latest": "2026-11-18.0",
        "links": [
            {"rel": "root", "href": ROOT},
            {"rel": "child", "href": "https://stac.example.test/2026-10-21.0/catalog.json", "title": "2026-10-21.0 Overture Release"},
            {"rel": "child", "href": "./2026-11-18.0/catalog.json", "title": "2026-11-18.0 Overture Release", "latest": true},
            {"rel": "child", "href": "./notes/catalog.json", "title": "Notes"},
            {"rel": "alternate", "href": "./latest.ddb"}
        ]
    });
    let releases = releases(ROOT, &root);
    assert_eq!(
        releases,
        vec![
            Release {
                version: "2026-11-18.0".into(),
                href: "https://stac.example.test/2026-11-18.0/catalog.json".into(),
                latest: true
            },
            Release {
                version: "2026-10-21.0".into(),
                href: "https://stac.example.test/2026-10-21.0/catalog.json".into(),
                latest: false
            },
        ]
    );
}

#[test]
fn a_release_names_its_schema_and_themes() {
    let url = "https://stac.example.test/2026-11-18.0/catalog.json";
    let document = json!({
        "release:version": "2026-11-18.0", "schema:version": "2.0.0",
        "links": [
            {"rel": "child", "href": "./places/catalog.json", "title": "places"},
            {"rel": "child", "href": "https://stac.example.test/2026-11-18.0/buildings/catalog.json", "title": "buildings"}
        ]
    });
    let catalog = release(url, &document).unwrap();
    assert_eq!(catalog.schema_version, "2.0.0");
    assert_eq!(
        catalog.themes[0],
        (
            "places".into(),
            "https://stac.example.test/2026-11-18.0/places/catalog.json".into()
        )
    );
    assert!(schema_supported("2.0.0") && schema_supported("2.1.3"));
    assert!(!schema_supported("1.18.0") && !schema_supported("3.0.0"));
    assert!(release(url, &json!({"release:version": "x"})).is_err());
}

#[test]
fn a_collection_gives_each_item_its_extent_box() {
    let url = "https://stac.example.test/r/places/place/collection.json";
    let document = json!({
        "extent": {"spatial": {"bbox": [[-180, -90, 180, 90], [0, 40, 20, 50], [100, -10, 120, 10]]}},
        "table:columns": [{"name": "id"}, {"name": "geometry"}],
        "schema:version": "2.0.0",
        "links": [
            {"rel": "item", "href": "./00000/00000.json"},
            {"rel": "item", "href": "./00001/00001.json"}
        ]
    });
    let collection = collection(url, &document);
    assert_eq!(
        collection.items[0].href,
        "https://stac.example.test/r/places/place/00000/00000.json"
    );
    assert_eq!(collection.items[1].bbox, Some([100.0, -10.0, 120.0, 10.0]));
    assert_eq!(collection.columns, vec!["id", "geometry"]);
    let region = [11.24, 43.76, 11.27, 43.78];
    let chosen: Vec<&ItemRef> = collection
        .items
        .iter()
        .filter(|item| meets(&item.bbox.unwrap(), &region))
        .collect();
    assert_eq!(chosen.len(), 1);
    // an extent that does not have a box for each item gives none
    let partial = json!({"extent": {"spatial": {"bbox": [[-180, -90, 180, 90]]}}, "links": document["links"].clone()});
    assert_eq!(super::collection(url, &partial).items[0].bbox, None);
}

#[test]
fn an_item_gives_its_https_parquet_asset() {
    let url = "https://stac.example.test/r/places/place/00000/00000.json";
    let document = json!({
        "bbox": [0, 40, 20, 50],
        "assets": {
            "azure": {"href": "https://blob.example.test/a.parquet", "type": "application/vnd.apache.parquet"},
            "aws": {"href": "https://s3.example.test/release/a.zstd.parquet", "type": "application/vnd.apache.parquet", "file:size": 1234}
        }
    });
    let item = item(url, &document).unwrap();
    assert_eq!(item.href, "https://s3.example.test/release/a.zstd.parquet");
    assert_eq!(item.size, Some(1234));
    let plain_http = json!({"bbox": [0, 0, 1, 1], "assets": {"aws": {"href": "http://s3.example.test/a.parquet"}}});
    assert!(super::item(url, &plain_http).is_err());
    assert!(super::item(url, &json!({"assets": {}})).is_err());
}

#[test]
fn links_resolve_against_their_document() {
    assert_eq!(
        resolve("https://h.test/a/b/c.json", "./d/e.json"),
        "https://h.test/a/b/d/e.json"
    );
    assert_eq!(
        resolve("https://h.test/a/b/c.json", "../x.json"),
        "https://h.test/a/x.json"
    );
    assert_eq!(
        resolve("https://h.test/a/b/c.json", "/root.json"),
        "https://h.test/root.json"
    );
    assert_eq!(
        resolve("https://h.test/a/b/c.json", "https://o.test/y"),
        "https://o.test/y"
    );
    assert_eq!(
        origin("https://Stac.Example.test/a?b"),
        Some("https://stac.example.test".into())
    );
    assert!(meets(&[0.0, 0.0, 1.0, 1.0], &[1.0, 1.0, 2.0, 2.0]));
    assert!(!meets(&[0.0, 0.0, 1.0, 1.0], &[1.5, 0.0, 2.0, 1.0]));
}
