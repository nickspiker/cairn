//! Diff computation for generating patch operations
//!
//! Computes LineOps between two file tree states.
//! Week 1: Simple line-based diff using Myers algorithm.

use crate::patch::LineOp;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

/// Compute diff operations between old and new file states
///
/// Returns a Vec of LineOp that transform old_files into new_files.
/// Operations are ordered: deletions, modifications, insertions, file ops.
pub fn compute_diff(
    old_files: &HashMap<PathBuf, Vec<u8>>,
    new_files: &HashMap<PathBuf, Vec<u8>>,
) -> Result<Vec<LineOp>> {
    let mut operations = Vec::new();

    // 1. Find deleted files (in old but not in new)
    for (path, old_content) in old_files {
        if !new_files.contains_key(path) {
            operations.push(LineOp::DeleteFile {
                path: path.clone(),
                old_content: old_content.clone(),
            });
        }
    }

    // 2. Find added files (in new but not in old)
    for (path, new_content) in new_files {
        if !old_files.contains_key(path) {
            operations.push(LineOp::AddFile {
                path: path.clone(),
                content: new_content.clone(),
            });
        }
    }

    // 3. Find modified files (in both, but different content)
    for (path, new_content) in new_files {
        if let Some(old_content) = old_files.get(path) {
            if old_content != new_content {
                // File modified - compute line-level diff
                let line_ops = compute_line_diff(path, old_content, new_content)?;
                operations.extend(line_ops);
            }
        }
    }

    Ok(operations)
}

/// Compute line-level diff between old and new file content
///
/// Week 1: Simple line-by-line comparison (not Myers algorithm yet).
/// Returns operations to transform old_lines into new_lines.
fn compute_line_diff(
    path: &PathBuf,
    old_content: &[u8],
    new_content: &[u8],
) -> Result<Vec<LineOp>> {
    let old_lines = split_lines(old_content);
    let new_lines = split_lines(new_content);

    // Simple approach for Week 1: delete all old lines, insert all new lines
    // This is inefficient but correct. Myers algorithm can be added later.
    let mut operations = Vec::new();

    // Delete old lines (in reverse order to maintain indices)
    for (idx, line) in old_lines.iter().enumerate().rev() {
        operations.push(LineOp::DeleteLine {
            file: path.clone(),
            at: idx,
            old_content: line.clone(),
        });
    }

    // Insert new lines
    for (idx, line) in new_lines.iter().enumerate() {
        operations.push(LineOp::InsertLine {
            file: path.clone(),
            after: if idx == 0 { 0 } else { idx - 1 },
            content: line.clone(),
        });
    }

    Ok(operations)
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
    fn test_split_lines_empty() {
        let content = b"";
        let lines = split_lines(content);
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_split_lines_single() {
        let content = b"hello\n";
        let lines = split_lines(content);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], b"hello\n");
    }

    #[test]
    fn test_split_lines_multiple() {
        let content = b"line1\nline2\nline3\n";
        let lines = split_lines(content);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], b"line1\n");
        assert_eq!(lines[1], b"line2\n");
        assert_eq!(lines[2], b"line3\n");
    }

    #[test]
    fn test_split_lines_no_trailing_newline() {
        let content = b"line1\nline2";
        let lines = split_lines(content);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], b"line1\n");
        assert_eq!(lines[1], b"line2");
    }

    #[test]
    fn test_diff_add_file() {
        let old_files = HashMap::new();
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());

        let ops = compute_diff(&old_files, &new_files).unwrap();

        assert_eq!(ops.len(), 1);
        match &ops[0] {
            LineOp::AddFile { path, content } => {
                assert_eq!(path, &PathBuf::from("test.txt"));
                assert_eq!(content, b"content\n");
            }
            _ => panic!("Expected AddFile operation"),
        }
    }

    #[test]
    fn test_diff_delete_file() {
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());
        let new_files = HashMap::new();

        let ops = compute_diff(&old_files, &new_files).unwrap();

        assert_eq!(ops.len(), 1);
        match &ops[0] {
            LineOp::DeleteFile { path, old_content } => {
                assert_eq!(path, &PathBuf::from("test.txt"));
                assert_eq!(old_content, b"content\n");
            }
            _ => panic!("Expected DeleteFile operation"),
        }
    }

    #[test]
    fn test_diff_modify_file() {
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("test.txt"), b"old\n".to_vec());

        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), b"new\n".to_vec());

        let ops = compute_diff(&old_files, &new_files).unwrap();

        // Should have delete + insert operations
        assert!(ops.len() >= 2);

        // First operation should be delete
        match &ops[0] {
            LineOp::DeleteLine { file, at, old_content } => {
                assert_eq!(file, &PathBuf::from("test.txt"));
                assert_eq!(*at, 0);
                assert_eq!(old_content, b"old\n");
            }
            _ => panic!("Expected DeleteLine operation"),
        }
    }

    #[test]
    fn test_diff_no_changes() {
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());

        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());

        let ops = compute_diff(&old_files, &new_files).unwrap();

        // No changes = no operations
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_diff_multiple_files() {
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("a.txt"), b"old a\n".to_vec());
        old_files.insert(PathBuf::from("b.txt"), b"keep b\n".to_vec());

        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("b.txt"), b"keep b\n".to_vec());
        new_files.insert(PathBuf::from("c.txt"), b"new c\n".to_vec());

        let ops = compute_diff(&old_files, &new_files).unwrap();

        // Should have: delete a.txt, add c.txt
        let delete_count = ops.iter().filter(|op| matches!(op, LineOp::DeleteFile { .. })).count();
        let add_count = ops.iter().filter(|op| matches!(op, LineOp::AddFile { .. })).count();

        assert_eq!(delete_count, 1);
        assert_eq!(add_count, 1);
    }
}
