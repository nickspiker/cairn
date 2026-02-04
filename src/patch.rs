//! Core patch data structures for cairn
//!
//! Cairn uses patch-based version control with VSF encoding.
//! Each patch represents a state of successful cargo builds.

/// Binary diff operations on file content
///
/// Operations reference absolute byte positions in the immutable base blob.
/// All Copy operations read from the base blob, so positions never shift.
#[derive(Debug, Clone, PartialEq)]
pub enum ByteOp {
    /// Copy bytes from base blob
    Copy {
        /// Absolute byte offset in base blob
        start: usize,
        /// Number of bytes to copy
        len: usize,
    },

    /// Insert new bytes at current output position
    Insert {
        /// Content to insert
        content: Vec<u8>,
    },
}
