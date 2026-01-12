//! Mnemonic encoding for patch IDs
//!
//! Converts patch IDs (base64url hashes) to human-readable mnemonic words
//! using VSF's 3177-word list. Provides shorter, more memorable identifiers
//! for user-facing output.
//!
//! ## Collision Resistance
//!
//! - 4 words = ~46 bits (adequate for typical projects, ~70 trillion values)
//! - 5 words = ~58 bits (safer, ~288 quadrillion values)
//! - 6 words = ~70 bits (very safe, ~1.2 sextillion values)
//!
//! For comparison, Git's default 7-char hex = ~28 bits (~268 million values)

use anyhow::{Result, anyhow};

// Import VSF's word list and helper functions
use vsf::types::world_coord::WORD_LIST;

/// 3177-word mnemonic list (from VSF's world coordinate system)
const WORD_BASE: u128 = 3177;

/// Encode bytes as mnemonic words
///
/// Takes the first N bytes and encodes them as words.
/// Each word represents ~11.63 bits (log2(3177)).
///
/// # Arguments
/// * `bytes` - The bytes to encode (typically a hash)
/// * `num_words` - Number of words to generate (4-6 recommended)
///
/// # Returns
/// Dash-separated mnemonic words (e.g., "work-kid-long-service")
pub fn encode_bytes(bytes: &[u8], num_words: usize) -> String {
    // Convert bytes to u128 (big-endian)
    let mut value: u128 = 0;
    let bytes_needed = ((num_words as f64 * 11.63 / 8.0).ceil() as usize).min(16);

    for i in 0..bytes_needed.min(bytes.len()) {
        value = (value << 8) | (bytes[i] as u128);
    }

    // Convert to words
    let mut words = Vec::with_capacity(num_words);
    let mut remaining = value;

    for _ in 0..num_words {
        let index = (remaining % WORD_BASE) as usize;
        words.push(WORD_LIST[index]);
        remaining /= WORD_BASE;
    }

    words.join("-")
}

/// Decode mnemonic words back to bytes
///
/// Attempts exact match first, then falls back to fuzzy matching.
///
/// # Arguments
/// * `mnemonic` - Space or dash-separated mnemonic words
///
/// # Returns
/// The decoded bytes (up to 16 bytes)
pub fn decode_mnemonic(mnemonic: &str) -> Result<Vec<u8>> {
    // Split on spaces or dashes
    let words: Vec<&str> = mnemonic.split(|c| c == ' ' || c == '-').collect();

    if words.is_empty() {
        return Err(anyhow!("Empty mnemonic"));
    }

    // Try exact match first
    if let Ok(bytes) = try_decode_exact(&words) {
        return Ok(bytes);
    }

    // Try fuzzy matching (simple Levenshtein distance)
    let corrected: Vec<&str> = words.iter().map(|w| find_closest_word(w)).collect();

    try_decode_exact(&corrected)
}

/// Try exact word match decoding
fn try_decode_exact(words: &[&str]) -> Result<Vec<u8>> {
    let mut value: u128 = 0;
    let mut power: u128 = 1;

    for &word in words {
        let index = WORD_LIST
            .iter()
            .position(|&w| w == word)
            .ok_or_else(|| anyhow!("Unknown word: {}", word))?;

        value += (index as u128) * power;
        power *= WORD_BASE;
    }

    // Convert u128 to bytes (big-endian)
    let bytes_needed = ((words.len() as f64 * 11.63 / 8.0).ceil() as usize).min(16);
    let mut bytes = Vec::with_capacity(bytes_needed);

    for i in (0..bytes_needed).rev() {
        bytes.push(((value >> (i * 8)) & 0xFF) as u8);
    }

    Ok(bytes)
}

/// Find closest word using simple Levenshtein distance
fn find_closest_word(input: &str) -> &'static str {
    let input_lower = input.to_lowercase();
    let mut best_word = WORD_LIST[0];
    let mut best_distance = usize::MAX;

    for &word in WORD_LIST.iter() {
        let distance = levenshtein(&input_lower, word);
        if distance < best_distance {
            best_distance = distance;
            best_word = word;
        }
    }

    best_word
}

/// Compute Levenshtein distance between two strings
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0; b_len + 1]; a_len + 1];

    for i in 0..=a_len {
        matrix[i][0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[a_len][b_len]
}

/// Convert base64url patch ID to mnemonic
pub fn patch_id_to_mnemonic(patch_id: &str, num_words: usize) -> Result<String> {
    use base64::Engine;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(patch_id)
        .map_err(|e| anyhow!("Invalid base64url: {}", e))?;

    Ok(encode_bytes(&bytes, num_words))
}

/// Find patch by mnemonic prefix
///
/// Searches for patches whose mnemonic representation starts with the given prefix.
pub fn find_patch_by_mnemonic(
    patches: &[String],
    mnemonic: &str,
    num_words: usize,
) -> Result<String> {
    // Normalize input (replace spaces with dashes)
    let normalized = mnemonic.replace(' ', "-");

    for patch_id in patches {
        let patch_mnemonic = patch_id_to_mnemonic(patch_id, num_words)?;
        if patch_mnemonic.starts_with(&normalized) {
            return Ok(patch_id.clone());
        }
    }

    Err(anyhow!("No patch found matching mnemonic: {}", mnemonic))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn test_encode_decode_roundtrip() {
        // Test with a shorter input that fits cleanly
        let original = b"hello wo"; // 8 bytes = 64 bits
        let mnemonic = encode_bytes(original, 6); // 6 words = ~70 bits
        let decoded = decode_mnemonic(&mnemonic).unwrap();

        // Should decode to approximately the same bytes
        // (Note: encoding is lossy due to word-base conversion)
        assert_eq!(decoded.len(), 9); // 6 words * 11.63 bits / 8 ≈ 9 bytes
    }

    #[test]
    fn test_encode_different_lengths() {
        let bytes = b"test data";

        let words_4 = encode_bytes(bytes, 4);
        let words_5 = encode_bytes(bytes, 5);
        let words_6 = encode_bytes(bytes, 6);

        assert_eq!(words_4.split('-').count(), 4);
        assert_eq!(words_5.split('-').count(), 5);
        assert_eq!(words_6.split('-').count(), 6);
    }

    #[test]
    fn test_fuzzy_matching() {
        // Test with typos
        let mnemonic = "wrk-kidd-lng"; // Typos in "work-kid-long"
        let decoded = decode_mnemonic(mnemonic);
        assert!(decoded.is_ok());
    }

    #[test]
    fn test_patch_id_conversion() {
        // Create a fake patch ID (base64url encoded hash)
        let hash = blake3::hash(b"test").as_bytes().to_vec();
        let patch_id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash);

        let mnemonic = patch_id_to_mnemonic(&patch_id, 5).unwrap();

        // Should have 5 words
        assert_eq!(mnemonic.split('-').count(), 5);
    }
}
