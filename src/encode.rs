//! VSF encoding for cairn patches with binary diffs on x-encoded snapshots
//!
//! Encodes patches using VSF's hierarchical section format:
//! - metadata section: author, parent, timestamp, message
//! - operations section: FileOp with ByteOp encoded as byte vectors
//! - build_output section: cargo build hash
//!
//! Hash types used:
//! - hp (hash provenance): Content identification for snapshots and results (immutable, identifies specific content)

use crate::patch::{ByteOp, FileOp, Patch};
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

    // Author as BLAKE3 hash (provenance)
    section.add_field("author", VsfType::hp(metadata.author.to_vec()));

    // Parent patch hash (optional, provenance)
    if let Some(parent) = metadata.parent {
        section.add_field("parent", VsfType::hp(parent.to_vec()));
    }

    // Timestamp (Eagle Time - oscillation count since 1969-07-20 20:17:40 UTC)
    section.add_field("timestamp", VsfType::e(vsf::EtType::u(metadata.timestamp as u64)));

    // Commit message (Huffman compressed)
    section.add_field("message", VsfType::x(metadata.message.clone()));

    Ok(section)
}

/// Encode operations section with FileOp and ByteOp
fn encode_operations_section(operations: &[FileOp]) -> Result<VsfSection> {
    let mut section = VsfSection::new("operations");

    for op in operations {
        match op {
            FileOp::ModifyFile {
                path,
                base_snapshot,
                operations: byte_ops,
                result_hash,
            } => {
                // Encode ByteOp operations as a byte vector
                let byte_ops_encoded = encode_byte_ops(byte_ops);

                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(0),                                 // Op type: 0=modify file
                        VsfType::l(path.to_string_lossy().to_string()), // File path
                        VsfType::hp(base_snapshot.to_vec()), // Base snapshot hash (provenance)
                        VsfType::hp(result_hash.to_vec()), // Result hash (provenance of x-encoded result)
                        VsfType::v_u3(Vector {
                            data: byte_ops_encoded,
                        }), // ByteOp sequence (on x-encoded content)
                    ],
                );
            }

            FileOp::AddFile { path } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(1),                                 // Op type: 1=add file
                        VsfType::l(path.to_string_lossy().to_string()), // File path (content in new snapshot)
                    ],
                );
            }

            FileOp::DeleteFile { path, old_snapshot } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(2),                                 // Op type: 2=delete file
                        VsfType::l(path.to_string_lossy().to_string()), // File path
                        VsfType::hp(old_snapshot.to_vec()), // Old snapshot hash (provenance)
                    ],
                );
            }

            FileOp::RenameFile { from, to } => {
                section.add_field_multi(
                    "op",
                    vec![
                        VsfType::u3(3),                                 // Op type: 3=rename file
                        VsfType::l(from.to_string_lossy().to_string()), // Source path
                        VsfType::l(to.to_string_lossy().to_string()),   // Dest path
                    ],
                );
            }
        }
    }

    Ok(section)
}

/// Encode ByteOp operations as a byte vector
///
/// Format for each operation:
/// - Copy: [0u8, start_bytes..., len_bytes...]
/// - Insert: [1u8, len_bytes..., content_bytes...]
///
/// Uses variable-length encoding for sizes (LEB128-style).
pub(crate) fn encode_byte_ops(byte_ops: &[ByteOp]) -> Vec<u8> {
    let mut encoded = Vec::new();

    for op in byte_ops {
        match op {
            ByteOp::Copy { start, len } => {
                encoded.push(0); // Copy operation tag
                encode_varint(&mut encoded, *start);
                encode_varint(&mut encoded, *len);
            }
            ByteOp::Insert { content } => {
                encoded.push(1); // Insert operation tag
                encode_varint(&mut encoded, content.len());
                encoded.extend_from_slice(content);
            }
        }
    }

    encoded
}

