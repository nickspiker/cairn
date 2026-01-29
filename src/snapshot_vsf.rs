//! Snapshot VSF storage - stores complete workspace state in a single VSF file
//!
//! Each snapshot is a VSF file containing the entire workspace using direct labels:
//! ```text
//! [snapshot_metadata]
//!   created: eu6{oscillations}
//!
//! [files]
//!   (Cargo.toml: x{text})
//!   (src/main.rs: x{text})
//!   (src/lib.rs: x{text})
//!   (data/image.png: vb{binary})
//! ```
//!
//! Benefits:
//! - Single provenance hash verifies entire snapshot
//! - Compact direct labels for file paths (no nested sections)
//! - Automatic compression for text files (Huffman)
//! - Self-contained atomic snapshots

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use vsf::types::EtType;
use vsf::types::eagle_time;
use vsf::verification::compute_provenance_hash;
use vsf::{VsfBuilder, VsfSection, VsfType};

use crate::hash_encoding::base58_encode;

/// Create a snapshot VSF file from a HashMap of files
///
/// Returns the provenance hash of the snapshot (used to identify it)
pub fn create_snapshot(files: &HashMap<PathBuf, Vec<u8>>, cairn_dir: &Path) -> Result<[u8; 32]> {
    let mut builder = VsfBuilder::new();

    // Add metadata section
    let mut metadata = VsfSection::new("snapshot_metadata");
    metadata.add_field(
        "created",
        VsfType::e(EtType::u(eagle_time::eagle_time_oscillations())),
    );
    builder = builder.add_section_direct(metadata);

    // Build nested directory structure
    let files_section = build_file_tree(files)?;
    builder = builder.add_section_direct(files_section);

    // Build VSF bytes
    let vsf_bytes = builder.build().map_err(|e| anyhow!("{}", e))?;

    // Compute provenance hash (identifies this snapshot)
    let provenance = compute_provenance_hash(&vsf_bytes).map_err(|e| anyhow!("{}", e))?;

    // Write snapshot to file
    let snapshots_dir = cairn_dir.join("snapshots");
    fs::create_dir_all(&snapshots_dir).context("Failed to create snapshots directory")?;

    let snapshot_path = snapshots_dir.join(format!("{}.vsf", base58_encode(&provenance)));
    fs::write(&snapshot_path, &vsf_bytes).context("Failed to write snapshot VSF")?;

    Ok(provenance)
}

/// Build a VSF section with direct labels for each file
///
/// Creates a flat section with labels like (src.main_rs: x"content")
/// Paths use dots for hierarchy (VSF requirement) and underscores for filename parts
fn build_file_tree(files: &HashMap<PathBuf, Vec<u8>>) -> Result<VsfSection> {
    let mut root = VsfSection::new("files");

    // Add each file as a direct label with its content
    for (path, content) in files {
        // Convert path to VSF-compatible label: replace / with . and escape special chars
        let path_str = path_to_vsf_label(path)?;

        // Store all files as wrapped binary (no compression during snapshot)
        let content_value = VsfType::v(b'b', content.to_vec());

        // Add as direct label (path: content)
        root.add_field(&path_str, content_value);
    }

    Ok(root)
}

/// Convert a file path to a VSF-compatible label
///
/// Uses hex encoding for the full path to avoid any character conflicts
/// Hex encoding only uses 0-9a-f, which are all valid lowercase VSF characters
pub fn path_to_vsf_label(path: &Path) -> Result<String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow!("Invalid UTF-8 in path: {:?}", path))?;

    // Hex encode the path - uses only 0-9a-f (all lowercase, VSF-compatible)
    let encoded = hex::encode(path_str.as_bytes());

    // Prefix with f_ to indicate it's a file label
    let label = format!("f_{}", encoded);

    Ok(label)
}

/// Convert a VSF label back to a file path
pub fn vsf_label_to_path(label: &str) -> Result<PathBuf> {
    // Remove "f_" prefix
    let encoded = label
        .strip_prefix("f_")
        .ok_or_else(|| anyhow!("Invalid file label: {}", label))?;

    // Hex decode
    let decoded =
        hex::decode(encoded).map_err(|e| anyhow!("Failed to decode file label: {}", e))?;

    let path_str =
        String::from_utf8(decoded).map_err(|e| anyhow!("Invalid UTF-8 in decoded path: {}", e))?;

    Ok(PathBuf::from(path_str))
}

