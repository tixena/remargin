//! Image sampling and cropping for inline inspection.
//!
//! Reads a raster image (PNG / JPEG / GIF / WebP) from the managed tree,
//! optionally crops a sub-region, downscales to fit a max dimension budget,
//! and re-encodes to a target byte budget. The result is a fresh in-memory
//! image bytes payload the caller can return as base64 — small enough to fit
//! inside an MCP tool-result envelope without tripping token limits.
//!
//! Non-image binaries (PDF, audio, video) are rejected with a clear error
//! pointing the caller back to `get --binary` (or the appropriate viewer).
//! Markdown is rejected by the underlying `read_binary` call.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView as _, ImageFormat};
use os_shim::System;
use serde::Serialize;
use serde_json::{Value, json};

use crate::document::{self, mime};

/// Default upper bound on width/height for the sampled output.
const DEFAULT_MAX_DIMENSION: u32 = 1024;

/// Default target ceiling on the encoded output byte size.
const DEFAULT_MAX_BYTES: u64 = 256 * 1024;

/// Hard floor on the byte budget — below this we cannot produce a useful
/// thumbnail regardless of the encoder, so we reject upfront.
const MIN_MAX_BYTES: u64 = 1024;

/// JPEG quality used on the first encode pass. Subsequent passes step
/// down by [`JPEG_QUALITY_STEP`] until either the output fits or
/// [`JPEG_QUALITY_FLOOR`] is reached.
const JPEG_QUALITY_INITIAL: u8 = 85;
const JPEG_QUALITY_STEP: u8 = 10;
const JPEG_QUALITY_FLOOR: u8 = 30;

/// When even the quality floor overshoots the byte budget, the encoder
/// halves the dimension cap and retries. The retry stops when the
/// dimension cap drops below this floor (output gets useless smaller).
const DIMENSION_RETRY_FLOOR: u32 = 64;

/// Output encoding for the sampled image.
///
/// PNG preserves transparency and gives lossless thumbnails for source
/// PNGs; JPEG gives a tunable byte budget for photographic content.
/// The encoder picks JPEG by default because the byte-budget step
/// relies on quality tuning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OutputFormat {
    Jpeg,
    Png,
}

impl OutputFormat {
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }

    /// # Errors
    ///
    /// Returns an error when `value` is not one of `jpeg`, `jpg`, or `png`
    /// (case-insensitive).
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            other => bail!("unsupported output format: {other} (expected jpeg or png)"),
        }
    }
}

/// Crop region in pixels, applied before scaling. Bounds are clamped to
/// the image's dimensions; an empty intersection is rejected.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CropRegion {
    pub height: u32,
    pub width: u32,
    pub x: u32,
    pub y: u32,
}

impl CropRegion {
    /// Parse a `X,Y,W,H` 4-tuple as emitted by the CLI `--crop` flag.
    ///
    /// # Errors
    ///
    /// Returns an error when the spec has the wrong number of fields or
    /// any field fails to parse as a non-negative integer.
    pub fn parse(spec: &str) -> Result<Self> {
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            bail!("crop must be X,Y,W,H (got {} parts)", parts.len());
        }
        let parse_u32 = |raw: &str, name: &str| -> Result<u32> {
            raw.parse::<u32>().with_context(|| {
                format!("crop {name} must be a non-negative integer (got {raw:?})")
            })
        };
        Ok(Self {
            x: parse_u32(parts[0], "x")?,
            y: parse_u32(parts[1], "y")?,
            width: parse_u32(parts[2], "width")?,
            height: parse_u32(parts[3], "height")?,
        })
    }
}

/// Caller-supplied sampling knobs. All fields are optional; defaults are
/// chosen to keep output well under typical MCP tool-result token caps.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct SampleOptions {
    pub crop: Option<CropRegion>,
    pub format: Option<OutputFormat>,
    pub max_bytes: Option<u64>,
    pub max_dimension: Option<u32>,
}

