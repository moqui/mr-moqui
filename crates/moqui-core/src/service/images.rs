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

//! `org.moqui.impl.ImageServices` from `moqui-image`: convert an image to another format and to fixed widths, as the
//! Groovy on ImageIO and TwelveMonkeys did, here on the `image` crate.
//!
//! Formats read and written: jpg (jpeg), png, gif, tiff (tif), webp (written lossless), and bmp. Java also had wbmp,
//! and SVG input through batik; those are not here.
//!
//! `convert#Image` reads `inputImageStream` (the bytes) or `inputLocation`, and writes each output that is given: the
//! original, then the large, medium, small and thumbnail widths, keeping the aspect ratio with a triangle filter (Java's
//! `ResampleOp.FILTER_TRIANGLE`). An output is a resource location, or a resource reference from
//! `ec.resource.getLocationReference`. A format with no alpha (jpg, bmp) gets the image on white, as Java's fallback
//! did. When the encoder still refuses the image, it is written in the input's format with that extension, as Java's
//! retry did, and `contentLocation` names that file.
//!
//! Differences from Java: each width is resampled from the original, where Java resampled each from the one before;
//! the input and output sizes are limited (see [`MAX_INPUT_SIDE`], [`MAX_OUTPUT_SIDE`] and [`MAX_OUTPUT_PIXELS`]), where
//! Java had none; the writes run in the call, where Java started a service for each; the original is written as
//! `outputImageFormat` when that format can be written, where Java kept a png, gif, bmp or tiff original in its own
//! format; and `write#Image` takes the image as encoded bytes in `image`, where Java took an `IIOImage`.

use std::collections::BTreeMap;
use std::io::Cursor;

use super::util::param_text;

use ::image::imageops::FilterType;
use ::image::{DynamicImage, ImageFormat, ImageReader, Limits};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::web::multipart::UploadedFile;

/// The file suffixes of the enabled formats, read and written alike.
const SUFFIXES: [&str; 8] = ["jpg", "jpeg", "png", "gif", "tif", "tiff", "webp", "bmp"];
const MIME_TYPES: [&str; 6] = [
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/tiff",
    "image/webp",
    "image/bmp",
];

/// The widest or tallest image the decoder takes. The decoder's default allocation limit (512 MiB) holds as well.
const MAX_INPUT_SIDE: u32 = 30_000;
/// The widest or tallest resized image: WebP's limit, so every listed writer takes it.
const MAX_OUTPUT_SIDE: u32 = 16_383;
/// The most pixels in a resized image, 200 MB as RGBA.
const MAX_OUTPUT_PIXELS: u64 = 50_000_000;

/// The outputs of `convert#Image`: the parameter suffix and the width parameter (none for the original).
const OUTPUTS: [(&str, Option<(&str, u32)>); 5] = [
    ("Original", None),
    ("Large", Some(("pixelWidthLarge", 2160))),
    ("Medium", Some(("pixelWidthMedium", 1080))),
    ("Small", Some(("pixelWidthSmall", 360))),
    ("Thumbnail", Some(("pixelWidthThumbnail", 165))),
];

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, convert_image)?;
    rust_service!(map, write_image_service)?;
    rust_service!(map, upload_rich_text_image)?;
    rust_service!(map, get_rich_text_image)?;
    rust_service!(map, image_reader_file_suffixes)?;
    rust_service!(map, image_writer_file_suffixes)?;
    rust_service!(map, image_reader_mime_types)?;
    rust_service!(map, image_writer_mime_types)?;
    Ok(())
}

fn listed(key: &str, items: &[&str]) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    out.insert(
        key.to_string(),
        Value::List(items.iter().map(|item| Value::string(*item)).collect()),
    );
    Ok(out)
}

fn image_reader_file_suffixes(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    listed("imageReaderFileSuffixes", &SUFFIXES)
}

fn image_writer_file_suffixes(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    listed("imageWriterFileSuffixes", &SUFFIXES)
}

