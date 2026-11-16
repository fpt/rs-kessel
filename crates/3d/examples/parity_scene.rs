//! A scene that exercises every path the engine has — textures through two
//! banks, Gouraud lighting, fog, near-plane and guard-band clipping, Z, and
//! all four blend equations — written out as the GPU's words.
//!
//! ```text
//! cargo run -p kessel-3d --example parity_scene -- /tmp/scene.words
//! ```
//!
//! prints the frame's `FRAME_CRC` as `kessel_3d::frame_crc` computes it. Feed
//! the same file to tang-verilog's `gpu_model.py` and the two numbers must
//! agree; `tests/parity.rs` pins the one they agreed on.

#[path = "../tests/scene/mod.rs"]
mod scene;

fn main() {
    let words = scene::words();
    if let Some(path) = std::env::args().nth(1) {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        std::fs::write(&path, bytes).expect("write words");
        eprintln!("{} words → {path}", words.len());
    }
    println!("{:08x}", scene::render_crc(&words));
}
