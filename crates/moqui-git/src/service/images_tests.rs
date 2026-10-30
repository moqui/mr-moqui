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

//! `org.moqui.impl.ImageServices.convert#Image` writing its variants to `git://` locations: one commit, the PNGs in
//! LFS. The service's own tests are moqui-core's `service/images_tests.rs`.

use std::collections::BTreeMap;
use std::io::Cursor;

use image::{DynamicImage, ImageFormat};
use moqui_lib::object::Value;

fn png_with_alpha() -> Vec<u8> {
    let mut image = ::image::RgbaImage::new(40, 20);
    for (x, _, pixel) in image.enumerate_pixels_mut() {
        *pixel = if x < 20 {
            ::image::Rgba([255, 0, 0, 255])
        } else {
            ::image::Rgba([0, 0, 0, 0])
        };
    }
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// The variants of an image go into a git repository in one commit, its PNGs in LFS.
#[test]
fn the_variants_of_an_image_are_one_commit() {
    let ctx = moqui_core::context::ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    ctx.factory().set_entity(facade.clone());
    crate::install(ctx.factory()).unwrap();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let services = moqui_core::context::framework_dir().join("service/org/moqui/impl/ImageServices.xml");
    let failures = moqui_core::service::lint(&[services]).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let mut repository = facade.make_value("moqui.resource.GitRepository").unwrap();
    repository.set("repositoryId", Value::string("media"));
    repository.set("repositoryKind", Value::string("db"));
    repository.create(&facade).unwrap();
    let resources = ctx.factory().resource();
    let provider = crate::provider(ctx.factory()).unwrap();
    provider
        .setup_lfs("media", &crate::attributes::DEFAULT_LFS_PATTERNS)
        .unwrap();
    let mut parameters = BTreeMap::new();
    parameters.insert("inputImageStream".to_string(), Value::Bytes(png_with_alpha()));
    parameters.insert("outputImageFormat".to_string(), Value::string("png"));
    for suffix in ["Original", "Small", "Thumbnail"] {
        parameters.insert(
            format!("outputResourceReference{suffix}"),
            Value::string(format!("git://media/logo_{suffix}.png")),
        );
    }
    moqui_core::service::sync("org.moqui.impl.ImageServices.convert#Image", &parameters).unwrap();
    assert_eq!(ctx.message().errors(), Vec::<String>::new());
    let versions: Vec<String> = ["Original", "Small", "Thumbnail"]
        .iter()
        .map(|suffix| {
            let history = resources
                .version_history(&format!("git://media/logo_{suffix}.png"))
                .unwrap();
            assert_eq!(history.len(), 1);
            history[0].version_name.clone()
        })
        .collect();
    assert!(versions.iter().all(|version| *version == versions[0]), "{versions:?}");
    let small = image::load_from_memory(
        &resources
            .get("git://media/logo_Small.png")
            .unwrap()
            .read_bytes()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(small.width(), 360);
    let lfs = facade.find("moqui.resource.GitLfsObject").unwrap().list().unwrap();
    assert_eq!(lfs.len(), 3, "the PNGs are LFS files");
}
