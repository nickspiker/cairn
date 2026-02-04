//! Smart binary diff computation
//!
//! Generates efficient ByteOp sequences (Copy/Insert) to transform old → new content.
//! Uses suffix array-based matching for O(m log n) complexity on larger files.

use crate::patch::ByteOp;
use crate::suffix_array::SuffixArray;

/// Compute byte-level diff between two raw byte sequences
///
/// Generates optimal Copy/Insert operations to transform old → new.
/// Works on RAW bytes for maximum generality.
///
/// Algorithm: Suffix array-based matching for O(m log n) instead of O(n×m)
/// - Builds suffix array with LCP (Longest Common Prefix) array
/// - Uses binary search + LCP optimization for fast matching
/// - Falls back to naive algorithm for small files (< 64 bytes)
///
/// Edge cases handled:
/// - Empty files (old or new)
/// - Binary data (works on raw bytes)
/// - Large files (suffix array optimization)
pub fn compute_byte_level_diff(old_content: &[u8], new_content: &[u8]) -> Vec<ByteOp> {
    // Handle empty cases
    if old_content.is_empty() {
        return if new_content.is_empty() {
            vec![]
        } else {
            vec![ByteOp::Insert {
                content: new_content.to_vec(),
            }]
        };
    }

    if new_content.is_empty() {
        // Deletion - represented by absence of Copy
        return vec![];
    }

    // Use suffix array for larger files, naive for small files
    const SA_THRESHOLD: usize = 64;

    if old_content.len() < SA_THRESHOLD {
        compute_diff_naive(old_content, new_content)
    } else {
        compute_diff_with_suffix_array(old_content, new_content)
    }
}

/// Compute diff using suffix array (optimized for larger files)
fn compute_diff_with_suffix_array(old_content: &[u8], new_content: &[u8]) -> Vec<ByteOp> {
    // Build suffix array once for old_content
    let sa = SuffixArray::new(old_content);

    let mut operations = Vec::new();
    let mut new_pos = 0;

    while new_pos < new_content.len() {
        // Find longest match using suffix array
        let (match_old_pos, match_len) = sa.find_longest_match(old_content, new_content, new_pos);

        const MIN_MATCH: usize = 4;

        if match_len >= MIN_MATCH {
            // Found a good match - use Copy operation
            operations.push(ByteOp::Copy {
                start: match_old_pos,
                len: match_len,
            });
            new_pos += match_len;
        } else {
            // No good match - collect bytes to Insert until we find a match
            let insert_start = new_pos;
            let mut insert_len = 1;

            // Look ahead to find where next match starts
            while new_pos + insert_len < new_content.len() {
                let (_, peek_len) = sa.find_longest_match(old_content, new_content, new_pos + insert_len);
                if peek_len >= MIN_MATCH {
                    // Found a decent match ahead, stop inserting
                    break;
                }
                insert_len += 1;
            }

            operations.push(ByteOp::Insert {
                content: new_content[insert_start..insert_start + insert_len].to_vec(),
            });
            new_pos += insert_len;
        }
    }

    merge_operations(operations)
}

/// Compute diff using naive algorithm (for small files)
fn compute_diff_naive(old_content: &[u8], new_content: &[u8]) -> Vec<ByteOp> {
    let mut operations = Vec::new();
    let mut new_pos = 0;

    while new_pos < new_content.len() {
        // Find longest match starting from new_pos
        let (match_old_pos, match_len) = find_longest_match_naive(old_content, new_content, new_pos);

        const MIN_MATCH: usize = 4;

        if match_len >= MIN_MATCH {
            // Found a match - use Copy operation
            operations.push(ByteOp::Copy {
                start: match_old_pos,
                len: match_len,
            });
            new_pos += match_len;
        } else {
            // No match - collect bytes to Insert until we find a match
            let insert_start = new_pos;
            let mut insert_len = 1;

            // Look ahead to find where next match starts
            while new_pos + insert_len < new_content.len() {
                let (_, peek_len) = find_longest_match_naive(old_content, new_content, new_pos + insert_len);
                if peek_len >= MIN_MATCH {
                    // Found a decent match ahead, stop inserting
                    break;
                }
                insert_len += 1;
            }

            operations.push(ByteOp::Insert {
                content: new_content[insert_start..insert_start + insert_len].to_vec(),
            });
            new_pos += insert_len;
        }
    }

    merge_operations(operations)
}

