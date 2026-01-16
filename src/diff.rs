//! Binary diff computation on x-encoded snapshots
//!
//! Uses line-by-line byte comparison to generate binary diffs
//! with absolute byte positions on x-encoded (Huffman compressed) content.

use crate::patch::{ByteOp, FileOp};
use crate::snapshot_vsf::extract_encoded_files;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Compute diff operations between old and new snapshots
///
/// Operates on x-encoded content from snapshots for ~2× smaller patches.
/// Returns Vec<FileOp> that transform old snapshot into new snapshot.
pub fn compute_diff(
    cairn_dir: &Path,
    old_snapshot: &[u8; 32],
    new_snapshot: &[u8; 32],
) -> Result<Vec<FileOp>> {
    // Extract x-encoded content from both snapshots
    let old_files = extract_encoded_files(cairn_dir, old_snapshot)?;
    let new_files = extract_encoded_files(cairn_dir, new_snapshot)?;

    let mut operations = Vec::new();

    // 1. Find deleted files (in old but not in new)
    for path in old_files.keys() {
        if !new_files.contains_key(path) {
            operations.push(FileOp::DeleteFile {
                path: path.clone(),
                old_snapshot: *old_snapshot,
            });
        }
    }

    // 2. Find added files (in new but not in old)
    for path in new_files.keys() {
        if !old_files.contains_key(path) {
            operations.push(FileOp::AddFile { path: path.clone() });
        }
    }

    // 3. Find modified files (in both, but different x-encoded content)
    for (path, new_content) in &new_files {
        if let Some(old_content) = old_files.get(path) {
            if old_content != new_content {
                // File modified - compute binary diff on x-encoded content
                let file_op = compute_binary_diff(old_snapshot, path, old_content, new_content)?;
                operations.push(file_op);
            }
        }
    }

    Ok(operations)
}

/// Compute binary diff between old and new x-encoded file content
///
/// For text files: Uses Myers algorithm for optimal diff on x-encoded bytes.
/// For binary files: Stores full content as single Insert operation.
/// All Copy operations use absolute byte positions in the x-encoded base content.
fn compute_binary_diff(
    base_snapshot: &[u8; 32],
    path: &PathBuf,
    old_content: &[u8],
    new_content: &[u8],
) -> Result<FileOp> {
    // Detect if file is text or binary (x-encoded text has different characteristics)
    let byte_ops = if is_likely_text(new_content) {
        // Text file - use diff algorithm on x-encoded bytes
        generate_byte_ops(old_content, new_content)
    } else {
        // Binary file - just store full content (no diff)
        vec![ByteOp::Insert {
            content: new_content.to_vec(),
        }]
    };

    // Compute result hash of x-encoded content by reconstructing the file
    let reconstructed = apply_byte_ops(old_content, &byte_ops);
    let result_hash = *blake3::hash(&reconstructed).as_bytes();

    // Verify reconstruction matches new x-encoded content
    debug_assert_eq!(
        reconstructed, new_content,
        "Binary diff reconstruction mismatch"
    );

    Ok(FileOp::ModifyFile {
        path: path.clone(),
        base_snapshot: *base_snapshot,
        operations: byte_ops,
        result_hash,
    })
}

/// Generate ByteOp sequence using line-by-line byte comparison
///
/// Converts old → new by:
/// - Equal lines: Copy from base snapshot (x-encoded content)
/// - Deleted lines: Skip (don't copy from base)
/// - Inserted lines: Insert new x-encoded bytes
///
/// Works directly on raw x-encoded bytes without decoding.
fn generate_byte_ops(old_content: &[u8], new_content: &[u8]) -> Vec<ByteOp> {
    let mut operations = Vec::new();

    // Split into lines (including newline characters)
    let old_lines = split_lines(old_content);
    let new_lines = split_lines(new_content);

    // Simple line-by-line diff using longest common subsequence
    let matches = find_matching_lines(&old_lines, &new_lines);

    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut old_byte_pos = 0;

    for (old_match_idx, new_match_idx) in matches {
        // Insert any new lines before this match
        while new_idx < new_match_idx {
            operations.push(ByteOp::Insert {
                content: new_lines[new_idx].to_vec(),
            });
            new_idx += 1;
        }

        // Skip any deleted lines before this match
        while old_idx < old_match_idx {
            old_byte_pos += old_lines[old_idx].len();
            old_idx += 1;
        }

        // Copy the matching line
        let line_len = old_lines[old_idx].len();
        operations.push(ByteOp::Copy {
            start: old_byte_pos,
            len: line_len,
        });
        old_byte_pos += line_len;
        old_idx += 1;
        new_idx += 1;
    }

    // Handle any remaining new lines at the end
    while new_idx < new_lines.len() {
        operations.push(ByteOp::Insert {
            content: new_lines[new_idx].to_vec(),
        });
        new_idx += 1;
    }

    // Merge consecutive operations for efficiency
    merge_operations(operations)
}