fn image_reader_mime_types(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    listed("imageReaderMIMETypes", &MIME_TYPES)
}

fn image_writer_mime_types(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    listed("imageWriterMIMETypes", &MIME_TYPES)
}

/// A resource location given as text, or as a resource reference (a map with its `location`).
fn location_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(location) if !location.is_empty() => Some(location.clone()),
        Value::Map(map) => map
            .get("location")
            .map(to_plain_string)
            .filter(|location| !location.is_empty()),
        _ => None,
    }
}

fn width(parameters: &BTreeMap<String, Value>, name: &str, default: u32) -> u32 {
    let given = match parameters.get(name) {
        Some(Value::I32(number)) => u32::try_from(*number).ok(),
        Some(Value::I64(number)) => u32::try_from(*number).ok(),
        Some(value) if !is_empty(value) => to_plain_string(value).parse().ok(),
        _ => None,
    };
    given.filter(|width| *width > 0).unwrap_or(default)
}

/// The image bytes from `name` (bytes, or text that is a resource location) or from the resource at `location_name`.
fn input_bytes(parameters: &BTreeMap<String, Value>, name: &str, location_name: &str) -> Result<Vec<u8>, LibError> {
    let location = match parameters.get(name) {
        Some(Value::Bytes(bytes)) if !bytes.is_empty() => return Ok(bytes.clone()),
        Some(Value::String(location)) if !location.is_empty() => location.clone(),
        _ => param_text(parameters, location_name),
    };
    if location.is_empty() {
        return Err(LibError::new(format!("No image given in {name} or {location_name}")));
    }
    let found = ExecutionContext::with_current(|ctx| ctx.factory().resource().get(&location))
        .ok_or_else(|| LibError::new("no execution context is entered"))?;
    match found {
        Ok(resource) if resource.exists() => resource.read_bytes().map_err(|err| LibError::new(err.message())),
        _ => Err(LibError::new(format!("Could not find image at [{location}]"))),
    }
}

/// Decode an image, and the name of its format as a file suffix.
fn decode(bytes: &[u8]) -> Result<(DynamicImage, String), LibError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| LibError::new(format!("Could not read image: {err}")))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_INPUT_SIDE);
    limits.max_image_height = Some(MAX_INPUT_SIDE);
    reader.limits(limits);
    let format = reader.format().ok_or_else(|| {
        LibError::new("No reader for the image: its format is not one of jpg, png, gif, tiff, webp, bmp")
    })?;
    let image = reader
        .decode()
        .map_err(|err| LibError::new(format!("Could not read image: {err}")))?;
    Ok((image, suffix_of(format).to_string()))
}

fn suffix_of(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Tiff => "tiff",
        other => other.extensions_str().first().copied().unwrap_or("bin"),
    }
}

fn format_of(name: &str) -> Option<ImageFormat> {
    ImageFormat::from_extension(name.to_ascii_lowercase()).filter(|format| format.writing_enabled())
}

/// The bytes of `image` in `format_name`. A format with no alpha channel gets the image on white.
fn encode(image: &DynamicImage, format_name: &str) -> Result<Vec<u8>, LibError> {
    let format = format_of(format_name).ok_or_else(|| {
        LibError::new(format!(
            "No writer for: {format_name}, the formats are {}",
            SUFFIXES.join(", ")
        ))
    })?;
    let flattened;
    let image = if matches!(format, ImageFormat::Jpeg | ImageFormat::Bmp) && image.color().has_alpha() {
        flattened = on_white(image);
        &flattened
    } else if matches!(format, ImageFormat::Jpeg)
        && !matches!(image, DynamicImage::ImageRgb8(_) | DynamicImage::ImageLuma8(_))
    {
        // jpeg takes 8-bit gray or color
        flattened = DynamicImage::ImageRgb8(image.to_rgb8());
        &flattened
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, format)
        .map_err(|err| LibError::new(format!("Could not write image as {format_name}: {err}")))?;
    Ok(out.into_inner())
}

