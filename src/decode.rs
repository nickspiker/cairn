//! VSF decoding for cairn patches with binary diff and blob storage
//!
//! Parses VSF-encoded patches back into Rust structures.
//! Decodes FileOp with ByteOp from byte vectors.

use crate::patch::{ByteOp, FileOp, Patch, PatchId, PatchMetadata};
use anyhow::{anyhow, Result};
use vsf::{VsfHeader, VsfSection, VsfType};

impl Patch {
    /// Decode a patch from VSF format
    ///
    /// Takes raw VSF bytes and returns a Patch
    pub fn decode_vsf(bytes: &[u8]) -> Result<Self> {
        // 1. Decode header
        let (header, _header_len) =
            VsfHeader::decode(bytes).map_err(|e| anyhow!("Failed to decode VSF header: {}", e))?;

        // 2. Parse sections
        let mut metadata_opt = None;
        let mut operations = Vec::new();
        let mut build_hash = None;

        for field in &header.fields {
            let mut ptr;
            // Handle empty operations section (size_bytes == 0 means no section body)
            if field.size_bytes == 0 && field.name == "operations" {
                // Empty operations section - no operations to decode
                operations = Vec::new();
                continue;
            }


            // Jump to section offset
            ptr = field.offset_bytes;

            if ptr >= bytes.len() {
                continue; // Skip if offset is beyond data
            }

            // Parse section
            let section = VsfSection::parse(bytes, &mut ptr)
                .map_err(|e| anyhow!("Failed to parse section '{}': {}", field.name, e))?;

            match section.name.as_str() {
                "metadata" => {
                    metadata_opt = Some(decode_metadata_section(&section, &header)?);
                }
                "operations" => {
                    operations = decode_operations_section(&section)?;
                }
                "build_output" => {
                    build_hash = Some(decode_build_section(&section)?);
                }
                _ => {
                    // Unknown section, skip
                }
            }
        }

        // 3. Construct patch
        let metadata = metadata_opt.ok_or_else(|| anyhow!("Missing metadata section"))?;
        let build_hash = build_hash.ok_or_else(|| anyhow!("Missing build_output section"))?;

        Ok(Patch {
            metadata,
            operations,
            build_hash,
        })
    }
}

/// Decode metadata section
fn decode_metadata_section(
    section: &VsfSection,
    _header: &VsfHeader,
) -> Result<PatchMetadata> {
    let author = extract_hash(section, "author")?;
    let parent = extract_hash_opt(section, "parent");
    let message = extract_string(section, "message")?;

    // Extract timestamp from metadata section (not header)
    let timestamp = extract_timestamp(section, "timestamp")?;

    Ok(PatchMetadata {
        author,
        parent,
        timestamp,
        message,
    })
}

/// Decode operations section with FileOp and ByteOp
fn decode_operations_section(section: &VsfSection) -> Result<Vec<FileOp>> {
    let mut ops = Vec::new();

    for field in section.get_fields("op") {
        if field.values.is_empty() {
            continue;
        }

        let op_type = extract_u8_from_value(&field.values[0])?;

        let op = match op_type {
            0 => {
                // ModifyFile: [op_type, path, base_blob, result_hash, byte_ops_encoded]
                if field.values.len() < 5 {
                    return Err(anyhow!("ModifyFile operation missing values"));
                }

                let path = extract_pathbuf_from_value(&field.values[1])?;
                let base_blob = extract_hash_from_value(&field.values[2])?;
                let result_hash = extract_hash_from_value(&field.values[3])?;
                let byte_ops_encoded = extract_bytes_from_value(&field.values[4])?;

                // Decode ByteOp operations
                let operations = decode_byte_ops(&byte_ops_encoded)?;

                FileOp::ModifyFile {
                    path,
                    base_blob,
                    operations,
                    result_hash,
                }
            }
            1 => {
                // AddFile: [op_type, path, content_blob]
                if field.values.len() < 3 {
                    return Err(anyhow!("AddFile operation missing values"));
                }
                FileOp::AddFile {
                    path: extract_pathbuf_from_value(&field.values[1])?,
                    content_blob: extract_hash_from_value(&field.values[2])?,
                }
            }
            2 => {
                // DeleteFile: [op_type, path, old_blob]
                if field.values.len() < 3 {
                    return Err(anyhow!("DeleteFile operation missing values"));
                }
                FileOp::DeleteFile {
                    path: extract_pathbuf_from_value(&field.values[1])?,
                    old_blob: extract_hash_from_value(&field.values[2])?,
                }
            }
            3 => {
                // RenameFile: [op_type, from, to]
                if field.values.len() < 3 {
                    return Err(anyhow!("RenameFile operation missing values"));
                }
                FileOp::RenameFile {
                    from: extract_pathbuf_from_value(&field.values[1])?,
                    to: extract_pathbuf_from_value(&field.values[2])?,
                }
            }
            _ => return Err(anyhow!("Unknown operation type: {}", op_type)),
        };

        ops.push(op);
    }

    Ok(ops)
}

