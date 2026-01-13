use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Write content to blob storage and return its BLAKE3 hash (provenance)
///
/// Blobs are stored in .cairn/blobs/{hex_hash} and are content-addressed.
/// If a blob with the same hash already exists, it's not rewritten (deduplication).
pub fn write_blob(cairn_dir: &Path, content: &[u8]) -> Result<[u8; 32]> {
    // Compute BLAKE3 hash (provenance)
    let hash = blake3::hash(content);
    let hash_bytes: [u8; 32] = *hash.as_bytes();

    // Create blobs directory if it doesn't exist
    let blobs_dir = cairn_dir.join("blobs");
    fs::create_dir_all(&blobs_dir)
        .context("Failed to create blobs directory")?;

    // Blob path: .cairn/blobs/{hex_hash}
    let blob_path = blobs_dir.join(hex::encode(hash_bytes));

    // Only write if blob doesn't already exist (deduplication)
    if !blob_path.exists() {
        fs::write(&blob_path, content)
            .with_context(|| format!("Failed to write blob {}", hex::encode(hash_bytes)))?;
    }

    Ok(hash_bytes)
}

/// Read blob content from storage by its BLAKE3 hash
///
/// Returns an error if the blob doesn't exist or can't be read.
pub fn read_blob(cairn_dir: &Path, hash: &[u8; 32]) -> Result<Vec<u8>> {
    let blob_path = blob_path(cairn_dir, hash);

    fs::read(&blob_path)
        .with_context(|| format!("Failed to read blob {}", hex::encode(hash)))
}

/// Check if a blob exists in storage
pub fn blob_exists(cairn_dir: &Path, hash: &[u8; 32]) -> bool {
    blob_path(cairn_dir, hash).exists()
}

/// Get the filesystem path for a blob
fn blob_path(cairn_dir: &Path, hash: &[u8; 32]) -> PathBuf {
    cairn_dir.join("blobs").join(hex::encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_blob_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Write a blob
        let content = b"Hello, cairn!";
        let hash = write_blob(cairn_dir, content).unwrap();

        // Verify it exists
        assert!(blob_exists(cairn_dir, &hash));

        // Read it back
        let read_content = read_blob(cairn_dir, &hash).unwrap();
        assert_eq!(read_content, content);

        // Verify hash is correct BLAKE3
        let expected_hash = blake3::hash(content);
        assert_eq!(hash, *expected_hash.as_bytes());
    }

    #[test]
    fn test_blob_deduplication() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let content = b"duplicate content";

        // Write same content twice
        let hash1 = write_blob(cairn_dir, content).unwrap();
        let hash2 = write_blob(cairn_dir, content).unwrap();

        // Should produce same hash
        assert_eq!(hash1, hash2);

        // Should only exist once in filesystem
        let blob_path = cairn_dir.join("blobs").join(hex::encode(hash1));
        assert!(blob_path.exists());

        // Read should work
        let read_content = read_blob(cairn_dir, &hash1).unwrap();
        assert_eq!(read_content, content);
    }

    #[test]
    fn test_nonexistent_blob() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        let fake_hash = [0u8; 32];

        // Should not exist
        assert!(!blob_exists(cairn_dir, &fake_hash));

        // Reading should fail
        assert!(read_blob(cairn_dir, &fake_hash).is_err());
    }

    #[test]
    fn test_multiple_blobs() {
        let temp_dir = TempDir::new().unwrap();
        let cairn_dir = temp_dir.path();

        // Write multiple different blobs
        let content1 = b"first blob";
        let content2 = b"second blob";
        let content3 = b"third blob";

        let hash1 = write_blob(cairn_dir, content1).unwrap();
        let hash2 = write_blob(cairn_dir, content2).unwrap();
        let hash3 = write_blob(cairn_dir, content3).unwrap();

        // Hashes should be different
        assert_ne!(hash1, hash2);
        assert_ne!(hash2, hash3);
        assert_ne!(hash1, hash3);

        // All should exist and be readable
        assert_eq!(read_blob(cairn_dir, &hash1).unwrap(), content1);
        assert_eq!(read_blob(cairn_dir, &hash2).unwrap(), content2);
        assert_eq!(read_blob(cairn_dir, &hash3).unwrap(), content3);
    }
}
