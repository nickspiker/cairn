//! VSF encoding for cairn patches
//!
//! Encodes patches using VSF's hierarchical section format:
//! - metadata section: author, parent, message
//! - operations section: file operations (insert/delete/modify/add/delete/rename)
//! - build_output section: cargo build hash

use crate::patch::{LineOp, Patch};
use anyhow::Result;
use vsf::types::Vector;
use vsf::{VsfBuilder, VsfSection, VsfType};

impl Patch {
    /// Encode patch to VSF format
    ///
    /// Returns the complete VSF file as bytes
    pub fn encode_vsf(&self) -> Result<Vec<u8>> {
        // 1. Create metadata section
        let metadata_section = encode_metadata_section(&self.metadata)?;

        // 2. Create operations section
        let operations_section = encode_operations_section(&self.operations)?;

        // 3. Create build output section
        let build_section = encode_build_section(&self.build_hash)?;

        // 4. Build complete VSF file
        let builder = VsfBuilder::new()
            .add_section_direct(metadata_section)
            .add_section_direct(operations_section)
            .add_section_direct(build_section);

        builder.build().map_err(|e| anyhow::anyhow!(e))
    }
}

/// Encode metadata section
fn encode_metadata_section(metadata: &crate::patch::PatchMetadata) -> Result<VsfSection> {
    let mut section = VsfSection::new("metadata");

    // Author as BLAKE3 hash
    section.add_field("author", VsfType::hp(metadata.author.to_vec()));

    // Parent patch hash (optional)
    if let Some(parent) = metadata.parent {
        section.add_field("parent", VsfType::hp(parent.to_vec()));
    }

    // Commit message (Huffman compressed)
    section.add_field("message", VsfType::x(metadata.message.clone()));

    Ok(section)
}

/// Encode operations section
fn encode_operations_section(operations: &[LineOp]) -> Result<VsfSection> {
    let mut section = VsfSection::new("operations");

    for op in operations {
        match op {
            LineOp::InsertLine { file, after, content } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(0),                                      // Op type: 0=insert
                        VsfType::l(file.to_string_lossy().to_string()),      // File path
                        VsfType::u(*after, false),                           // Line number (auto-sized)
                        VsfType::v_u3(Vector { data: content.clone() }),     // Content bytes
                    ],
                );
            }
            LineOp::DeleteLine { file, at, old_content } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(1),                                      // Op type: 1=delete
                        VsfType::l(file.to_string_lossy().to_string()),      // File path
                        VsfType::u(*at, false),                              // Line number
                        VsfType::v_u3(Vector { data: old_content.clone() }), // Old content (for undo)
                    ],
                );
            }
            LineOp::ModifyLine { file, at, old, new } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(2),                                      // Op type: 2=modify
                        VsfType::l(file.to_string_lossy().to_string()),      // File path
                        VsfType::u(*at, false),                              // Line number
                        VsfType::v_u3(Vector { data: old.clone() }),         // Old content
                        VsfType::v_u3(Vector { data: new.clone() }),         // New content
                    ],
                );
            }
            LineOp::AddFile { path, content } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(3),                                      // Op type: 3=add file
                        VsfType::l(path.to_string_lossy().to_string()),      // File path
                        VsfType::v_u3(Vector { data: content.clone() }),     // File content
                    ],
                );
            }
            LineOp::DeleteFile { path, old_content } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(4),                                      // Op type: 4=delete file
                        VsfType::l(path.to_string_lossy().to_string()),      // File path
                        VsfType::v_u3(Vector { data: old_content.clone() }), // Old content (for undo)
                    ],
                );
            }
            LineOp::RenameFile { from, to } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(5),                                      // Op type: 5=rename file
                        VsfType::l(from.to_string_lossy().to_string()),      // Source path
                        VsfType::l(to.to_string_lossy().to_string()),        // Dest path
                    ],
                );
            }
        }
    }

    Ok(section)
}

/// Encode build output section
fn encode_build_section(build_hash: &blake3::Hash) -> Result<VsfSection> {
    let mut section = VsfSection::new("build_output");

    // Store BLAKE3 hash of cargo build output
    section.add_field("cargo_hash", VsfType::hp(build_hash.as_bytes().to_vec()));

    Ok(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::{get_author_id, PatchMetadata};
    use std::path::PathBuf;

    #[test]
    fn test_encode_metadata() {
        let metadata = PatchMetadata {
            author: get_author_id(),
            parent: None,
            timestamp: 1234567890.0,
            message: "Test patch".to_string(),
        };

        let section = encode_metadata_section(&metadata).unwrap();

        assert_eq!(section.name, "metadata");
        assert!(section.get_field("author").is_some());
        assert!(section.get_field("message").is_some());
    }

    #[test]
    fn test_encode_operations() {
        let ops = vec![
            LineOp::InsertLine {
                file: PathBuf::from("src/main.rs"),
                after: 0,
                content: b"fn main() {".to_vec(),
            },
            LineOp::DeleteLine {
                file: PathBuf::from("src/lib.rs"),
                at: 5,
                old_content: b"old line".to_vec(),
            },
            LineOp::ModifyLine {
                file: PathBuf::from("src/main.rs"),
                at: 10,
                old: b"old".to_vec(),
                new: b"new".to_vec(),
            },
        ];

        let section = encode_operations_section(&ops).unwrap();

        assert_eq!(section.name, "operations");
        let op_fields = section.get_fields("op");
        assert_eq!(op_fields.len(), 3);
    }

    #[test]
    fn test_encode_build_section() {
        let hash = blake3::hash(b"cargo build output");
        let section = encode_build_section(&hash).unwrap();

        assert_eq!(section.name, "build_output");
        assert!(section.get_field("cargo_hash").is_some());
    }

    #[test]
    fn test_full_patch_encode() {
        let author = get_author_id();
        let build_hash = blake3::hash(b"test build");

        let patch = Patch::new(
            author,
            None,
            1234567890.0,
            "Test patch".to_string(),
            vec![LineOp::InsertLine {
                file: PathBuf::from("src/main.rs"),
                after: 0,
                content: b"test".to_vec(),
            }],
            build_hash,
        );

        let encoded = patch.encode_vsf().unwrap();

        // Should start with VSF magic number "RÅ<"
        assert_eq!(&encoded[0..3], "RÅ".as_bytes());
        assert_eq!(encoded[3], b'<');
    }

    #[test]
    #[cfg(feature = "inspect")]
    fn test_inspect_patch_vsf() {
        use vsf::inspect::inspect_vsf;

        let author = get_author_id();
        let build_hash = blake3::hash(b"test build");

        let patch = Patch::new(
            author,
            None,
            1234567890.0,
            "Test patch".to_string(),
            vec![LineOp::InsertLine {
                file: PathBuf::from("src/main.rs"),
                after: 0,
                content: b"fn main() {".to_vec(),
            }],
            build_hash,
        );

        let encoded = patch.encode_vsf().unwrap();

        println!("\n=== VSF Inspector Output ===");
        match inspect_vsf(&encoded) {
            Ok(output) => println!("{}", output),
            Err(e) => println!("Inspector error: {}", e),
        }
        println!("=== End Inspector Output ===\n");
    }
}
