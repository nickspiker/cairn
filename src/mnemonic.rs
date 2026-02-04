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

/// Convert base64url patch ID to mnemonic
pub fn patch_id_to_mnemonic(patch_id: &str, num_words: usize) -> Result<String> {
    use base64::Engine;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(patch_id)
        .map_err(|e| anyhow!("Invalid base64url: {}", e))?;

    Ok(encode_bytes(&bytes, num_words))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

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
    fn test_patch_id_conversion() {
        // Create a fake patch ID (base64url encoded hash)
        let hash = blake3::hash(b"test").as_bytes().to_vec();
        let patch_id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash);

        let mnemonic = patch_id_to_mnemonic(&patch_id, 5).unwrap();

        // Should have 5 words
        assert_eq!(mnemonic.split('-').count(), 5);
    }
}