impl SampleOptions {
    /// Build a `SampleOptions` from the four CLI-flag shapes
    /// (`--crop X,Y,W,H`, `--format jpeg|png`, `--max-bytes N`,
    /// `--max-dimension N`). Each `Option` is the parsed CLI value or
    /// `None` when the user omitted the flag.
    ///
    /// # Errors
    ///
    /// Returns an error when the crop spec or format string is malformed.
    pub fn from_optionals(
        crop: Option<&str>,
        format: Option<&str>,
        max_bytes: Option<u64>,
        max_dimension: Option<u32>,
    ) -> Result<Self> {
        let mut options = Self::new();
        if let Some(spec) = crop {
            options = options.with_crop(CropRegion::parse(spec)?);
        }
        if let Some(raw) = format {
            options = options.with_format(OutputFormat::parse(raw)?);
        }
        if let Some(bytes) = max_bytes {
            options = options.with_max_bytes(bytes);
        }
        if let Some(dim) = max_dimension {
            options = options.with_max_dimension(dim);
        }
        Ok(options)
    }

    /// Owning constructor — callers outside the crate use this instead
    /// of struct-literal construction (the type is `non_exhaustive`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            crop: None,
            format: None,
            max_bytes: None,
            max_dimension: None,
        }
    }

    /// Builder-style setter for the crop region.
    #[must_use]
    pub const fn with_crop(mut self, crop: CropRegion) -> Self {
        self.crop = Some(crop);
        self
    }

    /// Builder-style setter for the output format override.
    #[must_use]
    pub const fn with_format(mut self, format: OutputFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Builder-style setter for the byte budget.
    #[must_use]
    pub const fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }

    /// Builder-style setter for the dimension cap.
    #[must_use]
    pub const fn with_max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }
}

/// Output of [`sample_image`]. `bytes` holds the re-encoded image; the
/// other fields describe what was actually produced so the caller can
/// surface it without re-decoding.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct SampleResult {
    pub bytes: Vec<u8>,
    pub format: &'static str,
    pub height: u32,
    pub mime: &'static str,
    pub source_height: u32,
    pub source_mime: &'static str,
    pub source_path: PathBuf,
    pub source_size_bytes: u64,
    pub source_width: u32,
    pub width: u32,
}

impl SampleResult {
    /// JSON shape returned to MCP callers. `content` is base64-encoded
    /// by the adapter so this stays adapter-agnostic.
    #[must_use]
    pub fn to_json_without_content(&self) -> Value {
        json!({
            "format": self.format,
            "height": self.height,
            "mime": self.mime,
            "size_bytes": self.bytes.len(),
            "source": {
                "height": self.source_height,
                "mime": self.source_mime,
                "path": self.source_path,
                "size_bytes": self.source_size_bytes,
                "width": self.source_width,
            },
            "width": self.width,
        })
    }
}

/// Sample (and optionally crop) an image attachment to fit a byte budget.
///
/// Steps:
/// 1. Read bytes through the sandboxed [`document::read_binary`] surface.
/// 2. Reject non-image mimes.
/// 3. Decode, crop (if requested), and downscale to the dimension cap.
/// 4. Encode to the requested format, stepping JPEG quality down until
///    the byte budget fits. PNG output skips the quality step (lossless).
///
/// # Errors
///
/// Returns an error if the path is invalid, the file is not a raster
/// image, decoding fails, the crop is out of bounds, or the byte budget
/// cannot be met even at the dimension/quality floor.
pub fn sample_image(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    unrestricted: bool,
    trusted_roots: &[PathBuf],
    options: &SampleOptions,
) -> Result<SampleResult> {
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    if max_bytes < MIN_MAX_BYTES {
        bail!(
            "max_bytes must be at least {MIN_MAX_BYTES} (got {max_bytes}); below this no useful \
             thumbnail can be produced"
        );
    }

    let payload = document::read_binary(system, base_dir, path, unrestricted, trusted_roots)?;
    let source_mime = payload.mime;
    if !source_mime.starts_with("image/") || source_mime == "image/svg+xml" {
        bail!(
            "sample only supports raster images (PNG, JPEG, GIF, WebP); got {source_mime}. For \
             other binaries use `get --binary`."
        );
    }

    let source_format = detect_source_format(source_mime)
        .with_context(|| format!("unsupported source format for sample: {source_mime}"))?;

    let image = image::load_from_memory_with_format(&payload.bytes, source_format)
        .context("decoding source image")?;
    let (source_width, source_height) = image.dimensions();

    let cropped = apply_crop(image, options.crop, source_width, source_height)?;
    let dimension_cap = options
        .max_dimension
        .unwrap_or(DEFAULT_MAX_DIMENSION)
        .max(1);
    let scaled = downscale(cropped, dimension_cap);

    let format = options
        .format
        .unwrap_or_else(|| pick_default_format(source_format));
    let (encoded, final_image) = encode_to_budget(scaled, format, max_bytes, dimension_cap)?;
    let (out_width, out_height) = final_image.dimensions();

    Ok(SampleResult {
        bytes: encoded,
        format: format_label(format),
        height: out_height,
        mime: format.mime(),
        source_height,
        source_mime: mime::mime_for_extension(&payload.path),
        source_path: payload.path,
        source_size_bytes: payload.size_bytes,
        source_width,
        width: out_width,
    })
}

