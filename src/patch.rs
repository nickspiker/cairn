//! Core patch data structures for cairn
//!
//! Cairn uses patch-based version control with VSF encoding.
//! Each patch represents a state of successful cargo builds.

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

/// File operations with binary diffs and blob storage
#[derive(Debug, Clone, PartialEq)]
pub enum FileOp {
    /// Modified file - stores diff from base blob
    ModifyFile {
        /// File path
        path: PathBuf,
        /// BLAKE3 hash of base content (provenance - identifies blob)
        base_blob: [u8; 32],
        /// Binary diff operations
        operations: Vec<ByteOp>,
        /// Expected BLAKE3 hash after applying operations (integrity check)
        result_hash: [u8; 32],
    },

    /// New file - stores full content as blob
    AddFile {
        /// File path
        path: PathBuf,
        /// BLAKE3 hash of content (provenance - identifies blob)
        content_blob: [u8; 32],
    },

    /// Deleted file
    DeleteFile {
        /// File path
        path: PathBuf,
        /// BLAKE3 hash of deleted content (provenance)
        old_blob: [u8; 32],
    },

    /// Renamed file - metadata only
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

    /// File operations to apply
    pub operations: Vec<FileOp>,

    /// BLAKE3 hash of cargo build output (integrity check)
    pub build_hash: [u8; 32],
}

impl Patch {
    /// Create a new patch
    pub fn new(
        author: AuthorId,
        parent: Option<PatchId>,
        timestamp: f64,
        message: String,
        operations: Vec<FileOp>,
        build_hash: [u8; 32],
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
        self.build_hash
    }
}

pub fn get_author_id() -> AuthorId {
    let name = std::env::var("CAIRN_AUTHOR_NAME").unwrap_or_else(|_| "cairn-default-author".to_string());
    let email = std::env::var("CAIRN_AUTHOR_EMAIL").unwrap_or_else(|_| "".to_string());

    let author_string = if email.is_empty() {
        name
    } else {
        format!("{} <{}>", name, email)
    };

    *blake3::hash(author_string.as_bytes()).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_creation() {
        let author = get_author_id();
        let timestamp = 1234567890.0;
        let build_hash = *blake3::hash(b"test build output").as_bytes();

        let content_blob = *blake3::hash(b"fn main() {\n    println!(\"Hello\");\n}").as_bytes();

        let patch = Patch::new(
            author,
            None, // Initial patch
            timestamp,
            "Initial patch".to_string(),
            vec![
                FileOp::AddFile {
                    path: PathBuf::from("src/main.rs"),
                    content_blob,
                },
            ],
            build_hash,
        );

        assert_eq!(patch.metadata.author, author);
        assert_eq!(patch.metadata.parent, None);
        assert_eq!(patch.metadata.message, "Initial patch");
        assert_eq!(patch.operations.len(), 1);
    }

    #[test]
    fn test_nick_spiker_author_id() {
        let author1 = get_author_id();
        let author2 = get_author_id();

        // Should be consistent
        assert_eq!(author1, author2);

        // Should be 32 bytes
        assert_eq!(author1.len(), 32);
    }
}
