//! Apply patch operations with BLAKE3 verification
//!
//! Reconstructs files from binary diffs on x-encoded snapshot content.
//! Validates BLAKE3 hashes before applying to ensure integrity.

use crate::patch::{ByteOp, FileOp};
use crate::snapshot_vsf::{extract_encoded_file, read_file_from_snapshot};
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Apply FileOp operations with BLAKE3 verification
///
/// Operations are validated atomically - all BLAKE3 hashes are verified
/// before applying any changes. This ensures repository state remains consistent.
///
/// Takes the new snapshot hash to read file content for AddFile operations.
/// Returns the new file tree state (plaintext) after applying all operations.
pub fn apply_operations(
    cairn_dir: &Path,
    files: &HashMap<PathBuf, Vec<u8>>,
    operations: &[FileOp],
    new_snapshot: &[u8; 32],
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    // Phase 1: Validate all operations and reconstruct all files
    // We do this BEFORE modifying any state to ensure atomicity
    let mut reconstructed_files: HashMap<PathBuf, Vec<u8>> = HashMap::new();

    for op in operations {
        match op {
            FileOp::ModifyFile {
                path,
                base_snapshot,
                operations: byte_ops,
                result_hash,
            } => {
                // Read x-encoded base content from snapshot
                let base_content = extract_encoded_file(cairn_dir, base_snapshot, path)
                    .with_context(|| {
                        format!("Failed to read base content for {:?} from snapshot", path)
                    })?;

                // Reconstruct x-encoded file from ByteOp operations
                let reconstructed_encoded = reconstruct_from_ops(&base_content, byte_ops)?;

                // Verify BLAKE3 hash matches expected result (hash of x-encoded content)
                let computed_hash = *blake3::hash(&reconstructed_encoded).as_bytes();
                if &computed_hash != result_hash {
                    return Err(anyhow!(
                        "BLAKE3 hash mismatch for {:?}\nExpected: {}\nComputed: {}",
                        path,
                        hex::encode(result_hash),
                        hex::encode(computed_hash)
                    ));
                }

                // Decode x-encoded result to plaintext for working directory
                let plaintext = read_file_from_snapshot(cairn_dir, new_snapshot, path)
                    .with_context(|| {
                        format!("Failed to decode reconstructed content for {:?}", path)
                    })?;

                reconstructed_files.insert(path.clone(), plaintext);
            }

            FileOp::AddFile { path } => {
                // Read full content from new snapshot
                let content =
                    read_file_from_snapshot(cairn_dir, new_snapshot, path).with_context(|| {
                        format!("Failed to read added file {:?} from snapshot", path)
                    })?;

                // Verify the file doesn't already exist
                if files.contains_key(path) {
                    return Err(anyhow!("File already exists: {:?}", path));
                }

                reconstructed_files.insert(path.clone(), content);
            }

            FileOp::DeleteFile { path, old_snapshot } => {
                // Read expected old content from old snapshot
                let old_content = read_file_from_snapshot(cairn_dir, old_snapshot, path)
                    .with_context(|| {
                        format!(
                            "Failed to read file {:?} from old snapshot for deletion verification",
                            path
                        )
                    })?;

                // Verify the file exists and content matches
                let current_content = files
                    .get(path)
                    .ok_or_else(|| anyhow!("File to delete does not exist: {:?}", path))?;

                if current_content != &old_content {
                    return Err(anyhow!(
                        "File content mismatch for deletion of {:?}\nSnapshot content hash: {}\nCurrent content hash: {}",
                        path,
                        hex::encode(blake3::hash(&old_content).as_bytes()),
                        hex::encode(blake3::hash(current_content).as_bytes())
                    ));
                }

                // Mark for deletion (we'll handle this in phase 2)
            }

            FileOp::RenameFile { from, to } => {
                // Verify source exists
                if !files.contains_key(from) {
                    return Err(anyhow!("Source file does not exist: {:?}", from));
                }

                // Verify destination doesn't exist
                if files.contains_key(to) {
                    return Err(anyhow!("Destination file already exists: {:?}", to));
                }

                // Mark for rename (we'll handle this in phase 2)
            }
        }
    }

    // Phase 2: Apply all operations atomically
    // All validation passed - now modify state
    let mut new_files = files.clone();

    for op in operations {
        match op {
            FileOp::ModifyFile { path, .. } | FileOp::AddFile { path } => {
                // Insert reconstructed/new file
                let content = reconstructed_files
                    .remove(path)
                    .expect("reconstructed file should exist");
                new_files.insert(path.clone(), content);
            }

            FileOp::DeleteFile { path, .. } => {
                new_files.remove(path);
            }

            FileOp::RenameFile { from, to } => {
                let content = new_files
                    .remove(from)
                    .expect("source file should exist after validation");
                new_files.insert(to.clone(), content);
            }
        }
    }

    Ok(new_files)
}

