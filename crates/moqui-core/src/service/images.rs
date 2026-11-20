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

use ::image::codecs::jpeg::JpegEncoder;
use ::image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use ::image::imageops::FilterType;
use ::image::metadata::Orientation;
use ::image::{DynamicImage, ImageDecoder, ImageEncoder, ImageError, ImageFormat, ImageReader, Limits};
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

/// The widest or tallest image the decoder takes.
const MAX_INPUT_SIDE: u32 = 30_000;
/// The most pixels of an image the decoder takes, read from its header before anything is decoded (SEC-SCR-85): 64
/// megapixels, past a phone's photo. The allocation limit is what that needs at 8 bytes a pixel (16-bit RGBA), 512 MiB.
const MAX_INPUT_PIXELS: u64 = 64_000_000;
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

/// The image in `bytes`, decoded within the same limits as `convert#Image` (no side over 30,000 pixels, the
/// decoder's allocation limit) and in its concurrency slot (`image-max-concurrent`, SEC-SCR-85), or None. For the
/// thumbnails of `ec.files` (SEC-SCR-09).
pub(crate) fn decode_limited(bytes: &[u8]) -> Option<DynamicImage> {
    let _slot = image_slot();
    decode(bytes).ok().map(|(image, _)| image)
}

/// Decode an image, and the name of its format as a file suffix.
fn decode(bytes: &[u8]) -> Result<(DynamicImage, String), LibError> {
    let (reader, format) = limited_reader(bytes)?;
    let image = reader.decode().map_err(read_error)?;
    Ok((image, suffix_of(format).to_string()))
}

/// The decoder's limits: no side over [`MAX_INPUT_SIDE`], and what [`MAX_INPUT_PIXELS`] needs at most.
fn input_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_INPUT_SIDE);
    limits.max_image_height = Some(MAX_INPUT_SIDE);
    limits.max_alloc = Some(MAX_INPUT_PIXELS * 8);
    limits
}

/// The reader [`limited_reader`] makes.
type BytesReader<'a> = ImageReader<Cursor<&'a [u8]>>;

/// A reader of `bytes` within [`input_limits`], and the format it found.
fn limited_reader(bytes: &[u8]) -> Result<(BytesReader<'_>, ImageFormat), LibError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| LibError::new(format!("Could not read image: {err}")))?;
    reader.limits(input_limits());
    let format = reader.format().ok_or_else(|| {
        LibError::new("No reader for the image: its format is not one of jpg, png, gif, tiff, webp, bmp")
    })?;
    // its sides from its header, before any pixel is decoded: an image of more pixels than MAX_INPUT_PIXELS is refused
    let mut header = ImageReader::with_format(Cursor::new(bytes), format);
    header.limits(input_limits());
    let (width, height) = header.into_dimensions().map_err(read_error)?;
    if u64::from(width) * u64::from(height) > MAX_INPUT_PIXELS {
        return Err(LibError::new(format!(
            "Could not read image: {width}x{height} is more than {MAX_INPUT_PIXELS} pixels"
        )));
    }
    Ok((reader, format))
}

fn read_error(err: ImageError) -> LibError {
    LibError::new(format!("Could not read image: {err}"))
}

/// An image decoded as [`decode`] does, with what is needed to draw it again without its metadata.
struct Decoded {
    image: DynamicImage,
    format: ImageFormat,
    /// The EXIF orientation, which a viewer applies when it shows the image.
    orientation: Orientation,
    /// The color profile, written again so a re-encoded photo keeps its colors.
    icc_profile: Option<Vec<u8>>,
}

