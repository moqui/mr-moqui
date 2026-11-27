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

/// An image whose header claims more than 64 megapixels within the side limit is refused from its header, before any
/// pixel is decoded or allocated (SEC-SCR-85).
#[test]
fn an_image_of_too_many_pixels_is_refused_from_its_header() {
    // a PNG signature and IHDR of 20000x20000 8-bit RGB, and nothing after: only the header is read
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&20_000u32.to_be_bytes());
    ihdr.extend_from_slice(&20_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&png_chunk(b"IDAT", &[]));
    png.extend_from_slice(&png_chunk(b"IEND", &[]));
    let err = decode(&png).unwrap_err();
    assert!(err.message().contains("more than 64000000 pixels"), "{}", err.message());
    // a photo-sized one is not refused for its pixels (it has no data here, so it fails as a read)
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&8_000u32.to_be_bytes());
    ihdr.extend_from_slice(&6_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&png_chunk(b"IDAT", &[]));
    png.extend_from_slice(&png_chunk(b"IEND", &[]));
    let err = decode(&png).unwrap_err();
    assert!(!err.message().contains("pixels"), "{}", err.message());
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
    let mut map = indexmap::IndexMap::default();
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
    let path = crate::context::framework_dir().join("service/org/moqui/resource/ResourceServices.xml");
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
        "org.moqui.resource.ResourceServices.get#RichTextImage",
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
    // the png is written again from its pixels (SEC-CLI-96): the same image, and its row names the uploader and the
    // stored size (SEC-CLI-92)
    let stored = ctx.factory().resource().get(&location).unwrap().read_bytes().unwrap();
    assert_eq!(
        decode(&stored).unwrap().0.to_rgba8(),
        decode(&png).unwrap().0.to_rgba8()
    );
    let row = image_row(&facade, id).expect("the image's row");
    assert_eq!(row.get("userId").map(to_plain_string).as_deref(), Some("imguser"));
    assert_eq!(row.get("location").map(to_plain_string), Some(location.clone()));
    assert_eq!(row.get("fileSize").map(to_plain_string), Some(stored.len().to_string()));

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
    let out = crate::service::sync(&format!("org.moqui.resource.ResourceServices.{name}"), &parameters)
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
    crate::service::sync(&format!("org.moqui.resource.ResourceServices.{name}"), &parameters)
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

fn image_row(facade: &EntityFacade, id: &str) -> Option<crate::entity::EntityValue> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(RICH_TEXT_IMAGE)
        .unwrap()
        .eq("imageId", Value::string(id))
        .one()
        .unwrap()
}

/// 40x20, the left half red and the right half blue, so a turn shows in where the colors are.
fn halves() -> DynamicImage {
    let mut image = ::image::RgbImage::new(40, 20);
    for (x, _, pixel) in image.enumerate_pixels_mut() {
        *pixel = if x < 20 {
            ::image::Rgb([255, 0, 0])
        } else {
            ::image::Rgb([0, 0, 255])
        };
    }
    DynamicImage::ImageRgb8(image)
}

/// What a test's EXIF carries as a GPS position, and its comments as text.
const GPS_MARKER: &[u8] = b"GPS 48.8584N 2.2945E";
const TEXT_MARKER: &[u8] = b"taken at home";

/// A big-endian TIFF block as EXIF holds it: an orientation entry and a GPS directory whose one entry carries
/// [`GPS_MARKER`].
fn exif_block(orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\0\x2a");
    tiff.extend_from_slice(&8u32.to_be_bytes());
    // IFD0 at 8: two entries, then no next directory; it ends at 38
    tiff.extend_from_slice(&2u16.to_be_bytes());
    tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // orientation, a SHORT
    tiff.extend_from_slice(&3u16.to_be_bytes());
    tiff.extend_from_slice(&1u32.to_be_bytes());
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0x8825u16.to_be_bytes()); // the GPS directory, a LONG offset
    tiff.extend_from_slice(&4u16.to_be_bytes());
    tiff.extend_from_slice(&1u32.to_be_bytes());
    tiff.extend_from_slice(&38u32.to_be_bytes());
    tiff.extend_from_slice(&0u32.to_be_bytes());
    // the GPS directory at 38: one UNDEFINED entry whose bytes are at 56
    tiff.extend_from_slice(&1u16.to_be_bytes());
    tiff.extend_from_slice(&0x001bu16.to_be_bytes());
    tiff.extend_from_slice(&7u16.to_be_bytes());
    tiff.extend_from_slice(&u32::try_from(GPS_MARKER.len()).unwrap().to_be_bytes());
    tiff.extend_from_slice(&56u32.to_be_bytes());
    tiff.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(tiff.len(), 56);
    tiff.extend_from_slice(GPS_MARKER);
    tiff
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn exif_of(bytes: &[u8]) -> Option<Vec<u8>> {
    let (reader, _) = limited_reader(bytes).unwrap();
    reader.into_decoder().unwrap().exif_metadata().unwrap()
}