/// String label used in JSON output. Matches the values
/// [`OutputFormat::parse`] accepts so round-tripping works (`jpeg`,
/// not `jpg`).
const fn format_label(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Jpeg => "jpeg",
        OutputFormat::Png => "png",
    }
}

/// Map the source MIME to an [`ImageFormat`]. Only the formats the image
/// crate's default feature set actually decodes are accepted.
fn detect_source_format(mime: &str) -> Result<ImageFormat> {
    match mime {
        "image/png" => Ok(ImageFormat::Png),
        "image/jpeg" => Ok(ImageFormat::Jpeg),
        "image/gif" => Ok(ImageFormat::Gif),
        "image/webp" => Ok(ImageFormat::WebP),
        other => bail!("unsupported source image mime: {other}"),
    }
}

/// When the caller does not pin an output format, prefer JPEG for
/// photographic source formats (JPEG / WebP) and PNG for lossless source
/// formats (PNG / GIF) so we do not blow up small line-art with JPEG
/// artefacts on the first pass. Other source formats never reach this
/// function — `detect_source_format` rejects them upstream — but the
/// catch-all keeps the match exhaustive without churning on future
/// `image` crate variants.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "detect_source_format already restricts the input set; the catch-all keeps the \
              match resilient to future image-crate variants without forcing a churn here."
)]
const fn pick_default_format(source: ImageFormat) -> OutputFormat {
    match source {
        ImageFormat::Png | ImageFormat::Gif => OutputFormat::Png,
        _ => OutputFormat::Jpeg,
    }
}

/// Apply the caller's crop region after clamping to the source bounds.
/// An empty intersection is an error rather than a silent fallback to
/// "no crop" — the caller asked for a region, returning the full image
/// would be misleading.
fn apply_crop(
    image: DynamicImage,
    crop: Option<CropRegion>,
    source_width: u32,
    source_height: u32,
) -> Result<DynamicImage> {
    let Some(region) = crop else {
        return Ok(image);
    };
    if region.x >= source_width || region.y >= source_height {
        bail!(
            "crop origin ({x},{y}) is outside the source image ({source_width}x{source_height})",
            x = region.x,
            y = region.y,
        );
    }
    let width = region.width.min(source_width.saturating_sub(region.x));
    let height = region.height.min(source_height.saturating_sub(region.y));
    if width == 0 || height == 0 {
        bail!(
            "crop region collapses to zero area within the source image \
             ({source_width}x{source_height})"
        );
    }
    Ok(image.crop_imm(region.x, region.y, width, height))
}

/// Lanczos3 downscale to fit inside `max_dim` on the longer edge. Aspect
/// ratio is preserved. Skips the resize when the image already fits.
fn downscale(image: DynamicImage, max_dim: u32) -> DynamicImage {
    let (w, h) = image.dimensions();
    if w <= max_dim && h <= max_dim {
        return image;
    }
    image.resize(max_dim, max_dim, FilterType::Lanczos3)
}

/// Encode to the chosen format, stepping JPEG quality down until the
/// byte budget fits. If even the quality floor overshoots, retries with
/// progressively smaller dimension caps. PNG output cannot be quality-
/// tuned, so it falls back to dimension retries directly.
fn encode_to_budget(
    image: DynamicImage,
    format: OutputFormat,
    max_bytes: u64,
    initial_dim_cap: u32,
) -> Result<(Vec<u8>, DynamicImage)> {
    let mut current = image;
    let mut dim_cap = initial_dim_cap;
    loop {
        let encoded = match format {
            OutputFormat::Jpeg => encode_jpeg_to_budget(&current, max_bytes)?,
            OutputFormat::Png => encode_png(&current)?,
        };
        if encoded.len() as u64 <= max_bytes {
            return Ok((encoded, current));
        }
        let next_cap = dim_cap / 2;
        if next_cap < DIMENSION_RETRY_FLOOR {
            bail!(
                "cannot fit sample within {max_bytes} bytes even at {DIMENSION_RETRY_FLOOR}px / \
                 quality floor; raise max_bytes or pick a tighter crop"
            );
        }
        dim_cap = next_cap;
        current = downscale(current, dim_cap);
    }
}