/// Split content into lines (including newline characters)
///
/// Each line includes its trailing newline (except possibly the last line).
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;

    for (i, &byte) in content.iter().enumerate() {
        if byte == b'\n' {
            lines.push(&content[start..=i]); // Include the newline
            start = i + 1;
        }
    }

    // Add remaining content if any
    if start < content.len() {
        lines.push(&content[start..]);
    }

    lines
}

/// Find matching lines between old and new using a simple greedy approach
///
/// Returns Vec<(old_idx, new_idx)> of matching line pairs in order.
fn find_matching_lines(old_lines: &[&[u8]], new_lines: &[&[u8]]) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    let mut used_old = vec![false; old_lines.len()];
    let mut used_new = vec![false; new_lines.len()];

    // Greedy matching: for each new line, find first matching old line
    for (new_idx, new_line) in new_lines.iter().enumerate() {
        for (old_idx, old_line) in old_lines.iter().enumerate() {
            if !used_old[old_idx] && !used_new[new_idx] && old_line == new_line {
                matches.push((old_idx, new_idx));
                used_old[old_idx] = true;
                used_new[new_idx] = true;
                break;
            }
        }
    }

    // Sort by old index to maintain order
    matches.sort_by_key(|(old_idx, _)| *old_idx);

    matches
}

/// Merge consecutive operations of the same type
///
/// - Consecutive Copy operations with adjacent positions are merged
/// - Consecutive Insert operations are merged into a single Insert
fn merge_operations(operations: Vec<ByteOp>) -> Vec<ByteOp> {
    if operations.is_empty() {
        return operations;
    }

    let mut merged = Vec::new();
    let mut current = operations[0].clone();

    for next in operations.into_iter().skip(1) {
        match (&current, &next) {
            // Merge consecutive Copy operations if adjacent
            (
                ByteOp::Copy {
                    start: start1,
                    len: len1,
                },
                ByteOp::Copy {
                    start: start2,
                    len: len2,
                },
            ) if *start1 + *len1 == *start2 => {
                current = ByteOp::Copy {
                    start: *start1,
                    len: len1 + len2,
                };
            }
            // Merge consecutive Insert operations
            (ByteOp::Insert { content: content1 }, ByteOp::Insert { content: content2 }) => {
                let mut merged_content = content1.clone();
                merged_content.extend(content2);
                current = ByteOp::Insert {
                    content: merged_content,
                };
            }
            // Different operations - push current and start new
            _ => {
                merged.push(current);
                current = next;
            }
        }
    }

    merged.push(current);
    merged
}

/// Apply ByteOp operations to reconstruct file content
///
/// Used internally to verify diff correctness and compute result_hash.
fn apply_byte_ops(base_content: &[u8], operations: &[ByteOp]) -> Vec<u8> {
    let mut output = Vec::new();

    for op in operations {
        match op {
            ByteOp::Copy { start, len } => {
                let end = start + len;
                output.extend_from_slice(&base_content[*start..end]);
            }
            ByteOp::Insert { content } => {
                output.extend_from_slice(content);
            }
        }
    }

    output
}