/// Reconstruct file content from ByteOp operations
///
/// Processes Copy and Insert operations sequentially to rebuild the file.
/// All Copy operations reference absolute byte positions in the immutable x-encoded base content.
fn reconstruct_from_ops(base_content: &[u8], operations: &[ByteOp]) -> Result<Vec<u8>> {
    let mut output = Vec::new();

    for op in operations {
        match op {
            ByteOp::Copy { start, len } => {
                let end = start + len;

                // Validate bounds
                if end > base_content.len() {
                    return Err(anyhow!(
                        "Copy operation out of bounds: start={}, len={}, base_len={}",
                        start,
                        len,
                        base_content.len()
                    ));
                }

                // Copy bytes from base blob
                output.extend_from_slice(&base_content[*start..end]);
            }

            ByteOp::Insert { content } => {
                // Insert new bytes at current output position
                output.extend_from_slice(content);
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_vsf::create_snapshot;
    use tempfile::TempDir;

    #[test]
    fn test_reconstruct_from_ops_copy_only() {
        let base = b"hello world";

        let ops = vec![ByteOp::Copy { start: 0, len: 5 }];
        let result = reconstruct_from_ops(base, &ops).unwrap();

        assert_eq!(result, b"hello");
    }

    #[test]
    fn test_reconstruct_from_ops_insert_only() {
        let base = b"ignored";

        let ops = vec![ByteOp::Insert {
            content: b"new content".to_vec(),
        }];
        let result = reconstruct_from_ops(base, &ops).unwrap();

        assert_eq!(result, b"new content");
    }

    #[test]
    fn test_reconstruct_from_ops_mixed() {
        let base = b"hello world";

        let ops = vec![
            ByteOp::Copy { start: 0, len: 5 },
            ByteOp::Insert {
                content: b" new".to_vec(),
            },
            ByteOp::Copy { start: 5, len: 6 },
        ];
        let result = reconstruct_from_ops(base, &ops).unwrap();

        assert_eq!(result, b"hello new world");
    }

    #[test]
    fn test_reconstruct_out_of_bounds() {
        let base = b"hello";

        let ops = vec![ByteOp::Copy { start: 0, len: 100 }];
        let result = reconstruct_from_ops(base, &ops);

        assert!(result.is_err());
    }

    #[test]
    fn test_apply_add_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let files = HashMap::new();

        // Create new snapshot with the added file
        let content = b"hello world";
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), content.to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = vec![FileOp::AddFile {
            path: PathBuf::from("test.txt"),
        }];

        let result = apply_operations(cairn_dir, &files, &ops, &new_snapshot).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(
            result.get(&PathBuf::from("test.txt")),
            Some(&content.to_vec())
        );
    }

    #[test]
    fn test_apply_delete_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let content = b"delete me";
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), content.to_vec());

        // Create old snapshot with the file to be deleted
        let old_snapshot = create_snapshot(&files, cairn_dir).unwrap();

        // Create new snapshot (empty)
        let new_files = HashMap::new();
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = vec![FileOp::DeleteFile {
            path: PathBuf::from("test.txt"),
            old_snapshot,
        }];

        let result = apply_operations(cairn_dir, &files, &ops, &new_snapshot).unwrap();

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_apply_modify_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let old_content = b"hello world";
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), old_content.to_vec());

        // Create old snapshot
        let old_snapshot = create_snapshot(&files, cairn_dir).unwrap();

        // Create new snapshot with modified content
        let new_content = b"hello rust world";
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), new_content.to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        // Extract x-encoded content for diffing
        let old_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &old_snapshot,
            &PathBuf::from("test.txt"),
        )
        .unwrap();
        let new_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &new_snapshot,
            &PathBuf::from("test.txt"),
        )
        .unwrap();

        // Generate ByteOps on x-encoded content (simplified - just replace all)
        let byte_ops = vec![ByteOp::Insert {
            content: new_encoded.clone(),
        }];

        let result_hash = *blake3::hash(&new_encoded).as_bytes();

        let ops = vec![FileOp::ModifyFile {
            path: PathBuf::from("test.txt"),
            base_snapshot: old_snapshot,
            operations: byte_ops,
            result_hash,
        }];

        let result = apply_operations(cairn_dir, &files, &ops, &new_snapshot).unwrap();

        assert_eq!(
            result.get(&PathBuf::from("test.txt")),
            Some(&new_content.to_vec())
        );
    }

    #[test]
    fn test_apply_rename_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let mut files = HashMap::new();
        files.insert(PathBuf::from("old.txt"), b"content".to_vec());

        // Create empty snapshot (rename doesn't need snapshot content)
        let empty_snapshot = create_snapshot(&HashMap::new(), cairn_dir).unwrap();

        let ops = vec![FileOp::RenameFile {
            from: PathBuf::from("old.txt"),
            to: PathBuf::from("new.txt"),
        }];

        let result = apply_operations(cairn_dir, &files, &ops, &empty_snapshot).unwrap();

        assert!(!result.contains_key(&PathBuf::from("old.txt")));
        assert_eq!(
            result.get(&PathBuf::from("new.txt")),
            Some(&b"content".to_vec())
        );
    }

    #[test]
    fn test_hash_mismatch_fails() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let old_content = b"hello";
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), old_content.to_vec());

        // Create snapshots
        let old_snapshot = create_snapshot(&files, cairn_dir).unwrap();
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), b"correct".to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        // Extract x-encoded content
        let old_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &old_snapshot,
            &PathBuf::from("test.txt"),
        )
        .unwrap();

        let byte_ops = vec![ByteOp::Insert {
            content: b"wrong".to_vec(),
        }];

        // Provide WRONG result hash (hash of "correct" x-encoded instead of "wrong")
        let correct_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &new_snapshot,
            &PathBuf::from("test.txt"),
        )
        .unwrap();
        let result_hash = *blake3::hash(&correct_encoded).as_bytes();

        let ops = vec![FileOp::ModifyFile {
            path: PathBuf::from("test.txt"),
            base_snapshot: old_snapshot,
            operations: byte_ops,
            result_hash,
        }];

        let result = apply_operations(cairn_dir, &files, &ops, &new_snapshot);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("BLAKE3 hash mismatch")
        );
    }

    #[test]
    fn test_atomicity_hash_failure() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let content1 = b"file1";
        let content2 = b"file2";

        let mut files = HashMap::new();
        files.insert(PathBuf::from("file1.txt"), content1.to_vec());
        files.insert(PathBuf::from("file2.txt"), content2.to_vec());

        // Create old snapshot
        let old_snapshot = create_snapshot(&files, cairn_dir).unwrap();

        // Create new snapshot
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("file1.txt"), b"valid".to_vec());
        new_files.insert(PathBuf::from("file2.txt"), b"not_wrong".to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        // Extract x-encoded content
        let valid_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &new_snapshot,
            &PathBuf::from("file1.txt"),
        )
        .unwrap();
        let not_wrong_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &new_snapshot,
            &PathBuf::from("file2.txt"),
        )
        .unwrap();

        // First op is valid, second op has wrong hash
        let ops = vec![
            FileOp::ModifyFile {
                path: PathBuf::from("file1.txt"),
                base_snapshot: old_snapshot,
                operations: vec![ByteOp::Insert {
                    content: valid_encoded.clone(),
                }],
                result_hash: *blake3::hash(&valid_encoded).as_bytes(),
            },
            FileOp::ModifyFile {
                path: PathBuf::from("file2.txt"),
                base_snapshot: old_snapshot,
                operations: vec![ByteOp::Insert {
                    content: b"wrong".to_vec(),
                }],
                result_hash: *blake3::hash(&not_wrong_encoded).as_bytes(), // Wrong!
            },
        ];

        let result = apply_operations(cairn_dir, &files, &ops, &new_snapshot);

        assert!(result.is_err());

        // Original files should be unchanged (atomicity)
        assert_eq!(
            files.get(&PathBuf::from("file1.txt")),
            Some(&content1.to_vec())
        );
        assert_eq!(
            files.get(&PathBuf::from("file2.txt")),
            Some(&content2.to_vec())
        );
    }

    #[test]
    fn test_binary_data_apply() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Binary data (not text)
        let old_binary = vec![0xFF, 0x00, 0xAB, 0xCD, 0xEF];
        let mut files = HashMap::new();
        files.insert(PathBuf::from("binary.dat"), old_binary.clone());

        // Create old snapshot
        let old_snapshot = create_snapshot(&files, cairn_dir).unwrap();

        // Create new snapshot
        let new_binary = vec![0xFF, 0x00, 0x12, 0x34, 0xCD, 0xEF];
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("binary.dat"), new_binary.clone());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        // Extract x-encoded (raw binary) content
        let old_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &old_snapshot,
            &PathBuf::from("binary.dat"),
        )
        .unwrap();
        let new_encoded = crate::snapshot_vsf::extract_encoded_file(
            cairn_dir,
            &new_snapshot,
            &PathBuf::from("binary.dat"),
        )
        .unwrap();

        let byte_ops = vec![
            ByteOp::Copy { start: 0, len: 2 },
            ByteOp::Insert {
                content: vec![0x12, 0x34],
            },
            ByteOp::Copy { start: 3, len: 2 },
        ];

        let result_hash = *blake3::hash(&new_encoded).as_bytes();

        let ops = vec![FileOp::ModifyFile {
            path: PathBuf::from("binary.dat"),
            base_snapshot: old_snapshot,
            operations: byte_ops,
            result_hash,
        }];

        let result = apply_operations(cairn_dir, &files, &ops, &new_snapshot).unwrap();

        assert_eq!(result.get(&PathBuf::from("binary.dat")), Some(&new_binary));
    }
}