/// Encode the image as JPEG, stepping quality down from
/// [`JPEG_QUALITY_INITIAL`] until the encoded size fits within
/// `max_bytes` or [`JPEG_QUALITY_FLOOR`] is reached. Returns the last
/// encode (which may still be larger than the budget; the outer loop
/// handles a dimension retry in that case).
fn encode_jpeg_to_budget(image: &DynamicImage, max_bytes: u64) -> Result<Vec<u8>> {
    let rgb = image.to_rgb8();
    let mut quality = JPEG_QUALITY_INITIAL;
    loop {
        let mut buffer: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);
        let encoder = JpegEncoder::new_with_quality(&mut cursor, quality);
        rgb.write_with_encoder(encoder).context("encoding JPEG")?;
        if (buffer.len() as u64) <= max_bytes || quality <= JPEG_QUALITY_FLOOR {
            return Ok(buffer);
        }
        quality = quality
            .saturating_sub(JPEG_QUALITY_STEP)
            .max(JPEG_QUALITY_FLOOR);
    }
}

/// Lossless PNG encode. No quality knob — byte budget enforcement falls
/// to the outer dimension retry loop.
fn encode_png(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    image
        .write_to(&mut cursor, ImageFormat::Png)
        .context("encoding PNG")?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::{
        CropRegion, DEFAULT_MAX_BYTES, MIN_MAX_BYTES, OutputFormat, SampleOptions, sample_image,
    };
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageBuffer, ImageFormat, Rgb, RgbImage};
    use os_shim::mock::MockSystem;
    use std::io::Cursor;
    use std::path::Path;

    fn write_png(width: u32, height: u32) -> Vec<u8> {
        let mut img: RgbImage = ImageBuffer::new(width, height);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let r = u8::try_from((x * 255) / width.max(1)).unwrap_or(255);
            let g = u8::try_from((y * 255) / height.max(1)).unwrap_or(255);
            let b = u8::try_from((x + y) % 255).unwrap_or(0);
            *pixel = Rgb([r, g, b]);
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut bytes);
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();
        bytes
    }

    fn write_jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut img: RgbImage = ImageBuffer::new(width, height);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let r = u8::try_from((x * 255) / width.max(1)).unwrap_or(255);
            let g = u8::try_from((y * 255) / height.max(1)).unwrap_or(255);
            let b = u8::try_from((x + y) % 255).unwrap_or(0);
            *pixel = Rgb([r, g, b]);
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut bytes);
        let encoder = JpegEncoder::new_with_quality(&mut cursor, 90);
        img.write_with_encoder(encoder).unwrap();
        bytes
    }

    #[test]
    fn crop_parse_accepts_four_numbers() {
        let region = CropRegion::parse("10,20,30,40").unwrap();
        assert_eq!(region.x, 10);
        assert_eq!(region.y, 20);
        assert_eq!(region.width, 30);
        assert_eq!(region.height, 40);
    }

    #[test]
    fn crop_parse_rejects_wrong_arity() {
        CropRegion::parse("1,2,3").unwrap_err();
        CropRegion::parse("1,2,3,4,5").unwrap_err();
    }

    #[test]
    fn crop_parse_rejects_negative() {
        CropRegion::parse("-1,0,10,10").unwrap_err();
    }

    #[test]
    fn output_format_parse_accepts_aliases() {
        assert_eq!(OutputFormat::parse("jpg").unwrap(), OutputFormat::Jpeg);
        assert_eq!(OutputFormat::parse("JPEG").unwrap(), OutputFormat::Jpeg);
        assert_eq!(OutputFormat::parse("png").unwrap(), OutputFormat::Png);
        OutputFormat::parse("bmp").unwrap_err();
    }

    #[test]
    fn sample_downscales_png() {
        let png = write_png(2048, 1024);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/pic.png"), &png)
            .unwrap();

        let result = sample_image(
            &system,
            Path::new("/project"),
            Path::new("pic.png"),
            false,
            &[],
            &SampleOptions {
                max_dimension: Some(512),
                ..SampleOptions::default()
            },
        )
        .unwrap();

        assert!(result.width <= 512);
        assert!(result.height <= 512);
        assert_eq!(result.source_width, 2048);
        assert_eq!(result.source_height, 1024);
    }

    #[test]
    fn sample_crops_then_scales() {
        let png = write_png(1000, 1000);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/pic.png"), &png)
            .unwrap();

        let result = sample_image(
            &system,
            Path::new("/project"),
            Path::new("pic.png"),
            false,
            &[],
            &SampleOptions {
                crop: Some(CropRegion {
                    x: 100,
                    y: 100,
                    width: 400,
                    height: 400,
                }),
                max_dimension: Some(200),
                ..SampleOptions::default()
            },
        )
        .unwrap();

        assert!(result.width <= 200);
        assert!(result.height <= 200);
    }

    #[test]
    fn sample_clamps_crop_to_bounds() {
        let png = write_png(100, 100);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/pic.png"), &png)
            .unwrap();

        let result = sample_image(
            &system,
            Path::new("/project"),
            Path::new("pic.png"),
            false,
            &[],
            &SampleOptions {
                crop: Some(CropRegion {
                    x: 50,
                    y: 50,
                    width: 500,
                    height: 500,
                }),
                ..SampleOptions::default()
            },
        )
        .unwrap();

        assert_eq!(result.width, 50);
        assert_eq!(result.height, 50);
    }

    #[test]
    fn sample_rejects_crop_outside_image() {
        let png = write_png(100, 100);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/pic.png"), &png)
            .unwrap();

        let err = sample_image(
            &system,
            Path::new("/project"),
            Path::new("pic.png"),
            false,
            &[],
            &SampleOptions {
                crop: Some(CropRegion {
                    x: 500,
                    y: 500,
                    width: 10,
                    height: 10,
                }),
                ..SampleOptions::default()
            },
        )
        .unwrap_err();
        assert!(format!("{err}").contains("outside the source image"));
    }

    #[test]
    fn sample_jpeg_respects_byte_budget() {
        let jpeg = write_jpeg(1500, 1500);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/photo.jpg"), &jpeg)
            .unwrap();

        let result = sample_image(
            &system,
            Path::new("/project"),
            Path::new("photo.jpg"),
            false,
            &[],
            &SampleOptions {
                max_bytes: Some(64 * 1024),
                ..SampleOptions::default()
            },
        )
        .unwrap();

        assert!(
            result.bytes.len() as u64 <= 64 * 1024,
            "encoded {} bytes > budget",
            result.bytes.len()
        );
        assert_eq!(result.format, "jpeg");
    }

    #[test]
    fn sample_rejects_max_bytes_below_floor() {
        let png = write_png(50, 50);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/pic.png"), &png)
            .unwrap();

        let err = sample_image(
            &system,
            Path::new("/project"),
            Path::new("pic.png"),
            false,
            &[],
            &SampleOptions {
                max_bytes: Some(MIN_MAX_BYTES - 1),
                ..SampleOptions::default()
            },
        )
        .unwrap_err();
        assert!(format!("{err}").contains("max_bytes"));
    }

    #[test]
    fn sample_rejects_non_image_mime() {
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/doc.pdf"), b"%PDF-1.4\n")
            .unwrap();

        let err = sample_image(
            &system,
            Path::new("/project"),
            Path::new("doc.pdf"),
            false,
            &[],
            &SampleOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("raster images"));
    }

    #[test]
    fn sample_rejects_svg() {
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/icon.svg"), b"<svg/>")
            .unwrap();

        let err = sample_image(
            &system,
            Path::new("/project"),
            Path::new("icon.svg"),
            false,
            &[],
            &SampleOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("raster images"));
    }

    #[test]
    fn sample_rejects_markdown_via_read_binary() {
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/notes.md"), b"# hi")
            .unwrap();

        sample_image(
            &system,
            Path::new("/project"),
            Path::new("notes.md"),
            false,
            &[],
            &SampleOptions::default(),
        )
        .unwrap_err();
    }

    #[test]
    fn sample_defaults_keep_small_image_intact() {
        let png = write_png(200, 100);
        let system = MockSystem::new()
            .with_current_dir("/project")
            .unwrap()
            .with_file(Path::new("/project/small.png"), &png)
            .unwrap();

        let result = sample_image(
            &system,
            Path::new("/project"),
            Path::new("small.png"),
            false,
            &[],
            &SampleOptions::default(),
        )
        .unwrap();
        assert_eq!(result.width, 200);
        assert_eq!(result.height, 100);
        assert!(result.bytes.len() as u64 <= DEFAULT_MAX_BYTES);
    }
}