/// The image is upright (turned from 40x20 to 20x40 for orientation 6) and its colors are where a viewer showed them:
/// red on top, blue below.
fn assert_turned_upright(bytes: &[u8]) {
    let image = decode(bytes).unwrap().0.to_rgb8();
    assert_eq!(image.dimensions(), (20, 40));
    let top = image.get_pixel(10, 5);
    let bottom = image.get_pixel(10, 35);
    assert!(top[0] > 200 && top[2] < 60, "{top:?}");
    assert!(bottom[2] > 200 && bottom[0] < 60, "{bottom:?}");
}

/// A phone's jpeg (SEC-CLI-96): its EXIF segment, with a GPS position and orientation 6, and a comment are gone, and
/// the orientation is in the pixels.
#[test]
fn a_rich_text_jpeg_is_stored_upright_without_its_exif() {
    let mut plain = Vec::new();
    halves()
        .write_with_encoder(JpegEncoder::new_with_quality(&mut plain, 95))
        .unwrap();
    let exif = [b"Exif\0\0".as_slice(), &exif_block(6)].concat();
    let segment = |marker: u8, payload: &[u8]| {
        let length = u16::try_from(payload.len() + 2).unwrap().to_be_bytes();
        [&[0xff, marker], length.as_slice(), payload].concat()
    };
    let jpeg = [
        &plain[..2],
        &segment(0xe1, &exif),
        &segment(0xfe, TEXT_MARKER),
        &plain[2..],
    ]
    .concat();
    // the planted metadata is read from the upload
    assert_eq!(decode_with_metadata(&jpeg).unwrap().orientation, Orientation::Rotate90);
    assert!(exif_of(&jpeg).is_some_and(|block| contains(&block, GPS_MARKER)));

    let (ext, stored) = rich_text_image_bytes(&jpeg).unwrap();
    assert_eq!(ext, "jpg");
    assert_eq!(exif_of(&stored), None);
    for marker in [b"Exif\0\0".as_slice(), GPS_MARKER, TEXT_MARKER] {
        assert!(!contains(&stored, marker), "{}", String::from_utf8_lossy(marker));
    }
    assert_turned_upright(&stored);
}

/// A png chunk: its length, type, data and CRC-32 (of the type and data).
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = 0xffff_ffffu32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let length = u32::try_from(data.len()).unwrap().to_be_bytes();
    [length.as_slice(), kind, data, &(!crc).to_be_bytes()].concat()
}

/// The chunk types of a png, in order.
fn png_chunk_types(bytes: &[u8]) -> Vec<String> {
    let mut types = Vec::new();
    let mut rest = &bytes[8..];
    while rest.len() >= 12 {
        let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        types.push(String::from_utf8_lossy(&rest[4..8]).into_owned());
        rest = &rest[12 + length..];
    }
    types
}

/// A png's `eXIf` (with a GPS position and orientation 6), `tEXt` and `iTXt` (XMP) chunks are gone (SEC-CLI-96), and
/// the orientation is in the pixels.
#[test]
fn a_rich_text_png_is_stored_upright_without_its_exif_and_text() {
    let mut plain = Cursor::new(Vec::new());
    halves().write_to(&mut plain, ImageFormat::Png).unwrap();
    let plain = plain.into_inner();
    // the signature and IHDR (8 + 25 bytes), then the metadata, then the rest
    let (head, tail) = plain.split_at(33);
    let xmp = [
        b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta>".as_slice(),
        GPS_MARKER,
        b"</x:xmpmeta>",
    ]
    .concat();
    let png = [
        head,
        &png_chunk(b"eXIf", &exif_block(6)),
        &png_chunk(b"tEXt", &[b"Comment\0".as_slice(), TEXT_MARKER].concat()),
        &png_chunk(b"iTXt", &xmp),
        tail,
    ]
    .concat();
    let types = png_chunk_types(&png);
    assert!(
        ["eXIf", "tEXt", "iTXt"]
            .iter()
            .all(|kind| types.iter().any(|found| found == kind)),
        "{types:?}"
    );
    assert_eq!(decode_with_metadata(&png).unwrap().orientation, Orientation::Rotate90);

    let (ext, stored) = rich_text_image_bytes(&png).unwrap();
    assert_eq!(ext, "png");
    let types = png_chunk_types(&stored);
    for kind in ["eXIf", "tEXt", "iTXt", "zTXt"] {
        assert!(!types.iter().any(|found| found == kind), "{kind} in {types:?}");
    }
    for marker in [GPS_MARKER, TEXT_MARKER] {
        assert!(!contains(&stored, marker), "{}", String::from_utf8_lossy(marker));
    }
    assert_turned_upright(&stored);
}

