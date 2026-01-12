//! Apply patch operations to file tree
//!
//! Transforms file state by applying LineOp operations.
//! Validates operations before applying to ensure atomicity.

use crate::patch::LineOp;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// Apply a sequence of LineOps to a file tree
///
/// Operations are applied atomically - if any operation fails validation,
/// none are applied. This ensures repository state remains consistent.
///
/// Returns the new file tree state after applying all operations.
pub fn apply_operations(
    files: &HashMap<PathBuf, Vec<u8>>,
    operations: &[LineOp],
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    // Clone the current state - we'll modify this
    let mut new_files = files.clone();

    // Phase 1: Validate all operations can be applied
    for op in operations {
        validate_operation(op, &new_files)
            .with_context(|| format!("Failed to validate operation: {:?}", op))?;
    }

    // Phase 2: Apply all operations
    for op in operations {
        apply_single_operation(op, &mut new_files)
            .with_context(|| format!("Failed to apply operation: {:?}", op))?;
    }

    Ok(new_files)
}

/// Validate that an operation can be applied to the current file state
fn validate_operation(op: &LineOp, files: &HashMap<PathBuf, Vec<u8>>) -> Result<()> {
    match op {
        LineOp::InsertLine { file, after, .. } => {
            // File must exist
            let content = files
                .get(file)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", file))?;

            let lines = split_lines(content);

            // 'after' must be a valid position
            // after = 0 means insert before first line (valid for empty file)
            // after = n means insert after line n-1 (must be < lines.len())
            if *after > lines.len() {
                return Err(anyhow!(
                    "Invalid insert position {} in file with {} lines",
                    after,
                    lines.len()
                ));
            }

            Ok(())
        }
        LineOp::DeleteLine { file, at, old_content } => {
            // File must exist
            let content = files
                .get(file)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", file))?;

            let lines = split_lines(content);

            // Line index must be valid
            if *at >= lines.len() {
                return Err(anyhow!(
                    "Invalid delete position {} in file with {} lines",
                    at,
                    lines.len()
                ));
            }

            // Content must match (prevent applying wrong patch)
            if &lines[*at] != old_content {
                return Err(anyhow!(
                    "Line content mismatch at position {} - patch may be stale",
                    at
                ));
            }

            Ok(())
        }
        LineOp::ModifyLine { file, at, old, .. } => {
            // File must exist
            let content = files
                .get(file)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", file))?;

            let lines = split_lines(content);

            // Line index must be valid
            if *at >= lines.len() {
                return Err(anyhow!(
                    "Invalid modify position {} in file with {} lines",
                    at,
                    lines.len()
                ));
            }

            // Old content must match
            if &lines[*at] != old {
                return Err(anyhow!(
                    "Line content mismatch at position {} - patch may be stale",
                    at
                ));
            }

            Ok(())
        }
        LineOp::AddFile { path, .. } => {
            // File must not already exist
            if files.contains_key(path) {
                return Err(anyhow!("File already exists: {:?}", path));
            }
            Ok(())
        }
        LineOp::DeleteFile { path, old_content } => {
            // File must exist
            let content = files
                .get(path)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", path))?;

            // Content must match
            if content != old_content {
                return Err(anyhow!("File content mismatch - patch may be stale: {:?}", path));
            }

            Ok(())
        }
        LineOp::RenameFile { from, to } => {
            // Source must exist
            if !files.contains_key(from) {
                return Err(anyhow!("Source file does not exist: {:?}", from));
            }

            // Destination must not exist
            if files.contains_key(to) {
                return Err(anyhow!("Destination file already exists: {:?}", to));
            }

            Ok(())
        }
    }
}

/// Apply a single operation to the file tree
///
/// Assumes validation has already passed.
fn apply_single_operation(op: &LineOp, files: &mut HashMap<PathBuf, Vec<u8>>) -> Result<()> {
    match op {
        LineOp::InsertLine { file, after, content } => {
            let file_content = files
                .get_mut(file)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", file))?;

            let mut lines = split_lines(file_content);

            // Insert at the correct position
            // after = 0 means insert at beginning (before line 0)
            // after = n means insert after line n-1 (at index n)
            let insert_at = *after;
            lines.insert(insert_at, content.clone());

            // Rejoin lines
            *file_content = lines.concat();
            Ok(())
        }
        LineOp::DeleteLine { file, at, .. } => {
            let file_content = files
                .get_mut(file)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", file))?;

            let mut lines = split_lines(file_content);

            // Remove the line
            lines.remove(*at);

            // Rejoin lines
            *file_content = lines.concat();
            Ok(())
        }
        LineOp::ModifyLine { file, at, new, .. } => {
            let file_content = files
                .get_mut(file)
                .ok_or_else(|| anyhow!("File does not exist: {:?}", file))?;

            let mut lines = split_lines(file_content);

            // Replace the line
            lines[*at] = new.clone();

            // Rejoin lines
            *file_content = lines.concat();
            Ok(())
        }
        LineOp::AddFile { path, content } => {
            files.insert(path.clone(), content.clone());
            Ok(())
        }
        LineOp::DeleteFile { path, .. } => {
            files.remove(path);
            Ok(())
        }
        LineOp::RenameFile { from, to } => {
            let content = files
                .remove(from)
                .ok_or_else(|| anyhow!("Source file does not exist: {:?}", from))?;
            files.insert(to.clone(), content);
            Ok(())
        }
    }
}

