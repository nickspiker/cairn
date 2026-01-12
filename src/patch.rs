//! Core patch data structures for cairn
//!
//! Cairn uses patch-based version control with VSF encoding.
//! Each patch represents a state of successful cargo builds.

use blake3::Hash;
use std::path::PathBuf;

/// Patch ID is a BLAKE3 hash
pub type PatchId = [u8; 32];

/// Author ID is a BLAKE3 hash of identity
pub type AuthorId = [u8; 32];

/// Patch metadata
#[derive(Debug, Clone, PartialEq)]
pub struct PatchMetadata {
    /// Author identity (BLAKE3 hash of identity)
    pub author: AuthorId,

    /// Parent patch ID (None for initial patch)
    pub parent: Option<PatchId>,

    /// Creation timestamp (Eagle Time - seconds since 1969-07-20 20:17:40 UTC)
    pub timestamp: f64,

    /// Patch message
    pub message: String,
}

/// File and line-based edit operations
#[derive(Debug, Clone, PartialEq)]
pub enum LineOp {
    /// Insert a line after the specified position
    InsertLine {
        /// File path
        file: PathBuf,
        /// Line number to insert after (0 = before first line)
        after: usize,
        /// Content to insert (bytes, no trailing newline)
        content: Vec<u8>,
    },

    /// Delete a line at the specified position
    DeleteLine {
        /// File path
        file: PathBuf,
        /// Line number to delete
        at: usize,
        /// Old content (for undo/verification)
        old_content: Vec<u8>,
    },

    /// Modify a line at the specified position
    ModifyLine {
        /// File path
        file: PathBuf,
        /// Line number to modify
        at: usize,
        /// Old content
        old: Vec<u8>,
        /// New content
        new: Vec<u8>,
    },

    /// Add a new file
    AddFile {
        /// File path
        path: PathBuf,
        /// File content
        content: Vec<u8>,
    },

    /// Delete a file
    DeleteFile {
        /// File path
        path: PathBuf,
        /// Old content (for undo)
        old_content: Vec<u8>,
    },

    /// Rename a file
    RenameFile {
        /// Source path
        from: PathBuf,
        /// Destination path
        to: PathBuf,
    },
}

/// A complete patch representing a state transition
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    /// Patch metadata
    pub metadata: PatchMetadata,

    /// Edit operations to apply
    pub operations: Vec<LineOp>,

    /// BLAKE3 hash of cargo build output (for verification)
    pub build_hash: Hash,
}

impl Patch {
    /// Create a new patch
    pub fn new(
        author: AuthorId,
        parent: Option<PatchId>,
        timestamp: f64,
        message: String,
        operations: Vec<LineOp>,
        build_hash: Hash,
    ) -> Self {
        Self {
            metadata: PatchMetadata {
                author,
                parent,
                timestamp,
                message,
            },
            operations,
            build_hash,
        }
    }

    /// Compute the patch ID (BLAKE3 hash of encoded patch)
    pub fn id(&self) -> PatchId {
        // Will be implemented in encode.rs
        // For now, return a placeholder
        *self.build_hash.as_bytes()
    }
}

/// Get Nick Spiker's author ID
pub fn nick_spiker_author_id() -> AuthorId {
    *blake3::hash(b"Nick Spiker <nick@spiker.dev>").as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_creation() {
        let author = nick_spiker_author_id();
        let timestamp = 1234567890.0;
        let build_hash = blake3::hash(b"test build output");

        let patch = Patch::new(
            author,
            None, // Initial patch
            timestamp,
            "Initial patch".to_string(),
            vec![
                LineOp::InsertLine {
                    file: PathBuf::from("src/main.rs"),
                    after: 0,
                    content: b"fn main() {".to_vec(),
                },
                LineOp::InsertLine {
                    file: PathBuf::from("src/main.rs"),
                    after: 1,
                    content: b"    println!(\"Hello\");".to_vec(),
                },
                LineOp::InsertLine {
                    file: PathBuf::from("src/main.rs"),
                    after: 2,
                    content: b"}".to_vec(),
                },
            ],
            build_hash,
        );

        assert_eq!(patch.metadata.author, author);
        assert_eq!(patch.metadata.parent, None);
        assert_eq!(patch.metadata.message, "Initial patch");
        assert_eq!(patch.operations.len(), 3);
    }

    #[test]
    fn test_nick_spiker_author_id() {
        let author1 = nick_spiker_author_id();
        let author2 = nick_spiker_author_id();

        // Should be consistent
        assert_eq!(author1, author2);

        // Should be 32 bytes
        assert_eq!(author1.len(), 32);
    }
}
