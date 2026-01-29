//! Blob storage - content-addressed file storage with automatic deduplication
//!
//! Blobs are raw file contents stored once and identified by their BLAKE3 hash (hb).
//! This enables MEGA-style deduplication: same content → same hash → stored once.

use anyhow::{Context, Result};
use std::collections::HashMap;
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
    }

    Ok(hb_array)
}

/// Load a blob by its content hash
///
/// # Arguments
/// * `hb` - Content hash (BLAKE3) of the blob to load
///
/// # Returns
/// * `Vec<u8>` - Raw bytes of the blob content
pub fn load_blob(hb: &Blake3Hash) -> Result<Vec<u8>> {
    let blob_path = PathBuf::from(".cairn/blobs").join(base58_encode(hb));

    fs::read(&blob_path)
        .with_context(|| format!("Failed to load blob {}", base58_encode(hb)))
}

/// Check if a blob exists
///
/// # Arguments
/// * `hb` - Content hash (BLAKE3) of the blob to check
///
/// # Returns
/// * `bool` - true if the blob exists, false otherwise
pub fn blob_exists(hb: &Blake3Hash) -> bool {
    let blob_path = PathBuf::from(".cairn/blobs").join(base58_encode(hb));
    blob_path.exists()
}

/// Scan tracked files and store them as blobs
///
/// Walks through all files in the tracked paths, reads their content,
/// and stores them as blobs. Returns a mapping of file paths to their content hashes.
///
/// # Arguments
/// * `tracked_paths` - List of file or directory paths to track
///
/// # Returns
/// * `HashMap<PathBuf, Blake3Hash>` - Map of file paths to their blob hashes
pub fn scan_and_store_blobs(tracked_paths: &[PathBuf]) -> Result<HashMap<PathBuf, Blake3Hash>> {
    let mut file_to_blob = HashMap::new();

    for tracked_path in tracked_paths {
        if tracked_path.is_file() {
            // Single file
            let content = fs::read(tracked_path)
                .with_context(|| format!("Failed to read file {:?}", tracked_path))?;
            let hb = store_blob(&content)?;
            file_to_blob.insert(tracked_path.clone(), hb);
        } else if tracked_path.is_dir() {
            // Directory - walk recursively
            walk_and_store_blobs(tracked_path, &mut file_to_blob)?;
        }
    }

    Ok(file_to_blob)
}

/// Recursively walk a directory and store all files as blobs
fn walk_and_store_blobs(
    dir: &Path,
    file_to_blob: &mut HashMap<PathBuf, Blake3Hash>,
) -> Result<()> {
    use walkdir::WalkDir;

    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden directories and common build artifacts
            let file_name = e.file_name().to_string_lossy();
            !file_name.starts_with('.')
                && file_name != "target"
                && file_name != "node_modules"
        })
    {
        let entry = entry.context("Failed to read directory entry")?;

        if entry.file_type().is_file() {
            let path = entry.path();
            let content = fs::read(path)
                .with_context(|| format!("Failed to read file {:?}", path))?;
            let hb = store_blob(&content)?;
            file_to_blob.insert(path.to_path_buf(), hb);
        }
    }

    Ok(())
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
            let loaded = load_blob(&hb)?;

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

    #[test]
    fn test_blob_exists() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            let content = b"Test content";
            let hb = store_blob(content)?;

            assert!(blob_exists(&hb));

            // Non-existent blob
            let fake_hash = [0u8; 32];
            assert!(!blob_exists(&fake_hash));

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_scan_and_store_blobs() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create test files
            fs::create_dir_all("test_dir")?;
            fs::write("test_dir/file1.txt", b"Content 1")?;
            fs::write("test_dir/file2.txt", b"Content 2")?;

            // Scan and store
            let tracked_paths = vec![PathBuf::from("test_dir")];
            let file_to_blob = scan_and_store_blobs(&tracked_paths)?;

            assert_eq!(file_to_blob.len(), 2);

            // Verify blobs were created
            for hb in file_to_blob.values() {
                assert!(blob_exists(hb));
            }

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_content_deduplication_across_files() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create two files with identical content
            fs::create_dir_all("test_dir")?;
            let content = b"Identical content in both files";
            fs::write("test_dir/file1.txt", content)?;
            fs::write("test_dir/file2.txt", content)?;

            // Scan and store
            let tracked_paths = vec![PathBuf::from("test_dir")];
            let file_to_blob = scan_and_store_blobs(&tracked_paths)?;

            // Both files should map to the same blob hash
            let hashes: Vec<_> = file_to_blob.values().collect();
            assert_eq!(hashes.len(), 2);
            assert_eq!(hashes[0], hashes[1]);

            // Only one blob should exist on disk
            let blobs_dir = PathBuf::from(".cairn/blobs");
            let blob_count = fs::read_dir(&blobs_dir)?.count();
            assert_eq!(blob_count, 1);

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }
}