/// Split content into lines (preserving line endings)
fn split_lines(content: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut current_line = Vec::new();

    for &byte in content {
        current_line.push(byte);
        if byte == b'\n' {
            lines.push(current_line.clone());
            current_line.clear();
        }
    }

    // Add final line if not empty (no trailing newline)
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // Special case: empty file
    if lines.is_empty() && content.is_empty() {
        return Vec::new();
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_add_file() {
        let files = HashMap::new();
        let ops = vec![LineOp::AddFile {
            path: PathBuf::from("test.txt"),
            content: b"hello\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.get(&PathBuf::from("test.txt")), Some(&b"hello\n".to_vec()));
    }

    #[test]
    fn test_apply_delete_file() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());

        let ops = vec![LineOp::DeleteFile {
            path: PathBuf::from("test.txt"),
            old_content: b"content\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops).unwrap();

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_apply_insert_line() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), b"line1\n".to_vec());

        let ops = vec![LineOp::InsertLine {
            file: PathBuf::from("test.txt"),
            after: 1,
            content: b"line2\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops).unwrap();

        assert_eq!(
            result.get(&PathBuf::from("test.txt")),
            Some(&b"line1\nline2\n".to_vec())
        );
    }

    #[test]
    fn test_apply_delete_line() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), b"line1\nline2\n".to_vec());

        let ops = vec![LineOp::DeleteLine {
            file: PathBuf::from("test.txt"),
            at: 0,
            old_content: b"line1\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops).unwrap();

        assert_eq!(
            result.get(&PathBuf::from("test.txt")),
            Some(&b"line2\n".to_vec())
        );
    }

    #[test]
    fn test_apply_modify_line() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), b"old\n".to_vec());

        let ops = vec![LineOp::ModifyLine {
            file: PathBuf::from("test.txt"),
            at: 0,
            old: b"old\n".to_vec(),
            new: b"new\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops).unwrap();

        assert_eq!(
            result.get(&PathBuf::from("test.txt")),
            Some(&b"new\n".to_vec())
        );
    }

    #[test]
    fn test_validation_fails_missing_file() {
        let files = HashMap::new();
        let ops = vec![LineOp::InsertLine {
            file: PathBuf::from("missing.txt"),
            after: 0,
            content: b"line\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_fails_content_mismatch() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), b"actual\n".to_vec());

        let ops = vec![LineOp::DeleteLine {
            file: PathBuf::from("test.txt"),
            at: 0,
            old_content: b"expected\n".to_vec(),
        }];

        let result = apply_operations(&files, &ops);
        assert!(result.is_err());
    }

    #[test]
    fn test_atomicity_all_or_nothing() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("test.txt"), b"line1\n".to_vec());

        // Second operation will fail validation
        let ops = vec![
            LineOp::AddFile {
                path: PathBuf::from("new.txt"),
                content: b"content\n".to_vec(),
            },
            LineOp::DeleteLine {
                file: PathBuf::from("test.txt"),
                at: 0,
                old_content: b"wrong\n".to_vec(), // Mismatch!
            },
        ];

        let result = apply_operations(&files, &ops);
        assert!(result.is_err());

        // Original files should be unchanged
        assert_eq!(files.get(&PathBuf::from("test.txt")), Some(&b"line1\n".to_vec()));
        assert!(!files.contains_key(&PathBuf::from("new.txt")));
    }

    #[test]
    fn test_apply_rename_file() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("old.txt"), b"content\n".to_vec());

        let ops = vec![LineOp::RenameFile {
            from: PathBuf::from("old.txt"),
            to: PathBuf::from("new.txt"),
        }];

        let result = apply_operations(&files, &ops).unwrap();

        assert!(!result.contains_key(&PathBuf::from("old.txt")));
        assert_eq!(
            result.get(&PathBuf::from("new.txt")),
            Some(&b"content\n".to_vec())
        );
    }
}