/// Decode ByteOp operations from a byte vector
///
/// Format for each operation:
/// - Copy: [0u8, start_bytes..., len_bytes...]
/// - Insert: [1u8, len_bytes..., content_bytes...]
///
/// Uses variable-length encoding for sizes (LEB128-style).
fn decode_byte_ops(encoded: &[u8]) -> Result<Vec<ByteOp>> {
    let mut ops = Vec::new();
    let mut pos = 0;

    while pos < encoded.len() {
        let op_tag = encoded[pos];
        pos += 1;

        match op_tag {
            0 => {
                // Copy operation
                let (start, bytes_read) = decode_varint(&encoded[pos..])?;
                pos += bytes_read;

                let (len, bytes_read) = decode_varint(&encoded[pos..])?;
                pos += bytes_read;

                ops.push(ByteOp::Copy { start, len });
            }
            1 => {
                // Insert operation
                let (content_len, bytes_read) = decode_varint(&encoded[pos..])?;
                pos += bytes_read;

                if pos + content_len > encoded.len() {
                    return Err(anyhow!("Insert content extends beyond buffer"));
                }

                let content = encoded[pos..pos + content_len].to_vec();
                pos += content_len;

                ops.push(ByteOp::Insert { content });
            }
            _ => return Err(anyhow!("Unknown ByteOp tag: {}", op_tag)),
        }
    }

    Ok(ops)
}

/// Decode variable-length integer (LEB128)
fn decode_varint(buf: &[u8]) -> Result<(usize, usize)> {
    let mut result = 0usize;
    let mut shift = 0;
    let mut bytes_read = 0;

    for &byte in buf {
        bytes_read += 1;

        result |= ((byte & 0x7F) as usize) << shift;

        if byte & 0x80 == 0 {
            // Last byte
            return Ok((result, bytes_read));
        }

        shift += 7;

        if shift >= 64 {
            return Err(anyhow!("Varint too large"));
        }
    }

    Err(anyhow!("Incomplete varint"))
}

/// Decode build output section
fn decode_build_section(section: &VsfSection) -> Result<[u8; 32]> {
    extract_hash(section, "cargo_hash")
}

// ==================== Helper Functions ====================

/// Extract a BLAKE3 hash (32 bytes) from a field
fn extract_hash(section: &VsfSection, field_name: &str) -> Result<[u8; 32]> {
    let field = section
        .get_field(field_name)
        .ok_or_else(|| anyhow!("Missing field: {}", field_name))?;

    if field.values.is_empty() {
        return Err(anyhow!("Field '{}' has no values", field_name));
    }

    match &field.values[0] {
        VsfType::hp(bytes) | VsfType::hb(bytes) | VsfType::hs(bytes) | VsfType::hm(bytes) | VsfType::hg(bytes) | VsfType::hc(bytes) | VsfType::hk(bytes) => {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                Ok(arr)
            } else {
                Err(anyhow!(
                    "Invalid hash length for '{}': {} (expected 32)",
                    field_name,
                    bytes.len()
                ))
            }
        }
        _ => Err(anyhow!("Field '{}' is not a hash type", field_name)),
    }
}

