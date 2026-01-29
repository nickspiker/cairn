//! Hash encoding utilities for content-addressed storage
//!
//! Provides base58 encoding for BLAKE3 hashes used in snapshot and patch filenames.

use anyhow::Result;

/// Base58 encode a hash (44 characters for BLAKE3 32-byte hashes)
///
/// Used for content-addressed snapshot and patch filenames
pub fn base58_encode(hash: &[u8; 32]) -> String {
    bs58::encode(hash).into_string()
}

/// Base58 decode a hash string back to 32 bytes
///
/// Returns error if the string is not valid base58 or not 32 bytes
pub fn base58_decode(s: &str) -> Result<[u8; 32]> {
    let bytes = bs58::decode(s).into_vec()?;
    if bytes.len() != 32 {
        anyhow::bail!("Invalid hash length: expected 32 bytes, got {}", bytes.len());
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base58_roundtrip() {
        let hash = *blake3::hash(b"test content").as_bytes();
        let encoded = base58_encode(&hash);

        // Base58 encoding of 32 bytes should be ~44 characters
        assert!(encoded.len() >= 43 && encoded.len() <= 45);

        // Should round-trip correctly
        let decoded = base58_decode(&encoded).unwrap();
        assert_eq!(decoded, hash);
    }

    #[test]
    fn test_base58_different_hashes() {
        let hash1 = *blake3::hash(b"content1").as_bytes();
        let hash2 = *blake3::hash(b"content2").as_bytes();

        let encoded1 = base58_encode(&hash1);
        let encoded2 = base58_encode(&hash2);

        // Different hashes should produce different encodings
        assert_ne!(encoded1, encoded2);
    }

    #[test]
    fn test_base58_decode_invalid_length() {
        // Try to decode a string that doesn't represent 32 bytes
        let result = base58_decode("abc");
        assert!(result.is_err());
    }
}
