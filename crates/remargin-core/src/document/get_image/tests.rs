//! Tests for image sampling: option parsing, crop and downscale, the byte budget and the
//! inputs it rejects.

use super::{
    CropRegion, DEFAULT_MAX_BYTES, GetImageOptions, MIN_MAX_BYTES, OutputFormat, get_image,
};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ImageBuffer, Rgb, RgbImage};
use os_shim::mock::MemorySystem;
use std::path::Path;

use crate::config::{Mode, ResolvedConfig};
use crate::parser::AuthorType;

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn write_png(width: u32, height: u32) -> Vec<u8> {
    let mut img: RgbImage = ImageBuffer::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = u8::try_from((x * 255) / width.max(1)).unwrap_or(255);
        let g = u8::try_from((y * 255) / height.max(1)).unwrap_or(255);
        let b = u8::try_from((x + y) % 255).unwrap_or(0);
        *pixel = Rgb([r, g, b]);
    }
    let mut bytes: Vec<u8> = Vec::new();
    img.write_with_encoder(PngEncoder::new(&mut bytes)).unwrap();
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
    let encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
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
fn downscales_png() {
    let png = write_png(2048, 1024);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &png)
        .unwrap();

    let result = get_image(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
        &GetImageOptions {
            max_dimension: Some(512),
            ..GetImageOptions::default()
        },
    )
    .unwrap();

    assert!(result.width <= 512);
    assert!(result.height <= 512);
    assert_eq!(result.source_width, 2048);
    assert_eq!(result.source_height, 1024);
}

#[test]
fn crops_then_scales() {
    let png = write_png(1000, 1000);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &png)
        .unwrap();

    let result = get_image(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
        &GetImageOptions {
            crop: Some(CropRegion {
                x: 100,
                y: 100,
                width: 400,
                height: 400,
            }),
            max_dimension: Some(200),
            ..GetImageOptions::default()
        },
    )
    .unwrap();

    assert!(result.width <= 200);
    assert!(result.height <= 200);
}

#[test]
fn clamps_crop_to_bounds() {
    let png = write_png(100, 100);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &png)
        .unwrap();

    let result = get_image(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
        &GetImageOptions {
            crop: Some(CropRegion {
                x: 50,
                y: 50,
                width: 500,
                height: 500,
            }),
            ..GetImageOptions::default()
        },
    )
    .unwrap();

    assert_eq!(result.width, 50);
    assert_eq!(result.height, 50);
}

#[test]
fn rejects_crop_outside_image() {
    let png = write_png(100, 100);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &png)
        .unwrap();

    let err = get_image(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
        &GetImageOptions {
            crop: Some(CropRegion {
                x: 500,
                y: 500,
                width: 10,
                height: 10,
            }),
            ..GetImageOptions::default()
        },
    )
    .unwrap_err();
    assert!(format!("{err}").contains("outside the source image"));
}

#[test]
fn jpeg_respects_byte_budget() {
    let jpeg = write_jpeg(1500, 1500);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/photo.jpg"), &jpeg)
        .unwrap();

    let result = get_image(
        &system,
        Path::new("/project"),
        Path::new("photo.jpg"),
        &open_config(),
        &GetImageOptions {
            max_bytes: Some(64 * 1024),
            ..GetImageOptions::default()
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
fn rejects_max_bytes_below_floor() {
    let png = write_png(50, 50);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/pic.png"), &png)
        .unwrap();

    let err = get_image(
        &system,
        Path::new("/project"),
        Path::new("pic.png"),
        &open_config(),
        &GetImageOptions {
            max_bytes: Some(MIN_MAX_BYTES - 1),
            ..GetImageOptions::default()
        },
    )
    .unwrap_err();
    assert!(format!("{err}").contains("max_bytes"));
}

#[test]
fn rejects_non_image_mime() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/doc.pdf"), b"%PDF-1.4\n")
        .unwrap();

    let err = get_image(
        &system,
        Path::new("/project"),
        Path::new("doc.pdf"),
        &open_config(),
        &GetImageOptions::default(),
    )
    .unwrap_err();
    assert!(format!("{err}").contains("raster images"));
}

#[test]
fn rejects_svg() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/icon.svg"), b"<svg/>")
        .unwrap();

    let err = get_image(
        &system,
        Path::new("/project"),
        Path::new("icon.svg"),
        &open_config(),
        &GetImageOptions::default(),
    )
    .unwrap_err();
    assert!(format!("{err}").contains("raster images"));
}

#[test]
fn rejects_markdown_via_read_binary() {
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/notes.md"), b"# hi")
        .unwrap();

    get_image(
        &system,
        Path::new("/project"),
        Path::new("notes.md"),
        &open_config(),
        &GetImageOptions::default(),
    )
    .unwrap_err();
}

#[test]
fn defaults_keep_small_image_intact() {
    let png = write_png(200, 100);
    let system = MemorySystem::new()
        .with_current_dir("/project")
        .unwrap()
        .with_file(Path::new("/project/small.png"), &png)
        .unwrap();

    let result = get_image(
        &system,
        Path::new("/project"),
        Path::new("small.png"),
        &open_config(),
        &GetImageOptions::default(),
    )
    .unwrap();
    assert_eq!(result.width, 200);
    assert_eq!(result.height, 100);
    assert!(result.bytes.len() as u64 <= DEFAULT_MAX_BYTES);
}