/// The image drawn on a white background, as Java's `drawImage(image, 0, 0, Color.WHITE, null)`.
fn on_white(image: &DynamicImage) -> DynamicImage {
    let rgba = image.to_rgba8();
    let mut rgb = ::image::RgbImage::new(rgba.width(), rgba.height());
    for (target, source) in rgb.pixels_mut().zip(rgba.pixels()) {
        let alpha = u16::from(source[3]);
        for channel in 0..3 {
            let blended = (u16::from(source[channel]) * alpha + 255 * (255 - alpha) + 127) / 255;
            target[channel] = u8::try_from(blended).unwrap_or(u8::MAX);
        }
    }
    DynamicImage::ImageRgb8(rgb)
}

/// `width` wide, with the height that keeps the aspect ratio (the integer division Java did); a narrower image is
/// scaled up, as in Java. A size past [`MAX_OUTPUT_SIDE`] a side or [`MAX_OUTPUT_PIXELS`] in all is an error.
fn resized(image: &DynamicImage, width: u32) -> Result<DynamicImage, LibError> {
    let (from_width, from_height) = (image.width(), image.height());
    let width = width.max(1);
    let height = (u64::from(width) * u64::from(from_height) / u64::from(from_width.max(1))).max(1);
    if width > MAX_OUTPUT_SIDE || height > u64::from(MAX_OUTPUT_SIDE) || u64::from(width) * height > MAX_OUTPUT_PIXELS {
        return Err(LibError::new(format!(
            "Image of {from_width}x{from_height} would resize to {width}x{height}, past the limit of {MAX_OUTPUT_SIDE} pixels a side and {MAX_OUTPUT_PIXELS} in all"
        )));
    }
    let height = height as u32;
    if (width, height) == (from_width, from_height) {
        return Ok(image.clone());
    }
    Ok(image.resize_exact(width, height, FilterType::Triangle))
}

fn put_bytes(location: &str, bytes: &[u8]) -> Result<(), LibError> {
    ExecutionContext::with_current(|ctx| ctx.factory().resource().write_bytes(location, bytes))
        .ok_or_else(|| LibError::new("no execution context is entered"))?
        .map_err(|err| LibError::new(format!("Could not save image to {location}: {}", err.message())))
}

/// The largest rich-text image accepted, before it is decoded.
const MAX_RICH_TEXT_IMAGE: usize = 8 * 1024 * 1024;

/// `upload#RichTextImage`: a png, gif, jpeg or webp, stored as the original bytes at
/// `dbresource://moqui/rich-text/<32 hex>.<ext>`. The url is `/richTextImage/<id>` (the id is the capability). An svg,
/// a file that does not decode, and a file over 8 MiB are refused. The caller must be logged in.
fn upload_rich_text_image(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let bytes = content_file_bytes(parameters)?;
    let (_image, format) = decode(&bytes)?;
    let ext = match format.as_str() {
        "png" | "gif" | "jpg" | "webp" => format,
        _ => return Err(LibError::new("The image must be a png, gif, jpeg or webp")),
    };
    let id = rich_text_image_id()?;
    let location = format!("dbresource://moqui/rich-text/{id}.{ext}");
    put_bytes(&location, &bytes)?;
    let mut out = BTreeMap::new();
    out.insert("url".to_string(), Value::string(format!("/richTextImage/{id}")));
    Ok(out)
}

/// `get#RichTextImage`: the stored location of an id that is 32 lowercase hex digits, when that file exists.
fn get_rich_text_image(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = rich_text_image_location(&param_text(parameters, "imageId"))?;
    let mut out = BTreeMap::new();
    out.insert("location".to_string(), Value::string(location));
    Ok(out)
}

