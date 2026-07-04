//! Cairn - Patch-based version control for successful cargo builds
//! Now with true nested field support in VSF!

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
pub mod state;
pub mod suffix_array;
pub mod tree;
pub mod vault;
// Testing byte-level diffs
