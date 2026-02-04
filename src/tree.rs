//! Tree storage - directory structure with blob references
//!
//! Trees are VSF files that map file paths to blob hashes (hb).
//! Each tree is identified by its provenance hash (hp = BLAKE3(tree_vsf)),
//! which is computed from the file→blob mappings.
//!
//! Tree VSF format:
//! ```text
//! [files
//!   (f_7372632f6c69622e7273: hb"abc123...")  # src/lib.rs → blob hash
//!   (f_7372632f6d61696e2e7273: hb"def456...")  # src/main.rs → blob hash
//! ]
//! ```

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use vsf::{VsfBuilder, VsfSection, VsfType};

use crate::hash_encoding::base58_encode;
use crate::snapshot_vsf::{path_to_vsf_label, vsf_label_to_path};
use crate::state::Blake3Hash;

/// Create a tree from file→blob mappings
///
/// Returns the tree's provenance hash (hp), which is computed from the
/// file→blob mappings. Identical content produces identical hashes.
///
/// # Arguments
/// * `file_to_blob` - Mapping of file paths to their blob content hashes (hb)
///
/// # Returns
/// * `Blake3Hash` - The tree's provenance hash (hp)
pub fn create_tree(file_to_blob: &HashMap<PathBuf, Blake3Hash>) -> Result<Blake3Hash> {
    // 1. Compute content hash from sorted blob hashes ONLY (no paths)
    // This gives us pure content-based deduplication
    let mut hasher = blake3::Hasher::new();

    // Sort blob hashes only (file paths not included in hash)
    let mut sorted_hashes: Vec<_> = file_to_blob.values().copied().collect();
    sorted_hashes.sort();

    // Hash the sorted blob hashes
    for blob_hash in &sorted_hashes {
        hasher.update(blob_hash);
    }

    let content_hash = *hasher.finalize().as_bytes();

    // Keep sorted entries for VSF storage (paths needed for reconstruction)
    let mut sorted_entries: Vec<_> = file_to_blob.iter().collect();
    sorted_entries.sort_by_key(|(path, _)| path.as_os_str());

    // 2. Check if tree with this content already exists
    let trees_dir = PathBuf::from(".cairn/trees");
    let tree_path = trees_dir.join(format!("{}.vsf", base58_encode(&content_hash)));

    if tree_path.exists() {
        // Duplicate tree - return existing hash without writing
        println!("  Tree already exists (content-based deduplication)");
        return Ok(content_hash);
    }

    // 3. Build VSF for new tree
    let mut builder = VsfBuilder::new();
    let mut files = VsfSection::new("files");

    for (path, blob_hash) in sorted_entries {
        // Convert path to hex-encoded VSF label
        let label = path_to_vsf_label(path)?;
        // Store blob hash as hb type
        files.add_field(&label, VsfType::hb(blob_hash.to_vec()));
    }
    builder = builder.add_section_direct(files);

    // 4. Build and write VSF
    let tree_bytes = builder.build().map_err(|e| anyhow!("{}", e))?;
    fs::create_dir_all(&trees_dir).context("Failed to create trees directory")?;
    fs::write(&tree_path, &tree_bytes)
        .with_context(|| format!("Failed to write tree {}", base58_encode(&content_hash)))?;

    println!("  Stored tree: {}", base58_encode(&content_hash));
    Ok(content_hash)
}

/// Load a tree by its provenance hash
///
/// # Arguments
/// * `cairn_dir` - Path to .cairn directory
/// * `hp` - Provenance hash (BLAKE3) of the tree to load
///
/// # Returns
/// * `HashMap<PathBuf, Blake3Hash>` - Mapping of file paths to blob hashes
pub fn load_tree(cairn_dir: &Path, hp: &Blake3Hash) -> Result<HashMap<PathBuf, Blake3Hash>> {
    let tree_path = cairn_dir.join("trees")
        .join(format!("{}.vsf", base58_encode(hp)));

    let bytes = fs::read(&tree_path)
        .with_context(|| format!("Failed to load tree {}", base58_encode(hp)))?;

    // Parse VSF header
    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode tree VSF header: {}", e))?;

    // Find the "files" section
    let files_field = header
        .fields
        .iter()
        .find(|f| f.name == "files")
        .context("Tree missing 'files' section")?;

    // Check if section is empty
    if files_field.size_bytes == 0 {
        // Empty tree - no files
        return Ok(HashMap::new());
    }

    // Parse the files section
    let mut ptr = files_field.offset_bytes;
    let files_section = VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow!("Failed to parse files section: {}", e))?;

    // Extract path→blob mappings
    let mut file_to_blob = HashMap::new();

    for field in &files_section.fields {
        // Decode hex-encoded path label
        let path = vsf_label_to_path(&field.name)?;

        // Extract blob hash (hb)
        if let Some(VsfType::hb(blob_hash_vec)) = field.values.first() {
            if blob_hash_vec.len() == 32 {
                let mut blob_hash = [0u8; 32];
                blob_hash.copy_from_slice(blob_hash_vec);
                file_to_blob.insert(path, blob_hash);
            } else {
                return Err(anyhow!(
                    "Invalid blob hash length for {}: {} bytes",
                    field.name,
                    blob_hash_vec.len()
                ));
            }
        } else {
            return Err(anyhow!("Missing or invalid blob hash for file: {}", field.name));
        }
    }

    Ok(file_to_blob)
}

