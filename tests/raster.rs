//! Tests for the tiny-skia rasterizer.

#![allow(clippy::float_cmp)]

use tauri_runtime_servocat::{Error, Viewport, render, render_to_pixels};

fn fail(_msg: &'static str) -> Error {
    Error::Engine(boa_cat::Error::Unsupported { feature: "test" })
}

#[test]
fn buffer_has_expected_dimensions() -> Result<(), Error> {
    let frame = render(
        "<html><body><div></div></body></html>",
        "div { background-color: red; height: 100px; }",
        Viewport::new(200, 100),
    )?;
    let pixels = render_to_pixels(&frame, 200, 100);
    (pixels.width() == 200 && pixels.height() == 100 && pixels.rgba().len() == 200 * 100 * 4)
        .then_some(())
        .ok_or_else(|| fail("buffer dimensions wrong"))
}

#[test]
fn empty_doc_produces_transparent_buffer() -> Result<(), Error> {
    let frame = render("", "", Viewport::new(10, 10))?;
    let pixels = render_to_pixels(&frame, 10, 10);
    pixels
        .rgba()
        .iter()
        .all(|&byte| byte == 0)
        .then_some(())
        .ok_or_else(|| fail("empty doc should be fully transparent"))
}

#[test]
fn red_background_paints_red_pixels() -> Result<(), Error> {
    let frame = render(
        "<html><body><div></div></body></html>",
        "div { background-color: red; height: 50px; }",
        Viewport::new(100, 100),
    )?;
    let pixels = render_to_pixels(&frame, 100, 100);
    // At (10, 10) we should be inside the red box.
    let r = pixels.pixel(10, 10, 0).ok_or_else(|| fail("pixel oob"))?;
    let g = pixels.pixel(10, 10, 1).ok_or_else(|| fail("pixel oob"))?;
    let b = pixels.pixel(10, 10, 2).ok_or_else(|| fail("pixel oob"))?;
    let a = pixels.pixel(10, 10, 3).ok_or_else(|| fail("pixel oob"))?;
    // tiny-skia uses premultiplied RGBA.  Red full-opacity premul: (255, 0, 0, 255).
    (r > 200 && g < 50 && b < 50 && a > 200)
        .then_some(())
        .ok_or_else(|| fail("expected red pixel"))
}

#[test]
fn outside_box_is_transparent() -> Result<(), Error> {
    let frame = render(
        "<html><body><div style='width: 40px; height: 40px; background-color: red;'></div></body></html>",
        "",
        Viewport::new(100, 100),
    )?;
    let pixels = render_to_pixels(&frame, 100, 100);
    // (80, 80) is outside both the body (which fills viewport but is
    // transparent) and the 40x40 red div.  Alpha should be 0.
    let a = pixels.pixel(80, 80, 3).ok_or_else(|| fail("pixel oob"))?;
    (a == 0)
        .then_some(())
        .ok_or_else(|| fail("expected transparent pixel outside boxes"))
}
