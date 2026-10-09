//! Read a raster image, optionally cropped, downscaled to fit a byte budget.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView as _, ImageFormat};
use os_shim::System;
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::ResolvedConfig;
use crate::document::{self, mime};

const DEFAULT_MAX_DIMENSION: u32 = 1024;

const DEFAULT_MAX_BYTES: u64 = 256 * 1024;

/// Below this no encoder produces a useful thumbnail, so the request is rejected upfront.
const MIN_MAX_BYTES: u64 = 1024;

const JPEG_QUALITY_INITIAL: u8 = 85;
const JPEG_QUALITY_STEP: u8 = 10;
const JPEG_QUALITY_FLOOR: u8 = 30;

/// The dimension cap halves on each retry, and the retry stops below this floor.
const DIMENSION_RETRY_FLOOR: u32 = 64;

/// Output encoding for the returned image.
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
pub struct GetImageOptions {
    pub crop: Option<CropRegion>,
    pub format: Option<OutputFormat>,
    pub max_bytes: Option<u64>,
    pub max_dimension: Option<u32>,
}

impl GetImageOptions {
    /// Build a `GetImageOptions` from the four CLI-flag shapes
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

    #[must_use]
    pub const fn with_crop(mut self, crop: CropRegion) -> Self {
        self.crop = Some(crop);
        self
    }

    #[must_use]
    pub const fn with_format(mut self, format: OutputFormat) -> Self {
        self.format = Some(format);
        self
    }

    #[must_use]
    pub const fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }

    #[must_use]
    pub const fn with_max_dimension(mut self, max_dimension: u32) -> Self {
        self.max_dimension = Some(max_dimension);
        self
    }
}

/// Output of [`get_image`]. `bytes` holds the re-encoded image; the
/// other fields describe what was actually produced so the caller can
/// surface it without re-decoding.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct GetImageResult {
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

impl GetImageResult {
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

/// Read (and optionally crop) an image attachment, downscaled to fit a byte budget.
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
pub fn get_image(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    config: &ResolvedConfig,
    options: &GetImageOptions,
) -> Result<GetImageResult> {
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    if max_bytes < MIN_MAX_BYTES {
        bail!(
            "max_bytes must be at least {MIN_MAX_BYTES} (got {max_bytes}); below this no useful \
             thumbnail can be produced"
        );
    }

    let payload = document::read_binary(system, base_dir, path, config)?;
    let source_mime = payload.mime;
    if !source_mime.starts_with("image/") || source_mime == "image/svg+xml" {
        bail!(
            "get_image only supports raster images (PNG, JPEG, GIF, WebP); got {source_mime}. \
             For other binaries use `get --binary`."
        );
    }

    let source_format = detect_source_format(source_mime)
        .with_context(|| format!("unsupported source format for get_image: {source_mime}"))?;

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

    Ok(GetImageResult {
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
                "cannot fit output within {max_bytes} bytes even at {DIMENSION_RETRY_FLOOR}px / \
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
        let encoder = JpegEncoder::new_with_quality(&mut buffer, quality);
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
    image
        .write_with_encoder(PngEncoder::new(&mut buffer))
        .context("encoding PNG")?;
    Ok(buffer)
}

#[cfg(test)]
mod tests;