/// Check if content is likely text (vs binary)
///
/// Uses simple heuristic: check for NULL bytes and high ratio of printable ASCII
pub fn is_likely_text(content: &[u8]) -> bool {
    if content.is_empty() {
        return true;
    }

    // Check for NULL bytes (strong indicator of binary)
    if content.contains(&0) {
        return false;
    }

    // Check ratio of printable ASCII + common whitespace
    let printable_count = content
        .iter()
        .filter(|&&b| matches!(b, 0x20..=0x7E | b'\t' | b'\n' | b'\r'))
        .count();

    let ratio = printable_count as f64 / content.len() as f64;

    // If >90% printable, consider it text
    ratio > 0.9
}

/// Read a specific file from a snapshot VSF
///
/// Returns the file content as bytes
pub fn read_file_from_snapshot(
    cairn_dir: &Path,
    snapshot_hash: &[u8; 32],
    file_path: &Path,
) -> Result<Vec<u8>> {
    // Load snapshot VSF
    let snapshot_path = cairn_dir
        .join("snapshots")
        .join(format!("{}.vsf", base58_encode(snapshot_hash)));

    let bytes = fs::read(&snapshot_path).context(format!(
        "Failed to load snapshot: {}",
        base58_encode(snapshot_hash)
    ))?;

    // Parse VSF header
    let (header, _header_len) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode VSF header: {}", e))?;

    // Find the "files" section
    let files_field = header
        .fields
        .iter()
        .find(|f| f.name == "files")
        .context("Snapshot missing 'files' section")?;

    // Parse the files section
    let mut ptr = files_field.offset_bytes;
    let files_section = VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow!("Failed to parse files section: {}", e))?;

    // Convert path to hex-encoded label for lookup
    let label = path_to_vsf_label(file_path)?;

    // Find the file field directly by label
    let content_field = files_section
        .get_field(&label)
        .ok_or_else(|| anyhow!("File '{:?}' not found in snapshot", file_path))?;

    // Decode based on VSF type (field values is a Vec)
    if let Some(value) = content_field.values.first() {
        match value {
            VsfType::x(text) => Ok(text.as_bytes().to_vec()),
            VsfType::v(b'b', bytes) => Ok(bytes.clone()),
            _ => Err(anyhow!(
                "Unexpected content type for file {:?}: {:?}",
                file_path,
                value
            )),
        }
    } else {
        Err(anyhow!("Empty content field for file {:?}", file_path))
    }
}

/// Load all files from a snapshot and decode them
///
/// Returns a HashMap of file paths to decoded content (raw bytes)
pub fn load_snapshot_files(
    cairn_dir: &Path,
    snapshot_hash: &[u8; 32],
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    // Load snapshot VSF
    let snapshot_path = cairn_dir
        .join("snapshots")
        .join(format!("{}.vsf", base58_encode(snapshot_hash)));

    let bytes = fs::read(&snapshot_path).context(format!(
        "Failed to load snapshot: {}",
        base58_encode(snapshot_hash)
    ))?;

    // Parse VSF header
    let (header, _header_len) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode VSF header: {}", e))?;

    // Find the "files" section
    let files_field = header
        .fields
        .iter()
        .find(|f| f.name == "files")
        .context("Snapshot missing 'files' section")?;

    // Parse the files section
    let mut ptr = files_field.offset_bytes;
    let files_section = VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow!("Failed to parse files section: {}", e))?;

    // Decode all files
    let mut result = HashMap::new();

    for field in &files_section.fields {
        // Convert hex label back to path
        let path = vsf_label_to_path(&field.name)?;

        // Decode content
        if let Some(value) = field.values.first() {
            let content = match value {
                VsfType::x(text) => text.as_bytes().to_vec(),
                VsfType::v(b'b', bytes) => bytes.clone(),
                _ => continue, // Skip unknown types
            };

            result.insert(path, content);
        }
    }

    Ok(result)
}

/// Check if a snapshot exists
pub fn snapshot_exists(cairn_dir: &Path, snapshot_hash: &[u8; 32]) -> bool {
    let snapshot_path = cairn_dir
        .join("snapshots")
        .join(format!("{}.vsf", base58_encode(snapshot_hash)));

    snapshot_path.exists()
}