/// Check if a tree exists
///
/// # Arguments
/// * `hp` - Provenance hash (BLAKE3) of the tree to check
///
/// # Returns
/// * `bool` - true if the tree exists, false otherwise
pub fn tree_exists(hp: &Blake3Hash) -> bool {
    let tree_path = PathBuf::from(".cairn/trees")
        .join(format!("{}.vsf", base58_encode(hp)));
    tree_path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::store_blob;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Use a mutex to ensure tests run sequentially (they modify global state via cwd)
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_create_and_load_tree() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create some test blobs
            let content1 = b"fn main() {}";
            let content2 = b"fn test() {}";
            let blob1 = store_blob(content1)?;
            let blob2 = store_blob(content2)?;

            // Create file→blob mapping
            let mut file_to_blob = HashMap::new();
            file_to_blob.insert(PathBuf::from("src/main.rs"), blob1);
            file_to_blob.insert(PathBuf::from("src/lib.rs"), blob2);

            // Create tree
            let tree_hp = create_tree(&file_to_blob)?;

            // Verify tree exists
            assert!(tree_exists(&tree_hp));

            // Load tree
            let cairn_dir = PathBuf::from(".cairn");
            let loaded = load_tree(&cairn_dir, &tree_hp)?;

            // Verify mappings
            assert_eq!(loaded.len(), 2);
            assert_eq!(loaded.get(&PathBuf::from("src/main.rs")), Some(&blob1));
            assert_eq!(loaded.get(&PathBuf::from("src/lib.rs")), Some(&blob2));

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_tree_content_based_hashing() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create same blob
            let content = b"test content";
            let blob_hash = store_blob(content)?;

            // Create same file→blob mapping twice
            let mut file_to_blob = HashMap::new();
            file_to_blob.insert(PathBuf::from("test.txt"), blob_hash);

            // Create first tree
            let tree1_hp = create_tree(&file_to_blob)?;

            // Create second tree with identical content
            let tree2_hp = create_tree(&file_to_blob)?;

            // Trees should have identical hp since content is identical (duplicate detection)
            assert_eq!(
                tree1_hp, tree2_hp,
                "Trees with identical content should have identical hp for duplicate detection"
            );

            // Load the tree (both hashes point to the same tree)
            let cairn_dir = PathBuf::from(".cairn");
            let loaded = load_tree(&cairn_dir, &tree1_hp)?;
            assert_eq!(
                loaded.get(&PathBuf::from("test.txt")),
                Some(&blob_hash)
            );

            // Only one tree file should exist (since they have the same hash)
            assert!(tree_exists(&tree1_hp));

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_empty_tree() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create empty tree
            let file_to_blob = HashMap::new();
            let tree_hp = create_tree(&file_to_blob)?;

            // Load empty tree
            let cairn_dir = PathBuf::from(".cairn");
            let loaded = load_tree(&cairn_dir, &tree_hp)?;

            assert_eq!(loaded.len(), 0);
            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_tree_with_complex_paths() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create blobs
            let blob1 = store_blob(b"content1")?;
            let blob2 = store_blob(b"content2")?;
            let blob3 = store_blob(b"content3")?;

            // Create tree with complex paths
            let mut file_to_blob = HashMap::new();
            file_to_blob.insert(PathBuf::from("src/nested/deep/file.rs"), blob1);
            file_to_blob.insert(PathBuf::from("Cargo.toml"), blob2);
            file_to_blob.insert(PathBuf::from("tests/integration_test.rs"), blob3);

            // Create and load tree
            let tree_hp = create_tree(&file_to_blob)?;
            let cairn_dir = PathBuf::from(".cairn");
            let loaded = load_tree(&cairn_dir, &tree_hp)?;

            // Verify all paths preserved correctly
            assert_eq!(loaded.len(), 3);
            assert_eq!(
                loaded.get(&PathBuf::from("src/nested/deep/file.rs")),
                Some(&blob1)
            );
            assert_eq!(loaded.get(&PathBuf::from("Cargo.toml")), Some(&blob2));
            assert_eq!(
                loaded.get(&PathBuf::from("tests/integration_test.rs")),
                Some(&blob3)
            );

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_tree_nonexistent() {
        let fake_hp = [0u8; 32];
        assert!(!tree_exists(&fake_hp));
    }
}