/// Encode usize as variable-length integer (LEB128)
pub(crate) fn encode_varint(buf: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80; // More bytes coming
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Encode build output section
fn encode_build_section(build_hash: &[u8; 32]) -> Result<VsfSection> {
    let mut section = VsfSection::new("build_output");

    // Store BLAKE3 hash of cargo build output (integrity verification)
    section.add_field("cargo_hash", VsfType::hb(build_hash.to_vec()));

    Ok(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::{PatchMetadata, get_author_id};
    use std::path::PathBuf;

    #[test]
    fn test_encode_metadata() {
        let metadata = PatchMetadata {
            author: get_author_id(),
            parent: None,
            timestamp: 1234567890,
            message: "Test patch".to_string(),
        };

        let section = encode_metadata_section(&metadata).unwrap();

        assert_eq!(section.name, "metadata");
        assert!(section.get_field("author").is_some());
        assert!(section.get_field("timestamp").is_some());
        assert!(section.get_field("message").is_some());
    }

    #[test]
    fn test_encode_byte_ops() {
        let byte_ops = vec![
            ByteOp::Copy { start: 0, len: 5 },
            ByteOp::Insert {
                content: b"hello".to_vec(),
            },
            ByteOp::Copy { start: 10, len: 20 },
        ];

        let encoded = encode_byte_ops(&byte_ops);

        // Should not be empty
        assert!(!encoded.is_empty());

        // First byte should be 0 (Copy operation)
        assert_eq!(encoded[0], 0);
    }

    #[test]
    fn test_encode_operations() {
        let base_snapshot = *blake3::hash(b"base snapshot").as_bytes();
        let result_hash = *blake3::hash(b"result x-encoded content").as_bytes();
        let old_snapshot = *blake3::hash(b"old snapshot").as_bytes();

        let ops = vec![
            FileOp::AddFile {
                path: PathBuf::from("src/main.rs"),
            },
            FileOp::ModifyFile {
                path: PathBuf::from("src/lib.rs"),
                base_snapshot,
                operations: vec![ByteOp::Copy { start: 0, len: 10 }],
                result_hash,
            },
            FileOp::DeleteFile {
                path: PathBuf::from("old.rs"),
                old_snapshot,
            },
            FileOp::RenameFile {
                from: PathBuf::from("old.txt"),
                to: PathBuf::from("new.txt"),
            },
        ];

        let section = encode_operations_section(&ops).unwrap();

        assert_eq!(section.name, "operations");
        let op_fields = section.get_fields("op");
        assert_eq!(op_fields.len(), 4);
    }

    #[test]
    fn test_encode_build_section() {
        let hash = *blake3::hash(b"cargo build output").as_bytes();
        let section = encode_build_section(&hash).unwrap();

        assert_eq!(section.name, "build_output");
        assert!(section.get_field("cargo_hash").is_some());
    }

    #[test]
    fn test_full_patch_encode() {
        let author = get_author_id();
        let build_hash = *blake3::hash(b"test build").as_bytes();

        let patch = Patch::new(
            author,
            None,
            1234567890,
            "Test patch".to_string(),
            vec![FileOp::AddFile {
                path: PathBuf::from("src/main.rs"),
            }],
            build_hash,
        );

        let encoded = patch.encode_vsf().unwrap();

        // Should start with VSF magic number "RÅ<"
        assert_eq!(&encoded[0..3], "RÅ".as_bytes());
        assert_eq!(encoded[3], b'<');
    }

    #[test]
    fn test_varint_encoding() {
        let mut buf = Vec::new();

        // Test small value
        encode_varint(&mut buf, 127);
        assert_eq!(buf, vec![127]);

        // Test larger value
        buf.clear();
        encode_varint(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        // Test even larger value
        buf.clear();
        encode_varint(&mut buf, 16384);
        assert_eq!(buf, vec![0x80, 0x80, 0x01]);
    }

    #[test]
    fn test_byte_ops_roundtrip() {
        let original_ops = vec![
            ByteOp::Copy { start: 0, len: 100 },
            ByteOp::Insert {
                content: b"inserted text".to_vec(),
            },
            ByteOp::Copy {
                start: 200,
                len: 50,
            },
        ];

        let encoded = encode_byte_ops(&original_ops);

        // Should produce non-empty encoding
        assert!(!encoded.is_empty());

        // Should start with Copy operation tag (0)
        assert_eq!(encoded[0], 0);
    }

    #[test]
    #[cfg(feature = "inspect")]
    fn test_inspect_patch_vsf() {
        use vsf::inspect::inspect_vsf;

        let author = get_author_id();
        let build_hash = *blake3::hash(b"test build").as_bytes();

        let patch = Patch::new(
            author,
            None,
            1234567890,
            "Test patch".to_string(),
            vec![FileOp::AddFile {
                path: PathBuf::from("src/main.rs"),
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
