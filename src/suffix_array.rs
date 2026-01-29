//! Suffix array-based string matching for efficient byte-level diffs
//!
//! Implements suffix array construction with LCP (Longest Common Prefix) arrays
//! for O(m log n) diff computation instead of naive O(n×m).

use std::cmp::min;

/// Suffix array with LCP array for fast substring matching
pub struct SuffixArray {
    /// Sorted suffix positions
    sa: Vec<usize>,
    /// Longest Common Prefix array (lcp[i] = LCP between sa[i] and sa[i+1])
    lcp: Vec<usize>,
    /// Inverse suffix array (position -> rank in sa)
    rank: Vec<usize>,
}

impl SuffixArray {
    /// Build suffix array from text
    ///
    /// Uses simple O(n² log n) algorithm for correctness.
    /// For production, could use SA-IS (O(n)) or DC3 (O(n log n)).
    pub fn new(text: &[u8]) -> Self {
        if text.is_empty() {
            return Self {
                sa: vec![],
                lcp: vec![],
                rank: vec![],
            };
        }

        let sa = Self::build_sa_simple(text);
        let lcp = Self::build_lcp(text, &sa);
        let rank = Self::build_rank(&sa);

        Self { sa, lcp, rank }
    }

    /// Build suffix array using simple comparison-based sorting
    ///
    /// Time: O(n² log n)
    /// Space: O(n)
    fn build_sa_simple(text: &[u8]) -> Vec<usize> {
        let n = text.len();
        let mut sa: Vec<usize> = (0..n).collect();

        // Sort suffixes by their content
        sa.sort_by(|&a, &b| text[a..].cmp(&text[b..]));

        sa
    }

    /// Build LCP array using Kasai's algorithm
    ///
    /// lcp[i] = longest common prefix between suffixes sa[i] and sa[i+1]
    ///
    /// Time: O(n)
    /// Space: O(n)
    fn build_lcp(text: &[u8], sa: &[usize]) -> Vec<usize> {
        let n = text.len();
        if n == 0 {
            return vec![];
        }

        let mut lcp = vec![0; n];
        let rank = Self::build_rank(sa);

        let mut h = 0; // Current LCP length

        for i in 0..n {
            if rank[i] > 0 {
                let j = sa[rank[i] - 1];

                // Compute LCP between suffixes starting at i and j
                while i + h < n && j + h < n && text[i + h] == text[j + h] {
                    h += 1;
                }

                lcp[rank[i] - 1] = h;

                if h > 0 {
                    h -= 1;
                }
            }
        }

        lcp
    }

    /// Build inverse suffix array (position -> rank)
    fn build_rank(sa: &[usize]) -> Vec<usize> {
        let n = sa.len();
        let mut rank = vec![0; n];

        for (i, &pos) in sa.iter().enumerate() {
            rank[pos] = i;
        }

        rank
    }

    /// Find longest match for new_content[new_pos..] in old_content
    ///
    /// Uses binary search on suffix array + LCP optimization.
    ///
    /// Returns (old_pos, match_length)
    pub fn find_longest_match(
        &self,
        old_content: &[u8],
        new_content: &[u8],
        new_pos: usize,
    ) -> (usize, usize) {
        if self.sa.is_empty() || new_pos >= new_content.len() {
            return (0, 0);
        }

        let query = &new_content[new_pos..];

        // Binary search to find first suffix >= query
        let left = self.binary_search_lower_bound(old_content, query);

        // Search nearby suffixes for best match
        let mut best_old_pos = 0;
        let mut best_len = 0;

        // Search window around the binary search result
        const SEARCH_RADIUS: usize = 10;
        let search_start = left.saturating_sub(SEARCH_RADIUS);
        let search_end = min(left + SEARCH_RADIUS, self.sa.len());

        for rank in search_start..search_end {
            let old_pos = self.sa[rank];

            // Check if first byte matches
            if old_pos >= old_content.len() || old_content[old_pos] != new_content[new_pos] {
                continue;
            }

            // Compute match length
            let match_len = self.compute_match_length(
                old_content,
                new_content,
                old_pos,
                new_pos,
                rank,
            );

            if match_len > best_len {
                best_old_pos = old_pos;
                best_len = match_len;
            }

            // Early exit for very good matches
            if best_len >= 128 {
                break;
            }
        }

        (best_old_pos, best_len)
    }