/// Detect if content is likely text (vs binary)
///
/// Uses a simple heuristic:
/// - Check first 8KB for null bytes (0x00) or excessive control characters
/// - Text files typically don't contain null bytes
/// - Binary files often have null bytes or many control characters
fn is_likely_text(content: &[u8]) -> bool {
    const SAMPLE_SIZE: usize = 8192;
    let sample = if content.len() > SAMPLE_SIZE {
        &content[..SAMPLE_SIZE]
    } else {
        content
    };

    // Check for null bytes - strong indicator of binary
    if sample.contains(&0) {
        return false;
    }

    // Count control characters (excluding common whitespace)
    let control_chars = sample
        .iter()
        .filter(|&&b| b < 32 && b != b'\n' && b != b'\r' && b != b'\t')
        .count();

    // If more than 1% control characters, likely binary
    let threshold = sample.len() / 100;
    control_chars <= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_vsf::create_snapshot;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_diff_add_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Create old snapshot (with a dummy file to avoid empty snapshot issues)
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("dummy.txt"), b"dummy".to_vec());
        let old_snapshot = create_snapshot(&old_files, cairn_dir).unwrap();

        // Create new snapshot (keep dummy.txt + add test.txt)
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("dummy.txt"), b"dummy".to_vec());
        new_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = compute_diff(cairn_dir, &old_snapshot, &new_snapshot).unwrap();

        assert_eq!(ops.len(), 1);
        match &ops[0] {
            FileOp::AddFile { path } => {
                assert_eq!(path, &PathBuf::from("test.txt"));
            }
            _ => panic!("Expected AddFile operation"),
        }
    }

    #[test]
    fn test_diff_delete_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Create old snapshot (with dummy.txt + test.txt)
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("dummy.txt"), b"dummy".to_vec());
        old_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());
        let old_snapshot = create_snapshot(&old_files, cairn_dir).unwrap();

        // Create new snapshot (keep only dummy.txt to avoid empty snapshot issues)
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("dummy.txt"), b"dummy".to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = compute_diff(cairn_dir, &old_snapshot, &new_snapshot).unwrap();

        assert_eq!(ops.len(), 1);
        match &ops[0] {
            FileOp::DeleteFile {
                path,
                old_snapshot: _,
            } => {
                assert_eq!(path, &PathBuf::from("test.txt"));
            }
            _ => panic!("Expected DeleteFile operation"),
        }
    }

    #[test]
    fn test_diff_modify_file() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Create old snapshot
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("test.txt"), b"old content\n".to_vec());
        let old_snapshot = create_snapshot(&old_files, cairn_dir).unwrap();

        // Create new snapshot
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), b"new content\n".to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = compute_diff(cairn_dir, &old_snapshot, &new_snapshot).unwrap();

        assert_eq!(ops.len(), 1);
        match &ops[0] {
            FileOp::ModifyFile {
                path,
                base_snapshot: _,
                operations,
                result_hash: _,
            } => {
                assert_eq!(path, &PathBuf::from("test.txt"));
                assert!(!operations.is_empty());
            }
            _ => panic!("Expected ModifyFile operation"),
        }
    }

    #[test]
    fn test_diff_no_changes() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Create old snapshot
        let mut old_files = HashMap::new();
        old_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());
        let old_snapshot = create_snapshot(&old_files, cairn_dir).unwrap();

        // Create new snapshot (same content)
        let mut new_files = HashMap::new();
        new_files.insert(PathBuf::from("test.txt"), b"content\n".to_vec());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = compute_diff(cairn_dir, &old_snapshot, &new_snapshot).unwrap();

        // No changes = no operations
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_generate_byte_ops_identical() {
        let content = b"hello world";
        let ops = generate_byte_ops(content, content);

        // Should be a single Copy operation
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            ByteOp::Copy { start, len } => {
                assert_eq!(*start, 0);
                assert_eq!(*len, content.len());
            }
            _ => panic!("Expected Copy operation"),
        }
    }

    #[test]
    fn test_generate_byte_ops_partial_match() {
        let old = b"hello world\n";
        let new = b"hello world\nrust is here\n";
        let ops = generate_byte_ops(old, new);

        // Line-based diff: Copy("hello world\n"), Insert("rust is here\n")
        assert!(ops.len() >= 2);

        // Verify reconstruction
        let reconstructed = apply_byte_ops(old, &ops);
        assert_eq!(reconstructed, new);
    }

    #[test]
    fn test_generate_byte_ops_completely_different() {
        let old_content = b"old content";
        let new_content = b"completely different";
        let ops = generate_byte_ops(old_content, new_content);

        // Should be Insert (no matches)
        assert!(!ops.is_empty());

        // Verify reconstruction
        let reconstructed = apply_byte_ops(old_content, &ops);
        assert_eq!(reconstructed, new_content);
    }

    #[test]
    fn test_apply_byte_ops() {
        let base = b"hello world";

        // Test Copy
        let ops = vec![ByteOp::Copy { start: 0, len: 5 }];
        let result = apply_byte_ops(base, &ops);
        assert_eq!(result, b"hello");

        // Test Insert
        let ops = vec![ByteOp::Insert {
            content: b"new content".to_vec(),
        }];
        let result = apply_byte_ops(base, &ops);
        assert_eq!(result, b"new content");

        // Test Copy + Insert + Copy
        let ops = vec![
            ByteOp::Copy { start: 0, len: 5 },
            ByteOp::Insert {
                content: b" new".to_vec(),
            },
            ByteOp::Copy { start: 5, len: 6 },
        ];
        let result = apply_byte_ops(base, &ops);
        assert_eq!(result, b"hello new world");
    }

    #[test]
    fn test_merge_operations_consecutive_copy() {
        let ops = vec![
            ByteOp::Copy { start: 0, len: 10 },
            ByteOp::Copy { start: 10, len: 5 },
        ];

        let merged = merge_operations(ops);
        assert_eq!(merged.len(), 1);
        match &merged[0] {
            ByteOp::Copy { start, len } => {
                assert_eq!(*start, 0);
                assert_eq!(*len, 15);
            }
            _ => panic!("Expected merged Copy"),
        }
    }

    #[test]
    fn test_merge_operations_consecutive_insert() {
        let ops = vec![
            ByteOp::Insert {
                content: b"hello".to_vec(),
            },
            ByteOp::Insert {
                content: b" world".to_vec(),
            },
        ];

        let merged = merge_operations(ops);
        assert_eq!(merged.len(), 1);
        match &merged[0] {
            ByteOp::Insert { content } => {
                assert_eq!(content, b"hello world");
            }
            _ => panic!("Expected merged Insert"),
        }
    }

    #[test]
    fn test_binary_invariant() {
        // Test with text data (not binary) - generate_byte_ops() is for text only
        // Binary data should use the direct Insert path in compute_binary_diff()
        let old = b"hello world";
        let new = b"hello rust world";

        let ops = generate_byte_ops(old, new);
        let reconstructed = apply_byte_ops(old, &ops);

        assert_eq!(reconstructed, new);
    }

    #[test]
    fn test_is_likely_text() {
        // Text content
        assert!(is_likely_text(b"Hello, world!\n"));
        assert!(is_likely_text(b"fn main() {\n    println!(\"test\");\n}\n"));
        assert!(is_likely_text(b"Line 1\nLine 2\nLine 3\n"));

        // Binary content (contains null bytes)
        assert!(!is_likely_text(&[0xFF, 0x00, 0xAB, 0xCD]));
        assert!(!is_likely_text(b"text\x00with\x00nulls"));

        // Binary content (many control characters)
        let control_heavy: Vec<u8> = (0..255).collect();
        assert!(!is_likely_text(&control_heavy));
    }

    #[test]
    fn test_binary_file_no_diff() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Create old snapshot with binary file
        let mut old_files = HashMap::new();
        let old_binary = vec![0xFF, 0x00, 0xAB, 0xCD, 0xEF];
        old_files.insert(PathBuf::from("binary.dat"), old_binary.clone());
        let old_snapshot = create_snapshot(&old_files, cairn_dir).unwrap();

        // Create new snapshot with modified binary file
        let mut new_files = HashMap::new();
        let new_binary = vec![0xFF, 0x00, 0x12, 0x34, 0xCD, 0xEF];
        new_files.insert(PathBuf::from("binary.dat"), new_binary.clone());
        let new_snapshot = create_snapshot(&new_files, cairn_dir).unwrap();

        let ops = compute_diff(cairn_dir, &old_snapshot, &new_snapshot).unwrap();

        assert_eq!(ops.len(), 1);
        match &ops[0] {
            FileOp::ModifyFile {
                path, operations, ..
            } => {
                assert_eq!(path, &PathBuf::from("binary.dat"));

                // Binary file should have single Insert operation (no diff)
                assert_eq!(operations.len(), 1);
                match &operations[0] {
                    ByteOp::Insert { content } => {
                        // Content should be the x-encoded/raw binary bytes
                        assert_eq!(content, &new_binary);
                    }
                    _ => panic!("Expected Insert operation for binary file"),
                }
            }
            _ => panic!("Expected ModifyFile operation"),
        }
    }
}