/// Extract an optional BLAKE3 hash
fn extract_hash_opt(section: &VsfSection, field_name: &str) -> Option<PatchId> {
    extract_hash(section, field_name).ok()
}

/// Extract a string from a field
fn extract_string(section: &VsfSection, field_name: &str) -> Result<String> {
    let field = section
        .get_field(field_name)
        .ok_or_else(|| anyhow!("Missing field: {}", field_name))?;

    if field.values.is_empty() {
        return Err(anyhow!("Field '{}' has no values", field_name));
    }

    match &field.values[0] {
        VsfType::x(s) | VsfType::l(s) | VsfType::d(s) => Ok(s.clone()),
        _ => Err(anyhow!("Field '{}' is not a string type", field_name)),
    }
}

/// Extract u8 from a field
fn extract_u8(section: &VsfSection, field_name: &str) -> Result<u8> {
    let field = section
        .get_field(field_name)
        .ok_or_else(|| anyhow!("Missing field: {}", field_name))?;

    if field.values.is_empty() {
        return Err(anyhow!("Field '{}' has no values", field_name));
    }

    extract_u8_from_value(&field.values[0])
}

/// Extract u8 from a VsfType value
fn extract_u8_from_value(value: &VsfType) -> Result<u8> {
    match value {
        VsfType::u0(b) => Ok(*b as u8),
        VsfType::u3(n) => Ok(*n),
        VsfType::u4(n) => {
            if *n <= u8::MAX as u16 {
                Ok(*n as u8)
            } else {
                Err(anyhow!("Value {} too large for u8", n))
            }
        }
        _ => Err(anyhow!("Not an unsigned integer type")),
    }
}

/// Extract usize from a VsfType value
fn extract_usize_from_value(value: &VsfType) -> Result<usize> {
    match value {
        VsfType::u0(b) => Ok(*b as usize),
        VsfType::u3(n) => Ok(*n as usize),
        VsfType::u4(n) => Ok(*n as usize),
        VsfType::u5(n) => Ok(*n as usize),
        VsfType::u6(n) => Ok(*n as usize),
        VsfType::o(n) | VsfType::n(n) => Ok(*n),
        _ => Err(anyhow!("Not an unsigned integer type")),
    }
}

/// Extract bytes from a VsfType value
fn extract_bytes_from_value(value: &VsfType) -> Result<Vec<u8>> {
    match value {
        VsfType::v_u3(vector) => Ok(vector.data.clone()),
        VsfType::t_u3(tensor) => Ok(tensor.data.clone()),
        VsfType::v(_, bytes) => Ok(bytes.clone()),
        _ => Err(anyhow!("Not a byte array type, got: {:?}", value)),
    }
}

/// Extract PathBuf from a VsfType value
fn extract_pathbuf_from_value(value: &VsfType) -> Result<std::path::PathBuf> {
    match value {
        VsfType::l(s) | VsfType::x(s) | VsfType::d(s) => Ok(std::path::PathBuf::from(s)),
        _ => Err(anyhow!("Not a string/path type")),
    }
}

/// Extract Eagle Time (f64) from a VsfType
fn extract_eagle_time(value: &VsfType) -> Result<f64> {
    match value {
        VsfType::e(et_type) => match et_type {
            vsf::EtType::f5(f) => Ok(*f as f64),
            vsf::EtType::f6(f) => Ok(*f),
            vsf::EtType::u(u) => Ok(*u as f64),
            _ => Err(anyhow!("Unsupported Eagle Time format")),
        },
        _ => Err(anyhow!("Not an Eagle Time type")),
    }
}

/// Extract timestamp (Eagle Time) from a field
fn extract_timestamp(section: &VsfSection, field_name: &str) -> Result<f64> {
    let field = section
        .get_field(field_name)
        .ok_or_else(|| anyhow!("Missing field: {}", field_name))?;

    if field.values.is_empty() {
        return Err(anyhow!("Field '{}' has no values", field_name));
    }

    extract_eagle_time(&field.values[0])
}

