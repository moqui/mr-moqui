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

#[test]
fn decode_finds_the_format_and_resize_keeps_the_aspect_ratio() {
    let (image, format) = decode(&png_with_alpha()).unwrap();
    assert_eq!(format, "png");
    let small = resized(&image, 10).unwrap();
    assert_eq!((small.width(), small.height()), (10, 5));
    // a narrower image is scaled up, as in Java
    let large = resized(&image, 2160).unwrap();
    assert_eq!((large.width(), large.height()), (2160, 1080));
}

#[test]
fn a_tall_thin_image_is_refused_not_allocated() {
    // the review's case: 1x60000 at the default large width asked for 2160x129600000
    let tall = DynamicImage::ImageLuma8(::image::GrayImage::new(1, 60_000));
    let err = resized(&tall, 2160).unwrap_err();
    assert!(err.message().contains("past the limit"), "{}", err.message());
    let wide = DynamicImage::ImageLuma8(::image::GrayImage::new(20_000, 1));
    assert!(resized(&wide, 20_000).is_err());
    // the decoder refuses a side past its limit before allocating
    let mut out = Cursor::new(Vec::new());
    tall.write_to(&mut out, ImageFormat::Png).unwrap();
    let err = decode(&out.into_inner()).unwrap_err();
    assert!(err.message().contains("Could not read image"), "{}", err.message());
}

#[test]
fn jpeg_puts_a_transparent_image_on_white() {
    let (image, _) = decode(&png_with_alpha()).unwrap();
    let bytes = encode(&image, "jpg").unwrap();
    let (back, format) = decode(&bytes).unwrap();
    assert_eq!(format, "jpg");
    let rgb = back.to_rgb8();
    // the transparent half is white, the opaque half stays red
    let right = rgb.get_pixel(35, 10);
    assert!(right.0.iter().all(|channel| *channel > 240), "{right:?}");
    let left = rgb.get_pixel(5, 10);
    assert!(left[0] > 200 && left[1] < 60, "{left:?}");
}

#[test]
fn every_listed_format_writes() {
    let (image, _) = decode(&png_with_alpha()).unwrap();
    for name in ["jpg", "png", "gif", "tiff", "webp", "bmp"] {
        let bytes = encode(&image, name).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(decode(&bytes).unwrap().0.width(), 40, "{name}");
    }
    assert!(encode(&image, "wbmp")
        .unwrap_err()
        .message()
        .contains("No writer for: wbmp"));
}

/// The original is written as `outputImageFormat` when that format can be written, and in the input's otherwise.
#[test]
fn the_original_is_written_as_the_output_format_when_it_can_be() {
    let dir = std::env::temp_dir().join(format!("moqui-image-original-{}", std::process::id()));
    let facade = crate::testing::open();
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let convert = |output_format: &str, name: &str| {
        let location = format!("file:{}", dir.join(name).display());
        let mut parameters = BTreeMap::new();
        parameters.insert("inputImageStream".to_string(), Value::Bytes(png_with_alpha()));
        parameters.insert("outputImageFormat".to_string(), Value::string(output_format));
        parameters.insert("outputResourceReferenceOriginal".to_string(), Value::string(&location));
        let out = convert_image(&parameters, &facade).unwrap();
        assert_eq!(out.get("contentLocationOriginal"), Some(&Value::String(location)));
        ::image::guess_format(&std::fs::read(dir.join(name)).unwrap()).unwrap()
    };
    assert_eq!(convert("jpg", "a_Original.jpg"), ImageFormat::Jpeg);
    assert_eq!(convert("wbmp", "b_Original.wbmp"), ImageFormat::Png, "no wbmp writer");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_location_is_text_or_a_resource_reference() {
    assert_eq!(
        location_of(Some(&Value::string("runtime://a.png"))),
        Some("runtime://a.png".to_string())
    );
    let mut map = indexmap::IndexMap::new();
    map.insert("location".to_string(), Value::string("runtime://b.png"));
    assert_eq!(location_of(Some(&Value::Map(map))), Some("runtime://b.png".to_string()));
    assert_eq!(location_of(Some(&Value::Null)), None);
}

/// A png is stored and served to a signed-in user. An svg, a file over 8 MiB, and a get with no session are refused.
#[test]
fn upload_rich_text_image() {
    let ctx = crate::context::ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let path = crate::context::framework_dir().join("service/org/moqui/impl/ResourceServices.xml");
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
    account.set("userId", Value::string("imguser"));
    account.set("username", Value::string("imguser"));
    account.set("userFullName", Value::string("Image User"));
    account.create(&facade).unwrap();
    drop(_authz);

    let refused = crate::service::sync(
        "org.moqui.impl.ResourceServices.get#RichTextImage",
        &std::iter::once(("imageId".to_string(), Value::string("a".repeat(32)))).collect(),
    )
    .unwrap_err();
    assert_eq!(
        refused.kind(),
        crate::context::ErrorKind::AuthenticationRequired,
        "{refused}"
    );

    crate::user::internal_login_user("imguser").unwrap();
    let png = png_with_alpha();
    let uploaded = service_call(
        "upload#RichTextImage",
        upload_file("photo.png", "image/png", png.clone()),
    );
    let url = out_text(&uploaded, "url");
    assert!(url.starts_with("/richTextImage/"), "{url}");
    let id = url.trim_start_matches("/richTextImage/");
    assert!(
        id.len() == 32 && id.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "{url}"
    );
    let found = service_call("get#RichTextImage", [("imageId", Value::string(id))]);
    let location = out_text(&found, "location");
    assert_eq!(location, format!("dbresource://moqui/rich-text/{id}.png"));
    let stored = ctx.factory().resource().get(&location).unwrap().read_bytes().unwrap();
    assert_eq!(stored, png);

    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#;
    let svg_errors = service_errors(
        "upload#RichTextImage",
        upload_file("photo.png", "image/svg+xml", svg.to_vec()),
    );
    assert!(
        svg_errors.contains("read") || svg_errors.contains("png"),
        "{svg_errors}"
    );
    let big = service_errors(
        "upload#RichTextImage",
        upload_file("big.png", "image/png", vec![0; MAX_RICH_TEXT_IMAGE + 1]),
    );
    assert!(big.contains("8 MiB"), "{big}");
}

fn service_call(name: &str, parameters: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    let out = crate::service::sync(&format!("org.moqui.impl.ResourceServices.{name}"), &parameters)
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = message_errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn service_errors(name: &str, parameters: impl IntoIterator<Item = (&'static str, Value)>) -> String {
    let parameters: BTreeMap<String, Value> = parameters
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    crate::service::sync(&format!("org.moqui.impl.ResourceServices.{name}"), &parameters)
        .unwrap_or_else(|err| panic!("{name}: {err}"));
    message_errors()
}

fn message_errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn upload_file(name: &str, content_type: &str, bytes: Vec<u8>) -> Vec<(&'static str, Value)> {
    vec![(
        "contentFile",
        UploadedFile::in_memory("contentFile", name, content_type, bytes).into_value(),
    )]
}
