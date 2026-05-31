//! Tests for cosmic-text glyph rendering (v0.3).
//!
//! These tests build a fresh `TextRenderer` per test (cheap-ish but
//! amortized via `render_to_pixels_with`).

#![allow(clippy::float_cmp)]

use tauri_runtime_servocat::{Error, TextRenderer, Viewport, render, render_to_pixels_with};

fn fail(_msg: &'static str) -> Error {
    Error::Engine(boa_cat::Error::Unsupported { feature: "test" })
}

#[test]
fn text_paints_some_pixels() -> Result<(), Error> {
    let frame = render(
        "<html><body><p>hello world</p></body></html>",
        "p { color: black; height: 30px; }",
        Viewport::new(300, 50),
    )?;
    let mut renderer = TextRenderer::new();
    let pixels = render_to_pixels_with(&frame, 300, 50, &mut renderer);
    // Any non-zero alpha across the buffer means glyphs were
    // rasterized.  We don't assert exact pixel positions because font
    // selection varies across systems.
    pixels
        .rgba()
        .chunks(4)
        .any(|chunk| chunk.get(3).copied().unwrap_or(0) > 0)
        .then_some(())
        .ok_or_else(|| fail("expected at least one non-transparent pixel from text"))
}

#[test]
fn text_renderer_reusable_across_calls() -> Result<(), Error> {
    let mut renderer = TextRenderer::new();
    let frame_a = render(
        "<html><body><p>aaa</p></body></html>",
        "p { color: red; height: 30px; }",
        Viewport::new(100, 50),
    )?;
    let frame_b = render(
        "<html><body><p>bbb</p></body></html>",
        "p { color: blue; height: 30px; }",
        Viewport::new(100, 50),
    )?;
    let pixels_a = render_to_pixels_with(&frame_a, 100, 50, &mut renderer);
    let pixels_b = render_to_pixels_with(&frame_b, 100, 50, &mut renderer);
    (pixels_a.rgba() != pixels_b.rgba())
        .then_some(())
        .ok_or_else(|| fail("two distinct texts should produce distinct pixels"))
}

#[test]
fn empty_text_is_no_op() -> Result<(), Error> {
    let frame = render(
        "<html><body><p></p></body></html>",
        "p { color: black; height: 30px; }",
        Viewport::new(100, 50),
    )?;
    let mut renderer = TextRenderer::new();
    let pixels = render_to_pixels_with(&frame, 100, 50, &mut renderer);
    pixels
        .rgba()
        .iter()
        .all(|&b| b == 0)
        .then_some(())
        .ok_or_else(|| fail("empty text should leave buffer transparent"))
}
