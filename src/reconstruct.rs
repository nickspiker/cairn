//! File reconstruction from blob + diff operations
//!
//! Provides safe reconstruction in a working mirror to avoid interfering
//! with user's working directory. Handles both forward (parent → child)
//! and backward (child → parent) reconstruction.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::blob::{load_blob, store_blob};
use crate::patch_storage::{load_patch, Patch};
use crate::diff::compute_byte_level_diff;
use crate::patch::ByteOp;
use crate::state::Blake3Hash;
use crate::tree::load_tree;

/// Working mirror for safe file reconstruction
///
/// Maintains a clean workspace separate from user's working directory
/// where we can reconstruct files from blobs + diffs without interference.
pub struct WorkingMirror {
    /// Path to working mirror directory
    mirror_dir: PathBuf,
}

impl WorkingMirror {
    /// Create a new working mirror in a temporary directory
    pub fn new() -> Result<Self> {
        let mirror_dir = std::env::temp_dir().join(format!("cairn-mirror-{}", std::process::id()));
        fs::create_dir_all(&mirror_dir)
            .with_context(|| format!("Failed to create working mirror at {:?}", mirror_dir))?;

        Ok(Self { mirror_dir })
    }

    /// Get path within working mirror for a file
    pub fn mirror_path(&self, file_path: &Path) -> PathBuf {
        self.mirror_dir.join(file_path)
    }

    /// Write reconstructed content to mirror
    pub fn write_file(&self, file_path: &Path, content: &[u8]) -> Result<()> {
        let mirror_path = self.mirror_path(file_path);

        // Create parent directories if needed
        if let Some(parent) = mirror_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&mirror_path, content)
            .with_context(|| format!("Failed to write to mirror: {:?}", mirror_path))
    }

    /// Read content from mirror
    pub fn read_file(&self, file_path: &Path) -> Result<Vec<u8>> {
        let mirror_path = self.mirror_path(file_path);
        fs::read(&mirror_path)
            .with_context(|| format!("Failed to read from mirror: {:?}", mirror_path))
    }
}

impl Drop for WorkingMirror {
    fn drop(&mut self) {
        // Clean up temporary mirror on drop
        let _ = fs::remove_dir_all(&self.mirror_dir);
    }
}

/// Reconstruct file content by applying diff operations to base blob
///
/// Forward reconstruction: old_blob + diff_ops → new_content
///
/// # Arguments
/// * `base_content` - Content of the parent blob
/// * `operations` - Diff operations to apply
///
/// # Returns
/// * Reconstructed file content as bytes
///
/// # Edge Cases
/// * Empty base_content: operations should be all Insert
/// * Empty operations: returns base_content unchanged
/// * Invalid Copy offsets: returns error
pub fn apply_diff_forward(base_content: &[u8], operations: &[ByteOp]) -> Result<Vec<u8>> {
    // Empty operations means "no changes" - return base unchanged
    if operations.is_empty() {
        return Ok(base_content.to_vec());
    }

    let mut output = Vec::new();

    for op in operations {
        match op {
            ByteOp::Copy { start, len } => {
                let end = start + len;

                // Bounds check
                if end > base_content.len() {
                    anyhow::bail!(
                        "Copy operation out of bounds: trying to copy {}..{} from content of length {}",
                        start, end, base_content.len()
                    );
                }

                output.extend_from_slice(&base_content[*start..end]);
            }
            ByteOp::Insert { content } => {
                output.extend_from_slice(content);
            }
        }
    }

    Ok(output)
}

/// Reconstruct a file at a specific patch
///
/// Walks back through patch chain, loading blobs and applying diffs
/// until reaching the file's content at the target patch.
///
/// # Arguments
/// * `cairn_dir` - Path to .cairn directory
/// * `patch_hp` - Provenance hash of target patch
/// * `file_path` - Path of file to reconstruct
///
/// # Returns
/// * Reconstructed file content
///
/// # Process
/// 1. Load patch and get tree
/// 2. Get blob hash for file from tree
/// 3. If file has parent diff, reconstruct from parent
/// 4. Otherwise load blob directly
pub fn reconstruct_file_at_patch(
    cairn_dir: &Path,
    patch_hp: &Blake3Hash,
    file_path: &Path,
) -> Result<Vec<u8>> {
    // Load the patch
    let patch = load_patch(cairn_dir, patch_hp)
        .with_context(|| format!("Failed to load patch for reconstruction"))?;

    // Load the tree to get file→blob mapping
    let tree = load_tree(cairn_dir, &patch.tree_hp)
        .with_context(|| format!("Failed to load tree for reconstruction"))?;

    // Get blob hash for this file
    let blob_hash = tree.get(file_path)
        .ok_or_else(|| anyhow::anyhow!("File {:?} not found in tree", file_path))?;

    // For now, just load the blob directly (no diff application yet)
    // TODO: Implement diff-based reconstruction when parent diffs are stored
    let content = load_blob(cairn_dir, blob_hash)
        .with_context(|| format!("Failed to load blob for file {:?}", file_path))?;

    Ok(content)
}

/// Compute reverse diff operations (child → parent)
///
/// Given operations that transform old → new,
/// compute operations that transform new → old.
///
/// Used for backward reconstruction through patch history.
pub fn reverse_diff_operations(
    old_content: &[u8],
    new_content: &[u8],
) -> Vec<ByteOp> {
    // Simply compute diff in reverse direction
    compute_byte_level_diff(new_content, old_content)
}

