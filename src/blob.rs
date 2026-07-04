//! Blob storage - content-addressed file storage with automatic deduplication
//!
//! Blobs are raw file contents stored once in the vault and identified by their BLAKE3
//! hash (hb). Same content → same hash → same vault key → stored once. The dedup
//! check-then-put is race-free because the vault handle holds the repository lock.

use anyhow::{Context, Result};

use crate::state::Blake3Hash;
use crate::vault::{CairnVault, blob_key};

/// Store raw file content as a blob
///
/// Returns the content hash (hb) which is used as the blob's identifier.
/// If a blob with the same content already exists, it won't be written again (deduplication).
pub fn store_blob(vault: &mut CairnVault, content: &[u8]) -> Result<Blake3Hash> {
    let hb = *blake3::hash(content).as_bytes();
    vault
        .put_if_absent(&blob_key(&hb), content)
        .context("Failed to store blob")?;
    Ok(hb)
}

/// Load a blob by its content hash
pub fn load_blob(vault: &mut CairnVault, hb: &Blake3Hash) -> Result<Vec<u8>> {
    vault
        .get(&blob_key(hb))?
        .ok_or_else(|| anyhow::anyhow!("Blob not found: {}", crate::hash_encoding::base58_encode(hb)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_store_and_load_blob() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let mut vault = CairnVault::open(&temp_dir.path().join(".cairn"))?;

        let content = b"Hello, blob storage!";
        let hb = store_blob(&mut vault, content)?;
        let loaded = load_blob(&mut vault, &hb)?;

        assert_eq!(content, &loaded[..]);
        Ok(())
    }

    #[test]
    fn test_blob_deduplication() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let mut vault = CairnVault::open(&temp_dir.path().join(".cairn"))?;

        let content = b"Duplicate content test";
        let hb1 = store_blob(&mut vault, content)?;
        let hb2 = store_blob(&mut vault, content)?;

        assert_eq!(hb1, hb2);
        assert_eq!(load_blob(&mut vault, &hb1)?, content.to_vec());
        Ok(())
    }
}