/// The fourcc of each top-level chunk of a webp, and its `VP8X` flags.
fn webp_chunks(bytes: &[u8]) -> (Vec<String>, Option<u8>) {
    let mut kinds = Vec::new();
    let mut flags = None;
    let mut rest = &bytes[12..];
    while rest.len() >= 8 {
        let size = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
        if &rest[..4] == b"VP8X" {
            flags = Some(rest[8]);
        }
        kinds.push(String::from_utf8_lossy(&rest[..4]).into_owned());
        rest = rest.get(8 + size + size % 2..).unwrap_or_default();
    }
    (kinds, flags)
}

/// A webp's `EXIF` and `XMP ` chunks are gone and its `VP8X` flags for them cleared (SEC-CLI-96); the image data is
/// kept, so it decodes to the same pixels.
#[test]
fn a_rich_text_webp_is_stored_without_its_exif_and_xmp_chunks() {
    let plain = encode(&halves(), "webp").unwrap();
    assert_eq!(&plain[12..16], b"VP8L", "a simple lossless webp");
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let size = u32::try_from(data.len()).unwrap().to_le_bytes();
        let pad: &[u8] = if data.len() % 2 == 1 { &[0] } else { &[] };
        [kind.as_slice(), size.as_slice(), data, pad].concat()
    };
    // VP8X: the EXIF and XMP flags, three reserved bytes, the canvas less one a side in 24 bits
    let vp8x = [0x08 | 0x04, 0, 0, 0, 39, 0, 0, 19, 0, 0];
    // an odd-sized EXIF chunk, so its pad byte is read too
    let mut exif = exif_block(6);
    if exif.len().is_multiple_of(2) {
        exif.push(0);
    }
    let xmp = [b"<x:xmpmeta>".as_slice(), TEXT_MARKER, b"</x:xmpmeta>"].concat();
    let chunks = [
        chunk(b"VP8X", &vp8x),
        plain[12..].to_vec(),
        chunk(b"EXIF", &exif),
        chunk(b"XMP ", &xmp),
    ]
    .concat();
    let size = u32::try_from(chunks.len() + 4).unwrap().to_le_bytes();
    let webp = [b"RIFF".as_slice(), size.as_slice(), b"WEBP", &chunks].concat();
    assert_eq!(webp_chunks(&webp).0, ["VP8X", "VP8L", "EXIF", "XMP "]);
    assert!(exif_of(&webp).is_some_and(|block| contains(&block, GPS_MARKER)));
    let original = decode(&webp).unwrap().0.to_rgba8();

    let (ext, stored) = rich_text_image_bytes(&webp).unwrap();
    assert_eq!(ext, "webp");
    let (kinds, flags) = webp_chunks(&stored);
    assert_eq!(kinds, ["VP8X", "VP8L"]);
    assert_eq!(flags, Some(0));
    assert_eq!(
        u32::from_le_bytes(stored[4..8].try_into().unwrap()) as usize,
        stored.len() - 8
    );
    for marker in [GPS_MARKER, TEXT_MARKER] {
        assert!(!contains(&stored, marker), "{}", String::from_utf8_lossy(marker));
    }
    assert_eq!(exif_of(&stored), None);
    assert_eq!(decode(&stored).unwrap().0.to_rgba8(), original);
}