fn decode_with_metadata(bytes: &[u8]) -> Result<Decoded, LibError> {
    let (reader, format) = limited_reader(bytes)?;
    let mut decoder = reader.into_decoder().map_err(read_error)?;
    // the whole image held to the allocation limit before it is read, as ImageReader::decode does
    input_limits().reserve(decoder.total_bytes()).map_err(read_error)?;
    // an orientation or profile that does not parse is left out: the pixels are what is checked
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let icc_profile = decoder
        .icc_profile()
        .ok()
        .flatten()
        .filter(|profile| !profile.is_empty());
    let image = DynamicImage::from_decoder(decoder).map_err(read_error)?;
    Ok(Decoded {
        image,
        format,
        orientation,
        icc_profile,
    })
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

/// The quality a rich-text jpeg is written at when it is drawn again without its metadata (SEC-CLI-96).
const RICH_TEXT_JPEG_QUALITY: u8 = 90;
/// One row for each stored rich-text image: its uploader and size (SEC-CLI-92).
const RICH_TEXT_IMAGE: &str = "moqui.resource.RichTextImage";
/// A user's rich-text images counted and their bytes summed.
const RICH_TEXT_IMAGE_TOTAL: &str = "moqui.resource.RichTextImageTotal";
/// The `resource-facade/rich-text-images` defaults, for a conf that leaves them out or empty.
const DEFAULT_IMAGES_PER_USER: u64 = 1000;
const DEFAULT_IMAGE_BYTES_PER_USER: u64 = 200 * 1024 * 1024;

/// `upload#RichTextImage`: a png, gif, jpeg or webp, stored at `dbresource://moqui/rich-text/<32 hex>.<ext>` as
/// [`rich_text_image_bytes`] makes it, with a `moqui.resource.RichTextImage` row naming the uploader and its size. The
/// url is `/richTextImage/<id>` (the id is the capability). An svg, a file that does not decode, a file over 8 MiB,
/// and an image past the uploader's `resource-facade/rich-text-images` limits are refused, and nothing is stored. The
/// caller must be logged in.
fn upload_rich_text_image(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let user_id = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| LibError::new("Sign in to upload an image"))?;
    let bytes = content_file_bytes(parameters)?;
    // SEC-CLI-92: a user at either limit is refused before the image is decoded, and one it would pass before it is
    // written. The counts are read in the slot, so an upload waiting for one holds no database connection. Uploads of
    // one user at the same moment are not serialized: each that passes adds its one image.
    let limits = RichTextImageLimits::read();
    let (ext, stored, size) = {
        let _slot = image_slot();
        let used = rich_text_image_usage(facade, &user_id)?;
        limits.check(used, 0)?;
        let (ext, stored) = rich_text_image_bytes(&bytes)?;
        let size = u64::try_from(stored.len()).unwrap_or(u64::MAX);
        limits.check(used, size)?;
        (ext, stored, size)
    };
    let id = rich_text_image_id()?;
    let location = format!("dbresource://moqui/rich-text/{id}.{ext}");
    // the file and its row in the service's transaction: one is not kept without the other
    put_bytes(&location, &stored)?;
    record_rich_text_image(facade, &id, &location, &user_id, size)?;
    let mut out = BTreeMap::new();
    out.insert("url".to_string(), Value::string(format!("/richTextImage/{id}")));
    Ok(out)
}

/// What a rich-text upload stores, and its extension (SEC-CLI-96). A jpeg or png is drawn again from its pixels, turned
/// as its EXIF orientation says first so a phone photo still shows upright: its EXIF (a phone's GPS position, device
/// and time), XMP, comments and text chunks are not written; its color profile is. A webp keeps its bytes but only
/// the chunks that draw it ([`webp_without_metadata`]). A gif is stored as uploaded. Each is decoded first, so a file
/// that does not decode is refused; any other format is too.
fn rich_text_image_bytes(bytes: &[u8]) -> Result<(&'static str, Vec<u8>), LibError> {
    let decoded = decode_with_metadata(bytes)?;
    match decoded.format {
        ImageFormat::Jpeg => Ok(("jpg", reencoded(decoded)?)),
        ImageFormat::Png => Ok(("png", reencoded(decoded)?)),
        ImageFormat::WebP => Ok(("webp", webp_without_metadata(bytes)?)),
        ImageFormat::Gif => Ok(("gif", bytes.to_vec())),
        _ => Err(LibError::new("The image must be a png, gif, jpeg or webp")),
    }
}

