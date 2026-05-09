//! Windy — a 2D esoteric programming language.
//!
//! The v0.2 reference implementation. Same crate backs the native CLI
//! today and (in v0.3) the browser playground via `wasm32` target.

pub mod debugger;
pub mod easter;
pub mod grid;
pub mod opcodes;
pub mod parser;
pub mod vm;

// wasm_api is the wasm-bindgen surface for the browser playground. The
// WASI target (`wasm32-wasip1`) re-uses the same crate as a normal CLI
// binary and must NOT pull wasm_api in.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub mod wasm_api;

pub use debugger::debug_source;
pub use easter::{banner, detect, SIGNATURE};
pub use grid::{Grid, Ip, SPACE};
pub use opcodes::{decode_cell, Op};
pub use parser::{normalize, parse};
pub use vm::{run_source, ExitCode, RunOptions, Vm};

#[cfg(feature = "metrics")]
pub use vm::VmMetrics;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