/// A context on a database of its own, with `conf`, and a user account for each of `users`.
fn upload_context(conf: &str, users: &[&str]) -> (ExecutionContext, EntityFacade) {
    let ctx = crate::context::ExecutionContextFactory::test_context_in(
        conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    {
        let _entered = ctx.enter();
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        for user in users {
            let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
            account.set("userId", Value::string(*user));
            account.set("username", Value::string(*user));
            account.create(&facade).unwrap();
        }
    }
    (ctx, facade)
}

fn sign_in(user: &str) {
    crate::user::logout().unwrap();
    crate::user::internal_login_user(user).unwrap();
}

fn upload_png(png: &[u8]) -> Result<String, String> {
    let parameters: BTreeMap<String, Value> = upload_file("pasted.png", "image/png", png.to_vec())
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    let out = crate::service::sync("org.moqui.resource.ResourceServices.upload#RichTextImage", &parameters)
        .unwrap_or_else(|err| panic!("upload: {err}"));
    let errors = message_errors();
    if errors.is_empty() {
        Ok(out_text(&out, "url"))
    } else {
        Err(errors)
    }
}

/// The stored files and the image rows, to show a refused upload stores neither.
fn stored_counts(facade: &EntityFacade) -> (i64, i64) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let files = facade
        .find("moqui.resource.DbResource")
        .unwrap()
        .eq("isFile", Value::string("Y"))
        .count()
        .unwrap();
    (files, facade.find(RICH_TEXT_IMAGE).unwrap().count().unwrap())
}

/// With `max-per-user="2"` a user's third image is refused and nothing of it stored, while another user still uploads
/// (SEC-CLI-92).
#[test]
fn a_users_rich_text_images_are_capped_by_count() {
    let (ctx, facade) = upload_context(
        r#"<moqui-conf><resource-facade><rich-text-images max-per-user="2"/></resource-facade></moqui-conf>"#,
        &["capped", "other"],
    );
    let _entered = ctx.enter();
    let png = png_with_alpha();
    sign_in("capped");
    for _ in 0..2 {
        upload_png(&png).unwrap();
    }
    assert_eq!(stored_counts(&facade), (2, 2));
    let refused = upload_png(&png).unwrap_err();
    assert!(refused.contains("the most a user may store"), "{refused}");
    assert_eq!(
        stored_counts(&facade),
        (2, 2),
        "nothing of the refused upload is stored"
    );
    sign_in("other");
    let url = upload_png(&png).unwrap();
    let row = image_row(&facade, url.trim_start_matches("/richTextImage/")).unwrap();
    assert_eq!(row.get("userId").map(to_plain_string).as_deref(), Some("other"));
}

/// Uploads of one user at the same moment are counted one after another: the upload locks the uploader's account row
/// before it counts the user's images (SEC-CLI-92). The in-memory driver holds no row lock across transactions, so
/// the statement and its place are what is checked.
#[test]
fn an_upload_locks_its_uploaders_account_before_counting() {
    let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let ctx = crate::context::ExecutionContextFactory::test_context_in(
        "<moqui-conf/>",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open_with_tape(std::sync::Arc::clone(&tape));
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", Value::string("racer"));
        account.set("username", Value::string("racer"));
        account.create(&facade).unwrap();
    }
    sign_in("racer");
    tape.lock().unwrap().clear();
    upload_png(&png_with_alpha()).unwrap();
    let statements = tape.lock().unwrap().clone();
    let locked = statements
        .iter()
        .position(|sql| sql.contains("\"user_account\"") && sql.ends_with(" FOR UPDATE"));
    let counted = statements.iter().position(|sql| sql.contains("\"rich_text_image\""));
    assert!(locked.is_some(), "{statements:#?}");
    assert!(locked < counted, "{statements:#?}");
}

/// A noisy png, which no compression makes small.
fn noise_png(side: u32) -> Vec<u8> {
    let mut state = 0x2545_f491u32;
    let image = ::image::RgbImage::from_fn(side, side, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let [r, g, b, _] = state.to_le_bytes();
        ::image::Rgb([r, g, b])
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// With `max-bytes-per-user` a small image passes and one whose stored bytes would take the user past it is refused,
/// with nothing stored; another small one still fits (SEC-CLI-92).
#[test]
fn a_users_rich_text_images_are_capped_by_bytes() {
    let (ctx, facade) = upload_context(
        r#"<moqui-conf><resource-facade><rich-text-images max-bytes-per-user="8192"/></resource-facade></moqui-conf>"#,
        &["bytes"],
    );
    let _entered = ctx.enter();
    sign_in("bytes");
    let small = png_with_alpha();
    upload_png(&small).unwrap();
    let large = noise_png(64);
    assert!(large.len() > 8192, "{}", large.len());
    let refused = upload_png(&large).unwrap_err();
    assert!(refused.contains("past 8192 bytes"), "{refused}");
    assert_eq!(
        stored_counts(&facade),
        (1, 1),
        "nothing of the refused upload is stored"
    );
    upload_png(&small).unwrap();
    assert_eq!(stored_counts(&facade), (2, 2));
}
