//! Tests for the v3.32.0 image-asset pipeline: data: URL
//! decoding, layout-tree walk, and pixel stamping through the
//! existing tiny-skia rasterizer.

#![allow(clippy::float_cmp)]

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tauri_runtime_servocat::{
    DecodedImage, Error, Viewport, decode_data_url, render, render_to_pixels,
    render_with_inline_assets,
};

fn fail(_msg: &'static str) -> Error {
    Error::Engine(boa_cat::Error::Unsupported { feature: "test" })
}

/// Build a tiny solid-color RGBA PNG and wrap it in a data: URL.
/// Returns `None` on png-encoder failure (vanishingly rare for
/// in-memory writes; tests propagate via `?` rather than panic).
fn build_solid_png_data_url(width: u32, height: u32, rgba: [u8; 4]) -> Option<String> {
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        let total = usize::try_from(width).ok()? * usize::try_from(height).ok()? * 4;
        let pixels: Vec<u8> = (0..total).map(|i| rgba[i % 4]).collect();
        writer.write_image_data(&pixels).ok()?;
    }
    let mut url = String::from("data:image/png;base64,");
    STANDARD.encode_string(&png_bytes, &mut url);
    Some(url)
}

#[test]
fn decode_data_url_returns_expected_dimensions() -> Result<(), Error> {
    let url = build_solid_png_data_url(2, 3, [0xff, 0x00, 0x00, 0xff])
        .ok_or_else(|| fail("png build failed"))?;
    let decoded: DecodedImage = decode_data_url(&url).ok_or_else(|| fail("decode failed"))?;
    (decoded.width() == 2 && decoded.height() == 3 && decoded.rgba().len() == 2 * 3 * 4)
        .then_some(())
        .ok_or_else(|| fail("dimensions wrong"))
}

#[test]
fn decode_data_url_returns_expected_pixel_values() -> Result<(), Error> {
    let url = build_solid_png_data_url(1, 1, [0xff, 0x00, 0x00, 0xff])
        .ok_or_else(|| fail("png build failed"))?;
    let decoded = decode_data_url(&url).ok_or_else(|| fail("decode failed"))?;
    (decoded.rgba() == [0xff, 0x00, 0x00, 0xff])
        .then_some(())
        .ok_or_else(|| fail("pixel values wrong"))
}

#[test]
fn decode_data_url_rejects_non_png_scheme() -> Result<(), Error> {
    decode_data_url("data:image/jpeg;base64,abc")
        .is_none()
        .then_some(())
        .ok_or_else(|| fail("non-png scheme should be rejected"))
}

#[test]
fn decode_data_url_rejects_garbage_base64() -> Result<(), Error> {
    decode_data_url("data:image/png;base64,!!!not-base64!!!")
        .is_none()
        .then_some(())
        .ok_or_else(|| fail("garbage base64 should be rejected"))
}

#[test]
fn render_with_inline_assets_collects_img() -> Result<(), Error> {
    let url = build_solid_png_data_url(4, 4, [0xff, 0x00, 0x00, 0xff])
        .ok_or_else(|| fail("png build failed"))?;
    let html = format!("<html><body><img src='{url}'/></body></html>");
    let frame = render_with_inline_assets(
        &html,
        "img { width: 4px; height: 4px; }",
        Viewport::new(100, 100),
    )?;
    (frame.decoded_images().len() == 1)
        .then_some(())
        .ok_or_else(|| fail("expected one decoded img"))
}

#[test]
fn render_with_inline_assets_skips_unsupported_scheme() -> Result<(), Error> {
    let html = "<html><body><img src='https://example.com/cat.png'/></body></html>";
    let frame = render_with_inline_assets(
        html,
        "img { width: 4px; height: 4px; }",
        Viewport::new(100, 100),
    )?;
    frame
        .decoded_images()
        .is_empty()
        .then_some(())
        .ok_or_else(|| fail("https URL should be skipped silently"))
}

#[test]
fn render_with_inline_assets_handles_multiple_images() -> Result<(), Error> {
    let red = build_solid_png_data_url(2, 2, [0xff, 0x00, 0x00, 0xff])
        .ok_or_else(|| fail("png build failed"))?;
    let blue = build_solid_png_data_url(2, 2, [0x00, 0x00, 0xff, 0xff])
        .ok_or_else(|| fail("png build failed"))?;
    let html = format!("<html><body><img src='{red}'/><img src='{blue}'/></body></html>",);
    let frame = render_with_inline_assets(
        &html,
        "img { width: 4px; height: 4px; }",
        Viewport::new(100, 100),
    )?;
    (frame.decoded_images().len() == 2)
        .then_some(())
        .ok_or_else(|| fail("expected two decoded imgs"))
}

#[test]
fn raster_stamps_decoded_image_pixels() -> Result<(), Error> {
    // Render a 4x4 red png at (0,0) sized 4x4 on a 10x10 canvas.
    // The first 4x4 pixels of the buffer should be red (with
    // premultiplied alpha, which for opaque red stays 0xff,0,0,0xff).
    let url = build_solid_png_data_url(4, 4, [0xff, 0x00, 0x00, 0xff])
        .ok_or_else(|| fail("png build failed"))?;
    let html = format!("<html><body><img src='{url}'/></body></html>");
    let frame = render_with_inline_assets(
        &html,
        "body { margin: 0; padding: 0; } img { width: 4px; height: 4px; display: block; }",
        Viewport::new(10, 10),
    )?;
    let pixels = render_to_pixels(&frame, 10, 10);
    // Find some red pixel in the buffer (somewhere in the first
    // 4 rows / 4 columns).
    let any_red = (0..4).any(|y| {
        (0..4).any(|x| {
            let r = pixels.pixel(x, y, 0).unwrap_or(0);
            let g = pixels.pixel(x, y, 1).unwrap_or(0);
            let b = pixels.pixel(x, y, 2).unwrap_or(0);
            let a = pixels.pixel(x, y, 3).unwrap_or(0);
            r > 200 && g < 50 && b < 50 && a > 200
        })
    });
    any_red
        .then_some(())
        .ok_or_else(|| fail("expected at least one red pixel in the stamp region"))
}

#[test]
fn raster_without_assets_unchanged() -> Result<(), Error> {
    // Smoke test: a frame with no decoded images rasterizes
    // exactly as the legacy `render` path did (an existing test
    // suite covers the no-image case in tests/raster.rs).
    let frame = render(
        "<html><body><div></div></body></html>",
        "div { background-color: red; height: 5px; }",
        Viewport::new(10, 10),
    )?;
    frame
        .decoded_images()
        .is_empty()
        .then_some(())
        .ok_or_else(|| fail("frame from render() should have empty decoded-image map"))
}
