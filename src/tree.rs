//! Tree storage - directory structure with blob references
//!
//! Trees are VSF files that map file paths to blob hashes (hb).
//! Each tree is identified by its provenance hash (hp = BLAKE3(tree_vsf)),
//! which includes Eagle Time to ensure uniqueness even for identical content.
//!
//! Tree VSF format:
//! ```text
//! [tree_metadata
//!   created: eu6{oscillations}
//! ]
//! [files
//!   (f_7372632f6c69622e7273: hb"abc123...")  # src/lib.rs → blob hash
//!   (f_7372632f6d61696e2e7273: hb"def456...")  # src/main.rs → blob hash
//! ]
//! ```

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use vsf::types::EtType;
use vsf::types::eagle_time;
use vsf::verification::compute_provenance_hash;
use vsf::{VsfBuilder, VsfSection, VsfType};

use crate::hash_encoding::base58_encode;
use crate::snapshot_vsf::{path_to_vsf_label, vsf_label_to_path};
use crate::state::Blake3Hash;

/// Create a tree from file→blob mappings
///
/// Returns the tree's provenance hash (hp), which includes Eagle Time
/// for collision-free uniqueness.
///
/// # Arguments
/// * `file_to_blob` - Mapping of file paths to their blob content hashes (hb)
///
/// # Returns
/// * `Blake3Hash` - The tree's provenance hash (hp)
pub fn create_tree(file_to_blob: &HashMap<PathBuf, Blake3Hash>) -> Result<Blake3Hash> {
    let mut builder = VsfBuilder::new();

    // 1. Metadata section with Eagle Time
    let mut metadata = VsfSection::new("tree_metadata");
    metadata.add_field(
        "created",
        VsfType::e(EtType::u(eagle_time::eagle_time_oscillations())),
    );
    builder = builder.add_section_direct(metadata);

    // 2. Files section with path→blob mappings
    let mut files = VsfSection::new("files");
    for (path, blob_hash) in file_to_blob {
        // Convert path to hex-encoded VSF label
        let label = path_to_vsf_label(path)?;

        // Store blob hash as hb type
        files.add_field(&label, VsfType::hb(blob_hash.to_vec()));
    }
    builder = builder.add_section_direct(files);

    // 3. Build VSF bytes
    let tree_bytes = builder.build().map_err(|e| anyhow!("{}", e))?;

    // 4. Compute provenance hash (includes Eagle Time → unique hp)
    let hp = compute_provenance_hash(&tree_bytes).map_err(|e| anyhow!("{}", e))?;

    // 5. Write tree to .cairn/trees/{base58_hp}.vsf
    let trees_dir = PathBuf::from(".cairn/trees");
    fs::create_dir_all(&trees_dir).context("Failed to create trees directory")?;

    let tree_path = trees_dir.join(format!("{}.vsf", base58_encode(&hp)));
    fs::write(&tree_path, &tree_bytes)
        .with_context(|| format!("Failed to write tree {}", base58_encode(&hp)))?;

    Ok(hp)
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
            let loaded = load_tree(&tree_hp)?;

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
    fn test_tree_provenance_hash_includes_eagle_time() -> Result<()> {
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

            // Wait a tiny bit (Eagle Time has 704ps resolution, but filesystem ops take longer)
            std::thread::sleep(std::time::Duration::from_micros(1));

            // Create second tree with identical content
            let tree2_hp = create_tree(&file_to_blob)?;

            // Trees should have different hp due to Eagle Time
            assert_ne!(
                tree1_hp, tree2_hp,
                "Trees with identical content should have different hp due to Eagle Time"
            );

            // But both should reference the same blob
            let loaded1 = load_tree(&tree1_hp)?;
            let loaded2 = load_tree(&tree2_hp)?;
            assert_eq!(
                loaded1.get(&PathBuf::from("test.txt")),
                loaded2.get(&PathBuf::from("test.txt"))
            );

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
            let loaded = load_tree(&tree_hp)?;

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
            let loaded = load_tree(&tree_hp)?;

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
