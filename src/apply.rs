//! Apply patch operations with BLAKE3 verification
//!
//! Reconstructs files from binary diffs and blob storage.
//! Validates BLAKE3 hashes before applying to ensure integrity.

use crate::blob::read_blob;
use crate::patch::{ByteOp, FileOp};
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Apply FileOp operations with BLAKE3 verification
///
/// Operations are validated atomically - all BLAKE3 hashes are verified
/// before applying any changes. This ensures repository state remains consistent.
///
/// Returns the new file tree state after applying all operations.
pub fn apply_operations(
    cairn_dir: &Path,
    files: &HashMap<PathBuf, Vec<u8>>,
    operations: &[FileOp],
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    // Phase 1: Validate all operations and reconstruct all files
    // We do this BEFORE modifying any state to ensure atomicity
    let mut reconstructed_files: HashMap<PathBuf, Vec<u8>> = HashMap::new();

    for op in operations {
        match op {
            FileOp::ModifyFile {
                path,
                base_blob,
                operations: byte_ops,
                result_hash,
            } => {
                // Read base blob from storage
                let base_content = read_blob(cairn_dir, base_blob)
                    .with_context(|| format!("Failed to read base blob for {:?}", path))?;

                // Reconstruct file from ByteOp operations
                let reconstructed = reconstruct_from_ops(&base_content, byte_ops)?;

                // Verify BLAKE3 hash matches expected result
                let computed_hash = *blake3::hash(&reconstructed).as_bytes();
                if &computed_hash != result_hash {
                    return Err(anyhow!(
                        "BLAKE3 hash mismatch for {:?}\nExpected: {}\nComputed: {}",
                        path,
                        hex::encode(result_hash),
                        hex::encode(computed_hash)
                    ));
                }

                reconstructed_files.insert(path.clone(), reconstructed);
            }

            FileOp::AddFile { path, content_blob } => {
                // Read full content from blob storage
                let content = read_blob(cairn_dir, content_blob)
                    .with_context(|| format!("Failed to read content blob for {:?}", path))?;

                // Verify the file doesn't already exist
                if files.contains_key(path) {
                    return Err(anyhow!("File already exists: {:?}", path));
                }

                reconstructed_files.insert(path.clone(), content);
            }

            FileOp::DeleteFile { path, old_blob } => {
                // Verify the file exists and hash matches
                let current_content = files
                    .get(path)
                    .ok_or_else(|| anyhow!("File to delete does not exist: {:?}", path))?;

                let current_hash = *blake3::hash(current_content).as_bytes();
                if &current_hash != old_blob {
                    return Err(anyhow!(
                        "File content mismatch for deletion of {:?}\nExpected: {}\nCurrent: {}",
                        path,
                        hex::encode(old_blob),
                        hex::encode(current_hash)
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
            FileOp::ModifyFile { path, .. } | FileOp::AddFile { path, .. } => {
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
/// All Copy operations reference absolute byte positions in the immutable base blob.
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
    use crate::blob::write_blob;
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

        let content = b"hello world";
        let content_blob = write_blob(cairn_dir, content).unwrap();

        let ops = vec![FileOp::AddFile {
            path: PathBuf::from("test.txt"),
            content_blob,
        }];

        let result = apply_operations(cairn_dir, &files, &ops).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.get(&PathBuf::from("test.txt")), Some(&content.to_vec()));
    }

    #[test]
    fn test_apply_delete_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let content = b"delete me";
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), content.to_vec());

        let old_blob = *blake3::hash(content).as_bytes();

        let ops = vec![FileOp::DeleteFile {
            path: PathBuf::from("test.txt"),
            old_blob,
        }];

        let result = apply_operations(cairn_dir, &files, &ops).unwrap();

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_apply_modify_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let old_content = b"hello world";
        let base_blob = write_blob(cairn_dir, old_content).unwrap();

        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), old_content.to_vec());

        // Modify to "hello rust world"
        let byte_ops = vec![
            ByteOp::Copy { start: 0, len: 6 },
            ByteOp::Insert {
                content: b"rust ".to_vec(),
            },
            ByteOp::Copy { start: 6, len: 5 },
        ];

        let new_content = b"hello rust world";
        let result_hash = *blake3::hash(new_content).as_bytes();

        let ops = vec![FileOp::ModifyFile {
            path: PathBuf::from("test.txt"),
            base_blob,
            operations: byte_ops,
            result_hash,
        }];

        let result = apply_operations(cairn_dir, &files, &ops).unwrap();

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

        let ops = vec![FileOp::RenameFile {
            from: PathBuf::from("old.txt"),
            to: PathBuf::from("new.txt"),
        }];

        let result = apply_operations(cairn_dir, &files, &ops).unwrap();

        assert!(!result.contains_key(&PathBuf::from("old.txt")));
        assert_eq!(result.get(&PathBuf::from("new.txt")), Some(&b"content".to_vec()));
    }

    #[test]
    fn test_hash_mismatch_fails() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let old_content = b"hello";
        let base_blob = write_blob(cairn_dir, old_content).unwrap();

        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), old_content.to_vec());

        let byte_ops = vec![ByteOp::Insert {
            content: b"wrong".to_vec(),
        }];

        // Provide WRONG result hash (hash of "correct" instead of "wrong")
        let result_hash = *blake3::hash(b"correct").as_bytes();

        let ops = vec![FileOp::ModifyFile {
            path: PathBuf::from("test.txt"),
            base_blob,
            operations: byte_ops,
            result_hash,
        }];

        let result = apply_operations(cairn_dir, &files, &ops);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("BLAKE3 hash mismatch"));
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

        let base_blob1 = write_blob(cairn_dir, content1).unwrap();
        let base_blob2 = write_blob(cairn_dir, content2).unwrap();

        // First op is valid, second op has wrong hash
        let ops = vec![
            FileOp::ModifyFile {
                path: PathBuf::from("file1.txt"),
                base_blob: base_blob1,
                operations: vec![ByteOp::Insert {
                    content: b"valid".to_vec(),
                }],
                result_hash: *blake3::hash(b"valid").as_bytes(),
            },
            FileOp::ModifyFile {
                path: PathBuf::from("file2.txt"),
                base_blob: base_blob2,
                operations: vec![ByteOp::Insert {
                    content: b"wrong".to_vec(),
                }],
                result_hash: *blake3::hash(b"not_wrong").as_bytes(), // Wrong!
            },
        ];

        let result = apply_operations(cairn_dir, &files, &ops);

        assert!(result.is_err());

        // Original files should be unchanged (atomicity)
        assert_eq!(files.get(&PathBuf::from("file1.txt")), Some(&content1.to_vec()));
        assert_eq!(files.get(&PathBuf::from("file2.txt")), Some(&content2.to_vec()));
    }

    #[test]
    fn test_binary_data_apply() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Binary data (not text)
        let old_binary = vec![0xFF, 0x00, 0xAB, 0xCD, 0xEF];
        let base_blob = write_blob(cairn_dir, &old_binary).unwrap();

        let mut files = HashMap::new();
        files.insert(PathBuf::from("binary.dat"), old_binary.clone());

        let byte_ops = vec![
            ByteOp::Copy { start: 0, len: 2 },
            ByteOp::Insert {
                content: vec![0x12, 0x34],
            },
            ByteOp::Copy { start: 3, len: 2 },
        ];

        let new_binary = vec![0xFF, 0x00, 0x12, 0x34, 0xCD, 0xEF];
        let result_hash = *blake3::hash(&new_binary).as_bytes();

        let ops = vec![FileOp::ModifyFile {
            path: PathBuf::from("binary.dat"),
            base_blob,
            operations: byte_ops,
            result_hash,
        }];

        let result = apply_operations(cairn_dir, &files, &ops).unwrap();

        assert_eq!(result.get(&PathBuf::from("binary.dat")), Some(&new_binary));
    }
}