fn content_file_bytes(parameters: &BTreeMap<String, Value>) -> Result<Vec<u8>, LibError> {
    let Some(Value::Object(object)) = parameters.get("contentFile") else {
        return Err(LibError::new("contentFile is required"));
    };
    let Some(file) = object.downcast::<UploadedFile>() else {
        return Err(LibError::new("contentFile is not an uploaded file"));
    };
    // The size is known before the bytes are read, so a file over the limit is refused before the decode.
    if file.size() > MAX_RICH_TEXT_IMAGE as u64 {
        return Err(LibError::new("The image is larger than 8 MiB"));
    }
    let bytes = file.read_bytes().map_err(|err| LibError::new(err.message()))?;
    if bytes.len() > MAX_RICH_TEXT_IMAGE {
        return Err(LibError::new("The image is larger than 8 MiB"));
    }
    Ok(bytes)
}

fn rich_text_image_id() -> Result<String, LibError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| LibError::new(format!("could not read random bytes: {err}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The file for a 32-hex id, trying the formats the upload writes. Anything else is not an image.
fn rich_text_image_location(id: &str) -> Result<String, LibError> {
    if id.len() != 32 || !id.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(LibError::new("No image"));
    }
    for ext in ["png", "gif", "jpg", "webp"] {
        let location = format!("dbresource://moqui/rich-text/{id}.{ext}");
        let found = ExecutionContext::with_current(|ctx| ctx.factory().resource().get(&location))
            .ok_or_else(|| LibError::new("no execution context is entered"))?;
        if let Ok(resource) = found {
            if resource.exists() {
                return Ok(location);
            }
        }
    }
    Err(LibError::new("No image"))
}

/// Write `image` as `format_name` to `location`; when that format refuses it and `retry` holds, write it in the input
/// format to the same name with that extension. The location written.
fn write_image(
    image: &DynamicImage,
    location: &str,
    format_name: &str,
    input_format: &str,
    retry: bool,
) -> Result<String, LibError> {
    let (written, bytes) = encode_image(image, location, format_name, input_format, retry)?;
    put_bytes(&written, &bytes)?;
    Ok(written)
}

/// The bytes of `image` as `format_name`, for `location`; when that format refuses it and `retry` holds, in the input
/// format, for the same name with that extension. The location and the bytes.
fn encode_image(
    image: &DynamicImage,
    location: &str,
    format_name: &str,
    input_format: &str,
    retry: bool,
) -> Result<(String, Vec<u8>), LibError> {
    match encode(image, format_name) {
        Ok(bytes) => Ok((location.to_string(), bytes)),
        Err(err) => {
            let stem = location.rsplit_once('.').map_or(location, |(stem, _)| stem);
            let retry_location = format!("{stem}.{input_format}");
            if !retry || retry_location == location || format_of(input_format).is_none() {
                return Err(err);
            }
            tracing::warn!(
                "Retrying to write image from format {format_name} to {input_format} with a destination to {retry_location} instead of {location}: {}",
                err.message()
            );
            Ok((retry_location, encode(image, input_format)?))
        }
    }
}

/// Image conversions that may run at once (`resource-facade` `image-max-concurrent`, 2): a decoded image may take hundreds of MB.
struct Slots {
    used: std::sync::Mutex<usize>,
    freed: std::sync::Condvar,
}

/// A conversion's slot, given back when dropped.
struct Slot(&'static Slots);

impl Drop for Slot {
    fn drop(&mut self) {
        *moqui_lib::sync::lock(&self.0.used) -= 1;
        self.0.freed.notify_one();
    }
}

fn image_slot() -> Slot {
    static SLOTS: Slots = Slots {
        used: std::sync::Mutex::new(0),
        freed: std::sync::Condvar::new(),
    };
    let most = crate::resource::setting("image-max-concurrent")
        .parse::<usize>()
        .ok()
        .filter(|most| *most > 0)
        .unwrap_or(2);
    let mut used = moqui_lib::sync::lock(&SLOTS.used);
    while *used >= most {
        used = SLOTS
            .freed
            .wait(used)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    *used += 1;
    Slot(&SLOTS)
}

fn convert_image(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let output_format = param_text(parameters, "outputImageFormat");
    if output_format.is_empty() {
        return Err(LibError::new("outputImageFormat is required"));
    }
    let targets: Vec<(&str, Option<u32>, String)> = OUTPUTS
        .iter()
        .filter_map(|(suffix, sized)| {
            let location = location_of(parameters.get(&format!("outputResourceReference{suffix}")))?;
            let width = sized.map(|(name, default)| width(parameters, name, default));
            Some((*suffix, width, location))
        })
        .collect();
    let mut out = BTreeMap::new();
    for (suffix, _) in OUTPUTS {
        out.insert(format!("contentLocation{suffix}"), Value::Null);
    }
    if targets.is_empty() {
        return Ok(out);
    }
    let started = std::time::Instant::now();
    let bytes = input_bytes(parameters, "inputImageStream", "inputLocation")?;
    // every variant made first, then written together: one commit of a git:// repository
    let files = {
        let _slot = image_slot();
        let (image, input_format) = decode(&bytes)?;
        let mut files = Vec::with_capacity(targets.len());
        for (suffix, width, location) in targets {
            let encoded = match width {
                // the original is written as outputImageFormat, as its caller names the file, when that format can be
                // written, and in the input's format otherwise
                None => {
                    let format = if format_of(&output_format).is_some() {
                        output_format.as_str()
                    } else {
                        input_format.as_str()
                    };
                    encode_image(&image, &location, format, &input_format, true)?
                }
                Some(width) => encode_image(&resized(&image, width)?, &location, &output_format, &input_format, true)?,
            };
            out.insert(format!("contentLocation{suffix}"), Value::String(encoded.0.clone()));
            files.push(encoded);
        }
        files
    };
    ExecutionContext::with_current(|ctx| ctx.factory().resource().write_many(&files))
        .ok_or_else(|| LibError::new("no execution context is entered"))?
        .map_err(|err| LibError::new(format!("Could not save the images: {}", err.message())))?;
    tracing::info!(
        "Finished ImageServices.convert took milliseconds {}",
        started.elapsed().as_millis()
    );
    Ok(out)
}

/// `write#Image`: the image bytes in `image` written as `outputImageFormat` to `outputResourceReference`.
fn write_image_service(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = location_of(parameters.get("outputResourceReference"))
        .ok_or_else(|| LibError::new("outputResourceReference is required"))?;
    let output_format = param_text(parameters, "outputImageFormat");
    let bytes = input_bytes(parameters, "image", "inputLocation")?;
    let (image, found_format) = decode(&bytes)?;
    let input_format = Some(param_text(parameters, "inputImageFormat"))
        .filter(|name| !name.is_empty())
        .unwrap_or(found_format);
    let retry = !matches!(param_text(parameters, "retryIfFail").as_str(), "false" | "N");
    let written = write_image(&image, &location, &output_format, &input_format, retry)?;
    let mut out = BTreeMap::new();
    out.insert("contentLocation".to_string(), Value::String(written));
    out.insert("succeeded".to_string(), Value::Bool(true));
    Ok(out)
}

#[cfg(test)]
mod tests {
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

    /// The variants of an image go into a git repository in one commit, its PNGs in LFS.
    #[cfg(feature = "git")]
    #[test]
    fn the_variants_of_an_image_are_one_commit() {
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        ctx.factory().resource().install_git().unwrap();
        let _entered = ctx.enter();
        let mut repository = facade.make_value("moqui.resource.GitRepository").unwrap();
        repository.set("repositoryId", Value::string("media"));
        repository.set("repositoryKind", Value::string("db"));
        repository.create(&facade).unwrap();
        let resources = ctx.factory().resource();
        let provider = resources.git().unwrap().clone();
        provider
            .setup_lfs("media", &crate::resource::git::attributes::DEFAULT_LFS_PATTERNS)
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
        convert_image(&parameters, &facade).unwrap();
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
        let (small, _) = decode(
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

    fn service_call(
        name: &str,
        parameters: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> BTreeMap<String, Value> {
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
}
