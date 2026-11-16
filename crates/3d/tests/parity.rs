//! The software GPU against tang-verilog's reference model.
//!
//! `scene::words()` drives every path the engine has through the real
//! pipeline. The CRC below is the one tang-verilog's `gpu_model.py` computed
//! from the same words (`examples/parity_scene.rs` shows how to reproduce it),
//! which is the RTL's own `FRAME_CRC`. If this fails, either the engine now
//! sends different words — check the picture, then re-run the model — or the
//! rasterizer has stopped being a port of `raster.py`, which is never fine.

mod scene;

#[test]
fn the_parity_scene_matches_the_reference_model() {
    let words = scene::words();
    assert_eq!(scene::render_crc(&words), 0x4326_a16f);
}

#[test]
fn the_words_do_not_depend_on_anything_but_their_inputs() {
    assert_eq!(scene::words(), scene::words());
}