    /// Binary search for first suffix >= query
    fn binary_search_lower_bound(&self, text: &[u8], query: &[u8]) -> usize {
        let mut left = 0;
        let mut right = self.sa.len();

        while left < right {
            let mid = (left + right) / 2;
            let suffix_start = self.sa[mid];

            if suffix_start < text.len() && &text[suffix_start..] < query {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        left
    }

    /// Compute match length using LCP optimization
    ///
    /// If we know LCP between adjacent suffixes, we can skip comparing those bytes.
    fn compute_match_length(
        &self,
        old_content: &[u8],
        new_content: &[u8],
        old_pos: usize,
        new_pos: usize,
        rank: usize,
    ) -> usize {
        // Use LCP as starting point if available
        let min_lcp = if rank > 0 && rank - 1 < self.lcp.len() {
            self.lcp[rank - 1]
        } else {
            0
        };

        // Extend match from min_lcp
        let mut len = min_lcp;

        while old_pos + len < old_content.len()
            && new_pos + len < new_content.len()
            && old_content[old_pos + len] == new_content[new_pos + len]
        {
            len += 1;
        }

        len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suffix_array_construction() {
        let text = b"banana";
        let sa = SuffixArray::new(text);

        // Verify suffix array is sorted
        for i in 1..sa.sa.len() {
            let suffix1 = &text[sa.sa[i - 1]..];
            let suffix2 = &text[sa.sa[i]..];
            assert!(suffix1 <= suffix2, "Suffix array not sorted");
        }
    }

    #[test]
    fn test_suffix_array_empty() {
        let text = b"";
        let sa = SuffixArray::new(text);
        assert_eq!(sa.sa.len(), 0);
        assert_eq!(sa.lcp.len(), 0);
    }

    #[test]
    fn test_find_match_exact() {
        let old = b"hello world";
        let new = b"hello";

        let sa = SuffixArray::new(old);
        let (pos, len) = sa.find_longest_match(old, new, 0);

        assert_eq!(pos, 0);
        assert_eq!(len, 5);
    }

    #[test]
    fn test_find_match_substring() {
        let old = b"the quick brown fox";
        let new = b"quick";

        let sa = SuffixArray::new(old);
        let (pos, len) = sa.find_longest_match(old, new, 0);

        assert_eq!(pos, 4); // "quick" starts at position 4
        assert_eq!(len, 5);
    }

    #[test]
    fn test_find_match_no_match() {
        let old = b"hello";
        let new = b"xyz";

        let sa = SuffixArray::new(old);
        let (pos, len) = sa.find_longest_match(old, new, 0);

        assert_eq!(len, 0); // No match found
    }

    #[test]
    fn test_find_match_repeated_pattern() {
        let old = b"aaaaaaa";
        let new = b"aaaa";

        let sa = SuffixArray::new(old);
        let (pos, len) = sa.find_longest_match(old, new, 0);

        // Should find a match of at least length 4 (may find longer since old has 7 a's)
        assert!(len >= 4);
        assert!(pos + len <= old.len());
    }

    #[test]
    fn test_lcp_array() {
        let text = b"banana";
        let sa = SuffixArray::new(text);

        // Verify LCP values are reasonable
        for i in 0..sa.lcp.len() {
            assert!(sa.lcp[i] <= text.len());

            // LCP should match actual common prefix
            if i + 1 < sa.sa.len() {
                let suffix1 = &text[sa.sa[i]..];
                let suffix2 = &text[sa.sa[i + 1]..];

                let actual_lcp = suffix1
                    .iter()
                    .zip(suffix2.iter())
                    .take_while(|(a, b)| a == b)
                    .count();

                assert_eq!(sa.lcp[i], actual_lcp);
            }
        }
    }

    #[test]
    fn test_binary_search_lower_bound() {
        let text = b"abcdefgh";
        let sa = SuffixArray::new(text);

        // Search for "def"
        let pos = sa.binary_search_lower_bound(text, b"def");

        // Should find position where "defgh" appears in sorted suffixes
        assert!(pos < sa.sa.len());
        let found_suffix = &text[sa.sa[pos]..];
        assert!(found_suffix >= b"def".as_slice());
    }

    #[test]
    fn test_rank_array() {
        let text = b"banana";
        let sa = SuffixArray::new(text);

        // Verify rank is inverse of sa
        for (i, &pos) in sa.sa.iter().enumerate() {
            assert_eq!(sa.rank[pos], i);
        }
    }
}