/// Find longest common substring starting at new_pos (naive algorithm)
///
/// Returns (old_position, length) of best match.
/// Used as fallback for small files where suffix array overhead isn't worth it.
fn find_longest_match_naive(old_content: &[u8], new_content: &[u8], new_pos: usize) -> (usize, usize) {
    const MIN_MATCH_LEN: usize = 4; // Minimum worthwhile match
    const MAX_SEARCH_WINDOW: usize = 1024; // Limit search for large files

    let mut best_old_pos = 0;
    let mut best_len = 0;

    // Determine search window
    let search_end = old_content.len().min(new_pos + MAX_SEARCH_WINDOW);

    // Search for matches in old content
    for old_pos in 0..search_end {
        let mut match_len = 0;

        // Extend match as far as possible
        while old_pos + match_len < old_content.len()
            && new_pos + match_len < new_content.len()
            && old_content[old_pos + match_len] == new_content[new_pos + match_len]
        {
            match_len += 1;
        }

        // Update best match if this is longer
        if match_len > best_len {
            best_old_pos = old_pos;
            best_len = match_len;
        }

        // Early exit if we found a very good match
        if best_len > 128 {
            break;
        }
    }

    // Only return matches above minimum threshold
    if best_len >= MIN_MATCH_LEN {
        (best_old_pos, best_len)
    } else {
        (0, 0)
    }
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
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_level_diff_identical() {
        let content = b"hello world";
        let ops = compute_byte_level_diff(content, content);

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
    fn test_byte_level_diff_empty_old() {
        let old = b"";
        let new = b"hello";
        let ops = compute_byte_level_diff(old, new);

        // Should be a single Insert operation
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            ByteOp::Insert { content } => {
                assert_eq!(content, b"hello");
            }
            _ => panic!("Expected Insert operation"),
        }
    }

    #[test]
    fn test_byte_level_diff_empty_new() {
        let old = b"hello";
        let new = b"";
        let ops = compute_byte_level_diff(old, new);

        // Deletion - empty operations
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_byte_level_diff_partial_match() {
        let old = b"hello world";
        let new = b"hello rust world";

        let ops = compute_byte_level_diff(old, new);

        // Should use Copy/Insert efficiently
        // Verify reconstruction works
        let reconstructed = apply_byte_ops(old, &ops);
        assert_eq!(reconstructed, new);
    }

    #[test]
    fn test_byte_level_diff_single_byte_change() {
        let old = b"hello world";
        let new = b"hello World"; // Capital W

        let ops = compute_byte_level_diff(old, new);

        // Should be efficient: Copy("hello ") + Insert("W") + Copy("orld")
        // or similar smart diffing
        let reconstructed = apply_byte_ops(old, &ops);
        assert_eq!(reconstructed, new);
    }

    #[test]
    fn test_byte_level_diff_completely_different() {
        let old = b"old content";
        let new = b"completely different";

        let ops = compute_byte_level_diff(old, new);

        // Should be Insert (no matches)
        assert!(!ops.is_empty());

        // Verify reconstruction
        let reconstructed = apply_byte_ops(old, &ops);
        assert_eq!(reconstructed, new);
    }

    #[test]
    fn test_apply_byte_ops_copy() {
        let base = b"hello world";

        let ops = vec![ByteOp::Copy { start: 0, len: 5 }];
        let result = apply_byte_ops(base, &ops);
        assert_eq!(result, b"hello");
    }

    #[test]
    fn test_apply_byte_ops_insert() {
        let base = b"hello world";

        let ops = vec![ByteOp::Insert {
            content: b"new content".to_vec(),
        }];
        let result = apply_byte_ops(base, &ops);
        assert_eq!(result, b"new content");
    }

    #[test]
    fn test_apply_byte_ops_mixed() {
        let base = b"hello world";

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
    fn test_merge_operations_non_adjacent_copy() {
        let ops = vec![
            ByteOp::Copy { start: 0, len: 10 },
            ByteOp::Copy { start: 20, len: 5 }, // Not adjacent
        ];

        let merged = merge_operations(ops);
        assert_eq!(merged.len(), 2); // Should not merge
    }

    #[test]
    fn test_diff_reconstruction_invariant() {
        // Property test: diff then reconstruct should equal new content
        let test_cases = vec![
            (b"hello world".as_slice(), b"hello rust world".as_slice()),
            (b"abc".as_slice(), b"def".as_slice()),
            (b"".as_slice(), b"new".as_slice()),
            (b"old".as_slice(), b"".as_slice()),
            (b"same".as_slice(), b"same".as_slice()),
        ];

        for (old, new) in test_cases {
            let ops = compute_byte_level_diff(old, new);
            let reconstructed = apply_byte_ops(old, &ops);
            assert_eq!(
                reconstructed, new,
                "Reconstruction failed for old={:?}, new={:?}",
                String::from_utf8_lossy(old),
                String::from_utf8_lossy(new)
            );
        }
    }
}
