//! Blob storage - content-addressed file storage with automatic deduplication
//!
//! Blobs are raw file contents stored once and identified by their BLAKE3 hash (hb).
//! This enables MEGA-style deduplication: same content → same hash → stored once.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::hash_encoding::base58_encode;
use crate::state::Blake3Hash;

/// Store raw file content as a blob
///
/// Returns the content hash (hb) which is used as the blob's identifier.
/// If a blob with the same content already exists, it won't be written again (deduplication).
///
/// # Arguments
/// * `content` - Raw bytes of the file to store
///
/// # Returns
/// * `Blake3Hash` - The content hash (hb) of the stored blob
pub fn store_blob(content: &[u8]) -> Result<Blake3Hash> {
    let hb = blake3::hash(content);
    let hb_array = *hb.as_bytes();

    // Create .cairn/blobs/ if it doesn't exist
    let blobs_dir = PathBuf::from(".cairn/blobs");
    fs::create_dir_all(&blobs_dir).context("Failed to create blobs directory")?;

    let blob_path = blobs_dir.join(base58_encode(&hb_array));

    // Only write if doesn't exist (automatic deduplication)
    if !blob_path.exists() {
        fs::write(&blob_path, content)
            .with_context(|| format!("Failed to write blob {}", base58_encode(&hb_array)))?;
        println!("  Stored blob: {} ({} bytes)", base58_encode(&hb_array), content.len());
    }

    Ok(hb_array)
}

/// Load a blob by its content hash
///
/// # Arguments
/// * `cairn_dir` - Path to .cairn directory
/// * `hb` - Content hash (BLAKE3) of the blob to load
///
/// # Returns
/// * `Vec<u8>` - Raw bytes of the blob content
pub fn load_blob(cairn_dir: &Path, hb: &Blake3Hash) -> Result<Vec<u8>> {
    let blob_path = cairn_dir.join("blobs").join(base58_encode(hb));

    fs::read(&blob_path)
        .with_context(|| format!("Failed to load blob {}", base58_encode(hb)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::sync::Mutex;

    // Use a mutex to ensure tests run sequentially (they modify global state via cwd)
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_store_and_load_blob() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            let content = b"Hello, blob storage!";

            // Store blob
            let hb = store_blob(content)?;

            // Load blob
            let cairn_dir = PathBuf::from(".cairn");
            let loaded = load_blob(&cairn_dir, &hb)?;

            assert_eq!(content, &loaded[..]);
            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_blob_deduplication() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            let content = b"Duplicate content test";

            // Store same content twice
            let hb1 = store_blob(content)?;
            let hb2 = store_blob(content)?;

            // Should produce same hash
            assert_eq!(hb1, hb2);

            // Should only have one blob file
            let blobs_dir = PathBuf::from(".cairn/blobs");
            let blob_count = fs::read_dir(&blobs_dir)?.count();
            assert_eq!(blob_count, 1);

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

}