/// Extract a hash from a VsfType value (for inline hash extraction)
fn extract_hash_from_value(value: &VsfType) -> Result<[u8; 32]> {
    match value {
        VsfType::hp(bytes) | VsfType::hb(bytes) | VsfType::hs(bytes) | VsfType::hm(bytes) | VsfType::hg(bytes) | VsfType::hc(bytes) | VsfType::hk(bytes) => {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                Ok(arr)
            } else {
                Err(anyhow!(
                    "Invalid hash length: {} (expected 32)",
                    bytes.len()
                ))
            }
        }
        _ => Err(anyhow!("Not a hash type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::get_author_id;
    use std::path::PathBuf;

    #[test]
    fn test_roundtrip_patch() {
        let author = get_author_id();
        let build_hash = *blake3::hash(b"test build output").as_bytes();
        let content_blob = *blake3::hash(b"file content").as_bytes();
        let base_blob = *blake3::hash(b"base content").as_bytes();
        let result_hash = *blake3::hash(b"result content").as_bytes();
        let old_blob = *blake3::hash(b"old content").as_bytes();

        let original = Patch::new(
            author,
            None,
            1234567890.0,
            "Test patch message".to_string(),
            vec![
                FileOp::AddFile {
                    path: PathBuf::from("src/new.rs"),
                    content_blob,
                },
                FileOp::ModifyFile {
                    path: PathBuf::from("src/main.rs"),
                    base_blob,
                    operations: vec![
                        ByteOp::Copy { start: 0, len: 10 },
                        ByteOp::Insert {
                            content: b"new code".to_vec(),
                        },
                    ],
                    result_hash,
                },
                FileOp::DeleteFile {
                    path: PathBuf::from("src/old.rs"),
                    old_blob,
                },
                FileOp::RenameFile {
                    from: PathBuf::from("old.txt"),
                    to: PathBuf::from("new.txt"),
                },
            ],
            build_hash,
        );

        // Encode
        let encoded = original.encode_vsf().unwrap();

        // Decode
        let decoded = Patch::decode_vsf(&encoded).unwrap();

        // Verify roundtrip
        assert_eq!(decoded.metadata.author, original.metadata.author);
        assert_eq!(decoded.metadata.parent, original.metadata.parent);
        assert_eq!(decoded.metadata.message, original.metadata.message);
        assert_eq!(decoded.operations.len(), original.operations.len());
        assert_eq!(decoded.build_hash, original.build_hash);

        // Check operations
        for (i, (orig_op, dec_op)) in original
            .operations
            .iter()
            .zip(decoded.operations.iter())
            .enumerate()
        {
            assert_eq!(orig_op, dec_op, "Operation {} mismatch", i);
        }
    }

    #[test]
    fn test_decode_with_parent() {
        let author = get_author_id();
        let parent = *blake3::hash(b"parent patch").as_bytes();
        let build_hash = *blake3::hash(b"build").as_bytes();

        let original = Patch::new(
            author,
            Some(parent),
            1234567890.0,
            "Child patch".to_string(),
            vec![],
            build_hash,
        );

        let encoded = original.encode_vsf().unwrap();
        let decoded = Patch::decode_vsf(&encoded).unwrap();

        assert_eq!(decoded.metadata.parent, Some(parent));
    }

    #[test]
    fn test_byte_ops_roundtrip() {
        use crate::encode::encode_byte_ops;

        let original_ops = vec![
            ByteOp::Copy { start: 0, len: 100 },
            ByteOp::Insert {
                content: b"hello world".to_vec(),
            },
            ByteOp::Copy {
                start: 200,
                len: 50,
            },
        ];

        let encoded = encode_byte_ops(&original_ops);
        let decoded = decode_byte_ops(&encoded).unwrap();

        assert_eq!(original_ops, decoded);
    }

    #[test]
    fn test_varint_roundtrip() {
        use crate::encode::encode_varint;

        let test_values = vec![0, 1, 127, 128, 255, 256, 16383, 16384, 1000000];

        for value in test_values {
            let mut buf = Vec::new();
            encode_varint(&mut buf, value);

            let (decoded, bytes_read) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, value);
            assert_eq!(bytes_read, buf.len());
        }
    }
}