/// Extract x-encoded bytes for a single file from a snapshot
///
/// Returns the raw x-encoded Huffman bytes for text files,
/// or raw binary bytes for binary files.
/// This is used for computing deltas on compressed content.
pub fn extract_encoded_file(
    cairn_dir: &Path,
    snapshot_hash: &[u8; 32],
    file_path: &Path,
) -> Result<Vec<u8>> {
    let snapshot_path = cairn_dir
        .join("snapshots")
        .join(format!("{}.vsf", base58_encode(snapshot_hash)));

    let bytes = fs::read(&snapshot_path).context(format!(
        "Failed to load snapshot: {}",
        base58_encode(snapshot_hash)
    ))?;

    // Parse VSF header
    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode VSF header: {}", e))?;

    // Find the "files" section
    let files_field = header
        .fields
        .iter()
        .find(|f| f.name == "files")
        .context("Snapshot missing 'files' section")?;

    // Parse the files section
    let mut ptr = files_field.offset_bytes;
    let files_section = vsf::file_format::VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow!("Failed to parse files section: {}", e))?;

    // Convert path to hex-encoded label for lookup
    let label = path_to_vsf_label(file_path)?;

    // Find the file field directly by label
    let content_field = files_section
        .get_field(&label)
        .context(format!("File '{:?}' not found in snapshot", file_path))?;

    let value = content_field
        .values
        .first()
        .context("Content field has no value")?;

    // Re-encode based on type to get x-encoded bytes
    match value {
        VsfType::x(text) => {
            // Text file - re-encode using Huffman to get x-encoded bytes
            Ok(vsf::text_encoding::encode_text(text))
        }
        VsfType::v(b'b', bytes) => {
            // Binary file - return raw bytes
            Ok(bytes.clone())
        }
        _ => Err(anyhow!("Unexpected content type in file section")),
    }
}

/// Extract x-encoded bytes for all files from a snapshot
///
/// Returns a HashMap mapping file paths to their x-encoded content.
/// Text files are returned as Huffman-encoded bytes,
/// binary files are returned as raw bytes.
pub fn extract_encoded_files(
    cairn_dir: &Path,
    snapshot_hash: &[u8; 32],
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    let snapshot_path = cairn_dir
        .join("snapshots")
        .join(format!("{}.vsf", base58_encode(snapshot_hash)));

    let bytes = fs::read(&snapshot_path).context(format!(
        "Failed to load snapshot: {}",
        base58_encode(snapshot_hash)
    ))?;

    // Parse VSF header
    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode VSF header: {}", e))?;

    // Find the "files" section
    let files_field = header.fields.iter().find(|f| f.name == "files");

    // If no files section or it's empty, return empty HashMap
    let files_field = match files_field {
        Some(field) => field,
        None => return Ok(HashMap::new()),
    };

    // Check if the section has any content (size > 0)
    if files_field.size_bytes == 0 {
        // Empty files section - no files in snapshot
        return Ok(HashMap::new());
    }

    // Parse the files section
    let mut ptr = files_field.offset_bytes;
    let files_section = vsf::file_format::VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow!("Failed to parse files section: {}", e))?;

    // Extract all files from direct labels
    let mut encoded_files = HashMap::new();

    // Iterate over all fields in the files section
    for field in &files_section.fields {
        // Each field is a file with label=path, value=content
        if let Some(value) = field.values.first() {
            // Get file content (raw bytes)
            let encoded_content = match value {
                VsfType::x(text) => {
                    // Legacy x-encoded (Huffman compressed) text - re-encode to bytes
                    // This maintains backward compatibility with old snapshots
                    vsf::text_encoding::encode_text(text)
                }
                VsfType::v(b'b', bytes) => {
                    // Raw bytes (current format)
                    bytes.clone()
                }
                _ => return Err(anyhow!("Unexpected content type for file: {}", field.name)),
            };

            // The field name is the hex-encoded file path - decode it
            let file_path = vsf_label_to_path(&field.name)?;
            encoded_files.insert(file_path, encoded_content);
        }
    }

    Ok(encoded_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_text_detection() {
        assert!(is_likely_text(b"Hello, world!"));
        assert!(is_likely_text(b"fn main() {\n    println!(\"test\");\n}\n"));
        assert!(!is_likely_text(b"\x00\x01\x02\x03"));
        assert!(!is_likely_text(&[0xFF, 0xD8, 0xFF, 0xE0])); // JPEG header
    }

    #[test]
    fn test_snapshot_round_trip() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let cairn_dir = temp_dir.path();

        // Create test files
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("src/main.rs"),
            b"fn main() {\n    println!(\"test\");\n}".to_vec(),
        );
        files.insert(
            PathBuf::from("Cargo.toml"),
            b"[package]\nname = \"test\"\nversion = \"0.1.0\"".to_vec(),
        );

        // Create snapshot
        let snapshot_hash = create_snapshot(&files, cairn_dir)?;

        // Verify snapshot exists
        assert!(snapshot_exists(cairn_dir, &snapshot_hash));

        // Read files back
        let main_rs =
            read_file_from_snapshot(cairn_dir, &snapshot_hash, &PathBuf::from("src/main.rs"))?;
        assert_eq!(main_rs, b"fn main() {\n    println!(\"test\");\n}");

        let cargo_toml =
            read_file_from_snapshot(cairn_dir, &snapshot_hash, &PathBuf::from("Cargo.toml"))?;
        assert_eq!(
            cargo_toml,
            b"[package]\nname = \"test\"\nversion = \"0.1.0\""
        );

        Ok(())
    }
}