/// A decoded jpeg or png written again in its format, upright, with its color profile and no other metadata: a jpeg
/// at [`RICH_TEXT_JPEG_QUALITY`], a png at the default compression.
fn reencoded(decoded: Decoded) -> Result<Vec<u8>, LibError> {
    let Decoded {
        mut image,
        format,
        orientation,
        icc_profile,
    } = decoded;
    image.apply_orientation(orientation);
    let write_error = |err: ImageError| LibError::new(format!("Could not write image: {err}"));
    let mut out = Vec::new();
    if format == ImageFormat::Jpeg {
        // a jpeg decodes to 8-bit gray or color, which the encoder takes; anything else is made color
        if !matches!(image, DynamicImage::ImageRgb8(_) | DynamicImage::ImageLuma8(_)) {
            image = DynamicImage::ImageRgb8(image.to_rgb8());
        }
        let mut encoder = JpegEncoder::new_with_quality(&mut out, RICH_TEXT_JPEG_QUALITY);
        if let Some(profile) = icc_profile {
            // a profile the encoder cannot hold is left out
            let _ = encoder.set_icc_profile(profile);
        }
        image.write_with_encoder(encoder).map_err(write_error)?;
    } else {
        let mut encoder = PngEncoder::new_with_quality(&mut out, CompressionType::Default, PngFilter::Adaptive);
        if let Some(profile) = icc_profile {
            let _ = encoder.set_icc_profile(profile);
        }
        image.write_with_encoder(encoder).map_err(write_error)?;
    }
    Ok(out)
}