/// Verify diff correctness by round-trip reconstruction
///
/// Applies forward diff, then reverse diff, and checks we get back original.
/// This is a sanity check for diff quality.
pub fn verify_diff_roundtrip(
    old_content: &[u8],
    new_content: &[u8],
    forward_ops: &[ByteOp],
) -> Result<()> {
    // Forward: old + forward_ops = new
    let reconstructed_new = apply_diff_forward(old_content, forward_ops)?;

    if reconstructed_new != new_content {
        anyhow::bail!(
            "Forward diff failed: reconstructed {} bytes, expected {} bytes",
            reconstructed_new.len(),
            new_content.len()
        );
    }

    // Backward: compute reverse and apply
    let reverse_ops = reverse_diff_operations(old_content, new_content);
    let reconstructed_old = apply_diff_forward(new_content, &reverse_ops)?;

    if reconstructed_old != old_content {
        anyhow::bail!(
            "Reverse diff failed: reconstructed {} bytes, expected {} bytes",
            reconstructed_old.len(),
            old_content.len()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_diff_forward_copy() {
        let base = b"hello world";
        let ops = vec![ByteOp::Copy { start: 0, len: 5 }];

        let result = apply_diff_forward(base, &ops).unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn test_apply_diff_forward_insert() {
        let base = b"hello";
        let ops = vec![ByteOp::Insert {
            content: b" world".to_vec(),
        }];

        let result = apply_diff_forward(base, &ops).unwrap();
        assert_eq!(result, b" world");
    }

    #[test]
    fn test_apply_diff_forward_mixed() {
        let base = b"hello world";
        let ops = vec![
            ByteOp::Copy { start: 0, len: 5 },
            ByteOp::Insert { content: b" beautiful".to_vec() },
            ByteOp::Copy { start: 5, len: 6 },
        ];

        let result = apply_diff_forward(base, &ops).unwrap();
        assert_eq!(result, b"hello beautiful world");
    }

    #[test]
    fn test_apply_diff_forward_empty_base() {
        let base = b"";
        let ops = vec![ByteOp::Insert {
            content: b"new content".to_vec(),
        }];

        let result = apply_diff_forward(base, &ops).unwrap();
        assert_eq!(result, b"new content");
    }

    #[test]
    fn test_apply_diff_forward_empty_ops() {
        let base = b"original";
        let ops = vec![];

        let result = apply_diff_forward(base, &ops).unwrap();
        assert_eq!(result, b"original");
    }

    #[test]
    fn test_apply_diff_forward_bounds_error() {
        let base = b"short";
        let ops = vec![ByteOp::Copy { start: 0, len: 100 }]; // Out of bounds

        let result = apply_diff_forward(base, &ops);
        assert!(result.is_err());
    }

    #[test]
    fn test_reverse_diff() {
        let old = b"hello world";
        let new = b"hello beautiful world";

        let forward_ops = compute_byte_level_diff(old, new);
        let reverse_ops = reverse_diff_operations(old, new);

        // Apply forward
        let reconstructed_new = apply_diff_forward(old, &forward_ops).unwrap();
        assert_eq!(reconstructed_new, new);

        // Apply reverse
        let reconstructed_old = apply_diff_forward(new, &reverse_ops).unwrap();
        assert_eq!(reconstructed_old, old);
    }

    #[test]
    fn test_verify_diff_roundtrip_success() {
        let old = b"original content\nline 2\n";
        let new = b"modified content\nline 2\nextra line\n";

        let ops = compute_byte_level_diff(old, new);
        let result = verify_diff_roundtrip(old, new, &ops);

        assert!(result.is_ok());
    }

    #[test]
    fn test_working_mirror() {
        let mirror = WorkingMirror::new().unwrap();

        // Test with simple filename first
        let simple_path = Path::new("file.txt");
        let content1 = b"test content";

        let mirror_file_path = mirror.mirror_path(simple_path);
        eprintln!("Writing to: {:?}", mirror_file_path);

        mirror.write_file(simple_path, content1).unwrap();

        // Check if file exists
        eprintln!("Checking if file exists: {}", mirror_file_path.exists());
        assert!(mirror_file_path.exists(), "File should exist after write");

        let read_content1 = mirror.read_file(simple_path).unwrap();
        assert_eq!(read_content1, content1);

        // Test with subdirectory
        let nested_path = Path::new("test/file.txt");
        let content2 = b"nested content";

        mirror.write_file(nested_path, content2).unwrap();
        let read_content2 = mirror.read_file(nested_path).unwrap();
        assert_eq!(read_content2, content2);

        // Mirror path should be in temp dir
        let mirror_path = mirror.mirror_path(simple_path);
        assert!(mirror_path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn test_working_mirror_cleanup() {
        let mirror_dir = {
            let mirror = WorkingMirror::new().unwrap();
            let dir = mirror.mirror_dir.clone();
            eprintln!("Created mirror at: {:?}", dir);
            eprintln!("Directory exists before drop: {}", dir.exists());
            dir
        }; // mirror dropped here

        eprintln!("Directory exists after drop: {}", mirror_dir.exists());

        // Give the OS a moment to clean up
        std::thread::sleep(std::time::Duration::from_millis(10));
        eprintln!("Directory exists after sleep: {}", mirror_dir.exists());

        // Directory should be cleaned up after drop
        assert!(!mirror_dir.exists(), "Mirror directory should be cleaned up after drop");
    }
}
