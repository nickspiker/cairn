//! Cairn - Patch-based version control for successful cargo builds

pub mod blob;
pub mod daemon;
pub mod decode;
pub mod diff;
pub mod encode;
pub mod hash_encoding;
pub mod jump;
pub mod mnemonic;
pub mod patch;
pub mod patch_storage;
pub mod reconstruct;
pub mod snapshot;
pub mod snapshot_vsf;
pub mod state;
pub mod suffix_array;
pub mod tree;
// Testing byte-level diffs