/// The chunks of a webp that draw it; every other chunk (`EXIF`, `XMP `, and any a decoder does not read) is dropped.
const WEBP_KEPT_CHUNKS: [&[u8; 4]; 7] = [b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP"];
/// The flags of a `VP8X` header that say an `EXIF` and an `XMP ` chunk follow.
const VP8X_EXIF_FLAG: u8 = 0x08;
const VP8X_XMP_FLAG: u8 = 0x04;

/// A webp's bytes with only [`WEBP_KEPT_CHUNKS`] (SEC-CLI-96): its EXIF and XMP chunks are left out and its `VP8X`
/// header's flags for them cleared to match, the RIFF size written again, and anything after the RIFF data dropped.
/// The image data is kept as uploaded. A container whose chunks do not fit in it is refused.
fn webp_without_metadata(bytes: &[u8]) -> Result<Vec<u8>, LibError> {
    let malformed = || LibError::new("Could not read image: the webp container is not well formed");
    let (header, body) = bytes.split_at_checked(12).ok_or_else(malformed)?;
    let (riff, rest) = header.split_at(4);
    let (riff_size, webp) = rest.split_at(4);
    if riff != b"RIFF" || webp != b"WEBP" {
        return Err(malformed());
    }
    let riff_size = u32::from_le_bytes(riff_size.try_into().map_err(|_| malformed())?);
    // the RIFF size counts "WEBP" and the chunks after it
    let chunks_size = usize::try_from(riff_size)
        .map_err(|_| malformed())?
        .checked_sub(4)
        .ok_or_else(malformed)?;
    let mut rest = body.get(..chunks_size).ok_or_else(malformed)?;
    let mut chunks = Vec::with_capacity(rest.len());
    while !rest.is_empty() {
        let (chunk_header, after) = rest.split_at_checked(8).ok_or_else(malformed)?;
        let (fourcc, size) = chunk_header.split_at(4);
        let size =
            usize::try_from(u32::from_le_bytes(size.try_into().map_err(|_| malformed())?)).map_err(|_| malformed())?;
        let (payload, after) = after.split_at_checked(size).ok_or_else(malformed)?;
        // a chunk of odd size has a pad byte, which a last chunk may leave out
        rest = if size % 2 == 1 {
            after.get(1..).unwrap_or_default()
        } else {
            after
        };
        if !WEBP_KEPT_CHUNKS.iter().any(|kept| fourcc == kept.as_slice()) {
            continue;
        }
        chunks.extend_from_slice(chunk_header);
        let start = chunks.len();
        chunks.extend_from_slice(payload);
        if fourcc == b"VP8X" {
            if let Some(flags) = chunks.get_mut(start) {
                *flags &= !(VP8X_EXIF_FLAG | VP8X_XMP_FLAG);
            }
        }
        if size % 2 == 1 {
            chunks.push(0);
        }
    }
    let riff_size = u32::try_from(chunks.len() + 4).map_err(|_| malformed())?;
    let mut out = Vec::with_capacity(chunks.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&chunks);
    Ok(out)
}

/// The `resource-facade/rich-text-images` limits of one user's images (SEC-CLI-92).
struct RichTextImageLimits {
    max_count: u64,
    max_bytes: u64,
}

/// A user's stored rich-text images: how many, and their bytes in all.
#[derive(Clone, Copy)]
struct RichTextImageUsage {
    count: u64,
    bytes: u64,
}

impl RichTextImageLimits {
    /// The settings now; one that is empty, zero or not a number is its default.
    fn read() -> Self {
        let number = |name: &str, default: u64| {
            crate::context::conf_env::setting(&format!("resource-facade/rich-text-images@{name}"))
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        Self {
            max_count: number("max-per-user", DEFAULT_IMAGES_PER_USER),
            max_bytes: number("max-bytes-per-user", DEFAULT_IMAGE_BYTES_PER_USER),
        }
    }

    /// Refused when `used` and one more image of `size` bytes would pass either limit. With `size` 0 (before the
    /// decode), a user already at the byte limit is refused too.
    fn check(&self, used: RichTextImageUsage, size: u64) -> Result<(), LibError> {
        if used.count >= self.max_count {
            return Err(LibError::new(format!(
                "You have stored {} rich-text images, the most a user may store",
                used.count
            )));
        }
        let total = used.bytes.saturating_add(size);
        if total > self.max_bytes || (size == 0 && used.bytes >= self.max_bytes) {
            const MIB: u64 = 1024 * 1024;
            let limit = if self.max_bytes.is_multiple_of(MIB) {
                format!("{} MiB", self.max_bytes / MIB)
            } else {
                format!("{} bytes", self.max_bytes)
            };
            return Err(LibError::new(format!(
                "The image would take your rich-text images past {limit}, the most a user may store"
            )));
        }
        Ok(())
    }
}

fn rich_text_image_usage(facade: &EntityFacade, user_id: &str) -> Result<RichTextImageUsage, LibError> {
    let total = facade
        .find(RICH_TEXT_IMAGE_TOTAL)?
        .eq("userId", Value::string(user_id))
        .disable_authz()
        .one()?;
    let number = |name: &str| {
        total
            .as_ref()
            .and_then(|row| row.get(name))
            .map(to_plain_string)
            .and_then(|text| text.trim().parse::<f64>().ok())
            .map_or(0, |number| number.max(0.0) as u64)
    };
    Ok(RichTextImageUsage {
        count: number("imageCount"),
        bytes: number("totalBytes"),
    })
}

fn record_rich_text_image(
    facade: &EntityFacade,
    id: &str,
    location: &str,
    user_id: &str,
    size: u64,
) -> Result<(), LibError> {
    // the framework's own record of the upload, written whatever the caller may write
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(RICH_TEXT_IMAGE)?;
    row.set("imageId", Value::string(id));
    row.set("location", Value::string(location));
    row.set("userId", Value::string(user_id));
    row.set("fileSize", Value::I64(i64::try_from(size).unwrap_or(i64::MAX)));
    row.set("createdDate", crate::user::now_value());
    row.create(facade)?;
    Ok(())
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

/// Image conversions and rich-text uploads that may run at once (`resource-facade` `image-max-concurrent`, 2): a decoded
/// image may take hundreds of MB.
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
        used = moqui_lib::sync::wait(&SLOTS.freed, used);
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
    // decoded and written in the concurrency slot, as convert#Image is (SEC-SCR-85)
    let _slot = image_slot();
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
#[path = "images_tests.rs"]
mod tests;
