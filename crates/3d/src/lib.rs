//! Kessel3D: a PS1-shaped 3D pipeline for the kessel console, in integers.
//!
//! Two halves, split where the hardware splits them:
//!
//! - [`gte`] is the **host** half — the CPU and GTE of a PS1. It transforms,
//!   lights, fogs, clips and projects meshes, and writes the frame as command
//!   words for tang-verilog's 3D GPU (`docs/SPECIFICATION_3D.md` there). On a
//!   board it runs on the RP2350 beside the VM and the words go out over HSTX;
//!   only screen-space triangles cross the link, which is what keeps a scene at
//!   60 fps.
//! - [`raster`] is the **GPU** half, in software: it executes those words and
//!   produces the picture, pixel for pixel what the FPGA scans out. It is a
//!   port of tang-verilog's reference model and has to stay one.
//!
//! The words are the whole interface between them. The PC draws from exactly
//! the words the board would be sent, so a picture that is right on one is
//! right on the other — and [`raster::frame_crc`] is the board's own
//! `FRAME_CRC`, so the claim is checkable against hardware.
//!
//! Everything is fixed point ([`fixed`]): a float pipeline would round
//! differently on the PC and the RP2350, and a scene that drifts by a pixel
//! between the agent's machine and the board is the plausibly-wrong picture
//! this project exists to catch.
//!
//! `no_std` + `alloc`, and dependency-free, for the same reason `kessel-audio`
//! is: the firmware has to link it.

#![no_std]

extern crate alloc;

pub mod cmd;
pub mod fixed;
pub mod gte;
pub mod mesh;
pub mod raster;

pub use gte::{clut_from_palette, Gte, GteStats, TexCache, TexKey};
pub use raster::{frame_crc, GpuStats, SoftGpu};
