//! Patch storage - build metadata with tree references and parent chain
//!
//! Patches are VSF files that record successful builds with:
//! - Build metadata (timestamp, message, build hash)
//! - Tree reference (root directory structure)
//! - Optional parent reference (for patch chain)
//!
//! Each patch is identified by its provenance hash (hp = BLAKE3(patch_vsf)),
//! which includes Eagle Time to ensure uniqueness.
//!
//! Patch VSF format:
//! ```text
//! [patch_metadata
//!   timestamp: eu6{oscillations}
//!   build_hash: hb"..."
//!   message: x"Successful build"
//! ]
//! [tree
//!   root: hp"tree_hash..."
//! ]
//! [parent (optional)
//!   patch: hp"parent_patch_hash..."
//! ]
//! ```

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use vsf::types::EtType;
use vsf::types::eagle_time;
use vsf::verification::compute_provenance_hash;
use vsf::{VsfBuilder, VsfSection, VsfType};

use crate::hash_encoding::base58_encode;
use crate::patch::ByteOp;
use crate::state::Blake3Hash;
use std::collections::HashMap;

/// Information needed to create a patch
#[derive(Debug, Clone)]
pub struct PatchInfo {
    /// Patch message (e.g., "Successful build")
    pub message: String,
    /// Build output hash (BLAKE3 of cargo output)
    pub build_hash: Blake3Hash,
    /// Tree provenance hash (hp) - references directory structure
    pub tree_hp: Blake3Hash,
    /// Parent patch provenance hash (hp) - None for first patch
    pub parent_patch_hp: Option<Blake3Hash>,
    /// Per-file diffs (only present when parent exists)
    pub file_diffs: Option<HashMap<PathBuf, FileDiff>>,
}

/// Strategy for storing file changes in diffs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStrategy {
    /// Apply diff operations (Copy+Insert) from old blob
    Diff = 0,
    /// Load new blob directly (chain reset when chain size > file size)
    NewBlob = 1,
}

impl DiffStrategy {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(DiffStrategy::Diff),
            1 => Ok(DiffStrategy::NewBlob),
            _ => Err(anyhow!("Invalid DiffStrategy value: {}", value)),
        }
    }
}

/// Per-file diff information stored in patch
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Strategy: diff or new_blob (chain reset)
    pub strategy: DiffStrategy,
    /// Parent's blob hash (for verification)
    pub old_blob: Blake3Hash,
    /// Current blob hash
    pub new_blob: Blake3Hash,
    /// Diff operations (Copy+Insert sequence)
    pub ops: Vec<ByteOp>,
}

/// Parsed patch data loaded from VSF
#[derive(Debug, Clone)]
pub struct Patch {
    /// Patch message
    pub message: String,
    /// Build output hash
    pub build_hash: Blake3Hash,
    /// Eagle Time timestamp (oscillations since epoch)
    pub timestamp: u64,
    /// Tree provenance hash
    pub tree_hp: Blake3Hash,
    /// Parent patch provenance hash (None for first patch)
    pub parent_patch_hp: Option<Blake3Hash>,
    /// Per-file diffs (only present when parent exists)
    pub file_diffs: Option<HashMap<PathBuf, FileDiff>>,
}

/// Create a patch from metadata
///
/// Returns the patch's provenance hash (hp), which includes Eagle Time
/// for collision-free uniqueness.
///
/// # Arguments
/// * `info` - Patch metadata (message, build hash, tree, parent)
///
/// # Returns
/// * `Blake3Hash` - The patch's provenance hash (hp)
pub fn create_patch(info: PatchInfo) -> Result<Blake3Hash> {
    let mut builder = VsfBuilder::new();

    // 1. Metadata section with Eagle Time
    let mut metadata = VsfSection::new("patch_metadata");
    let timestamp = eagle_time::eagle_time_oscillations();
    metadata.add_field("timestamp", VsfType::e(EtType::u(timestamp)));
    metadata.add_field("build_hash", VsfType::hb(info.build_hash.to_vec()));
    metadata.add_field("message", VsfType::x(info.message));
    builder = builder.add_section_direct(metadata);

    // 2. Tree section with root tree reference
    let mut tree_section = VsfSection::new("tree");
    tree_section.add_field("root", VsfType::hp(info.tree_hp.to_vec()));
    builder = builder.add_section_direct(tree_section);

    // 3. Parent section (if exists)
    if let Some(parent_hp) = info.parent_patch_hp {
        let mut parent_section = VsfSection::new("parent");
        parent_section.add_field("patch", VsfType::hp(parent_hp.to_vec()));

        // Add diffs section with VSF-encoded operations
        if let Some(ref diffs) = info.file_diffs {
            let mut diffs_section = VsfSection::new("diffs");

            for (path, diff) in diffs {
                // Convert path to hex-encoded label (f_{hex})
                let path_label = path_to_hex_label(path)?;
                let mut file_diff_section = VsfSection::new(&path_label);

                // Strategy field (0=diff, 1=new_blob)
                file_diff_section.add_field("strategy", VsfType::u3(diff.strategy as u8));
                file_diff_section.add_field("old_blob", VsfType::hb(diff.old_blob.to_vec()));
                file_diff_section.add_field("new_blob", VsfType::hb(diff.new_blob.to_vec()));

                // Encode operations as nested VSF sections
                let mut ops_section = VsfSection::new("ops");
                for op in &diff.ops {
                    match op {
                        ByteOp::Copy { start, len } => {
                            let mut copy_section = VsfSection::new("copy");
                            copy_section.add_field("start", VsfType::u(*start, false));
                            copy_section.add_field("len", VsfType::u(*len, false));
                            ops_section.add_subsection(copy_section);
                        }
                        ByteOp::Insert { content } => {
                            let mut insert_section = VsfSection::new("insert");
                            insert_section.add_field("data", VsfType::v('b' as u8, content.clone()));
                            ops_section.add_subsection(insert_section);
                        }
                    }
                }
                file_diff_section.add_subsection(ops_section);

                diffs_section.add_subsection(file_diff_section);
            }

            parent_section.add_subsection(diffs_section);
        }

        builder = builder.add_section_direct(parent_section);
    }

    // 4. Build VSF bytes
    let patch_bytes = builder.build().map_err(|e| anyhow!("{}", e))?;

    // 5. Compute provenance hash (includes Eagle Time → unique hp)
    let hp = compute_provenance_hash(&patch_bytes).map_err(|e| anyhow!("{}", e))?;

    // 6. Write patch to .cairn/patches/{base58_hp}.vsf
    let patches_dir = PathBuf::from(".cairn/patches");
    fs::create_dir_all(&patches_dir).context("Failed to create patches directory")?;

    let patch_path = patches_dir.join(format!("{}.vsf", base58_encode(&hp)));
    fs::write(&patch_path, &patch_bytes)
        .with_context(|| format!("Failed to write patch {}", base58_encode(&hp)))?;

    Ok(hp)
}

/// Load a patch by its provenance hash
///
/// # Arguments
/// * `cairn_dir` - Path to .cairn directory
/// * `hp` - Provenance hash (BLAKE3) of the patch to load
///
/// # Returns
/// * `Patch` - Parsed patch data
pub fn load_patch(cairn_dir: &Path, hp: &Blake3Hash) -> Result<Patch> {
    let patch_path = cairn_dir.join("patches")
        .join(format!("{}.vsf", base58_encode(hp)));

    let bytes = fs::read(&patch_path)
        .with_context(|| format!("Failed to load patch {}", base58_encode(hp)))?;

    // Parse VSF header
    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode patch VSF header: {}", e))?;

    // Parse sections
    let mut message = String::new();
    let mut build_hash = [0u8; 32];
    let mut timestamp = 0u64;
    let mut tree_hp = [0u8; 32];
    let mut parent_patch_hp = None;
    let mut file_diffs = None;

    for field in &header.fields {
        if field.size_bytes == 0 {
            continue;
        }

        let mut ptr = field.offset_bytes;
        let section = VsfSection::parse(&bytes, &mut ptr)
            .map_err(|e| anyhow!("Failed to parse section '{}': {}", field.name, e))?;

        match section.name.as_str() {
            "patch_metadata" => {
                // Extract message
                if let Some(msg_field) = section.get_field("message") {
                    if let Some(VsfType::x(text)) = msg_field.values.first() {
                        message = text.clone();
                    }
                }

                // Extract build hash
                if let Some(hash_field) = section.get_field("build_hash") {
                    if let Some(VsfType::hb(hash_vec)) = hash_field.values.first() {
                        if hash_vec.len() == 32 {
                            build_hash.copy_from_slice(hash_vec);
                        }
                    }
                }

                // Extract timestamp
                if let Some(time_field) = section.get_field("timestamp") {
                    if let Some(VsfType::e(et)) = time_field.values.first() {
                        timestamp = match et {
                            EtType::u(val) => *val,
                            EtType::i(val) => *val as u64,
                            EtType::f5(val) => *val as u64,
                            EtType::f6(val) => *val as u64,
                        };
                    }
                }
            }
            "tree" => {
                // Extract tree hp
                if let Some(root_field) = section.get_field("root") {
                    if let Some(VsfType::hp(hash_vec)) = root_field.values.first() {
                        if hash_vec.len() == 32 {
                            tree_hp.copy_from_slice(hash_vec);
                        }
                    }
                }
            }
            "parent" => {
                // Extract parent patch hp
                if let Some(parent_field) = section.get_field("patch") {
                    if let Some(VsfType::hp(hash_vec)) = parent_field.values.first() {
                        if hash_vec.len() == 32 {
                            let mut parent_hp = [0u8; 32];
                            parent_hp.copy_from_slice(hash_vec);
                            parent_patch_hp = Some(parent_hp);
                        }
                    }
                }

                // TODO: Extract diffs subsection
                // Need to figure out how to access nested subsections in VSF
                // For now, reconstruction will work by loading blobs directly from tree
            }
            _ => {
                // Unknown section, skip
            }
        }
    }

    Ok(Patch {
        message,
        build_hash,
        timestamp,
        tree_hp,
        parent_patch_hp,
        file_diffs,
    })
}

/// Convert a file path to a hex-encoded VSF label with f_ prefix
///
/// # Arguments
/// * `path` - File path to encode
///
/// # Returns
/// * `String` - Hex-encoded label (e.g., "f_7372632f6c69622e7273" for "src/lib.rs")
fn path_to_hex_label(path: &PathBuf) -> Result<String> {
    let path_str = path.to_str()
        .ok_or_else(|| anyhow!("Path contains invalid UTF-8"))?;
    let hex = hex::encode(path_str.as_bytes());
    Ok(format!("f_{}", hex))
}

/// Convert a hex-encoded VSF label back to a file path
///
/// # Arguments
/// * `label` - Hex-encoded label (e.g., "f_7372632f6c69622e7273")
///
/// # Returns
/// * `PathBuf` - Decoded file path
fn hex_label_to_path(label: &str) -> Result<PathBuf> {
    // Remove f_ prefix
    let hex_str = label.strip_prefix("f_")
        .ok_or_else(|| anyhow!("Label does not start with f_ prefix: {}", label))?;

    // Decode hex to bytes
    let bytes = hex::decode(hex_str)
        .with_context(|| format!("Failed to decode hex label: {}", label))?;

    // Convert bytes to UTF-8 string
    let path_str = String::from_utf8(bytes)
        .with_context(|| format!("Invalid UTF-8 in decoded path from label: {}", label))?;

    Ok(PathBuf::from(path_str))
}

/// Check if a commit exists
///
/// # Arguments
/// * `hp` - Provenance hash (BLAKE3) of the commit to check
///
/// # Returns
/// * `bool` - true if the commit exists, false otherwise
pub fn commit_exists(hp: &Blake3Hash) -> bool {
    let commit_path = PathBuf::from(".cairn/patches")
        .join(format!("{}.vsf", base58_encode(hp)));
    commit_path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::store_blob;
    use crate::tree::create_tree;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Use a mutex to ensure tests run sequentially (they modify global state via cwd)
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_create_and_load_patch_no_parent() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create a blob and tree first
            let blob_hash = store_blob(b"test content")?;
            let mut file_to_blob = HashMap::new();
            file_to_blob.insert(PathBuf::from("test.txt"), blob_hash);
            let tree_hp = create_tree(&file_to_blob)?;

            // Create commit (no parent)
            let build_hash = *blake3::hash(b"cargo build output").as_bytes();
            let commit_info = PatchInfo {
                message: "Initial commit".to_string(),
                build_hash,
                tree_hp,
                parent_patch_hp: None,
                file_diffs: None,
            };

            let commit_hp = create_patch(commit_info)?;

            // Verify commit exists
            assert!(commit_exists(&commit_hp));

            // Load commit
            let loaded = load_patch(&commit_hp)?;

            // Verify fields
            assert_eq!(loaded.message, "Initial commit");
            assert_eq!(loaded.build_hash, build_hash);
            assert_eq!(loaded.tree_hp, tree_hp);
            assert_eq!(loaded.parent_patch_hp, None);
            assert!(loaded.timestamp > 0);

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_create_and_load_patch_with_parent() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create first commit
            let blob1 = store_blob(b"content 1")?;
            let mut tree1_files = HashMap::new();
            tree1_files.insert(PathBuf::from("file1.txt"), blob1);
            let tree1_hp = create_tree(&tree1_files)?;

            let commit1_info = PatchInfo {
                message: "First commit".to_string(),
                build_hash: *blake3::hash(b"build 1").as_bytes(),
                tree_hp: tree1_hp,
                parent_patch_hp: None,
                file_diffs: None,
            };
            let commit1_hp = create_patch(commit1_info)?;

            // Create second commit with parent
            let blob2 = store_blob(b"content 2")?;
            let mut tree2_files = HashMap::new();
            tree2_files.insert(PathBuf::from("file2.txt"), blob2);
            let tree2_hp = create_tree(&tree2_files)?;

            let commit2_info = PatchInfo {
                message: "Second commit".to_string(),
                build_hash: *blake3::hash(b"build 2").as_bytes(),
                tree_hp: tree2_hp,
                parent_patch_hp: Some(commit1_hp),
                file_diffs: None,
            };
            let commit2_hp = create_patch(commit2_info)?;

            // Load second commit
            let loaded = load_patch(&commit2_hp)?;

            // Verify parent reference
            assert_eq!(loaded.message, "Second commit");
            assert_eq!(loaded.tree_hp, tree2_hp);
            assert_eq!(loaded.parent_patch_hp, Some(commit1_hp));

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_commit_provenance_hash_includes_eagle_time() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create same tree
            let blob_hash = store_blob(b"test")?;
            let mut file_to_blob = HashMap::new();
            file_to_blob.insert(PathBuf::from("test.txt"), blob_hash);
            let tree_hp = create_tree(&file_to_blob)?;

            // Create first commit
            let build_hash = *blake3::hash(b"build").as_bytes();
            let commit1_info = PatchInfo {
                message: "Test commit".to_string(),
                build_hash,
                tree_hp,
                parent_patch_hp: None,
                file_diffs: None,
            };
            let commit1_hp = create_patch(commit1_info)?;

            // Wait a tiny bit
            std::thread::sleep(std::time::Duration::from_micros(1));

            // Create second commit with identical content
            let commit2_info = PatchInfo {
                message: "Test commit".to_string(),
                build_hash,
                tree_hp,
                parent_patch_hp: None,
                file_diffs: None,
            };
            let commit2_hp = create_patch(commit2_info)?;

            // Patchs should have different hp due to Eagle Time
            assert_ne!(
                commit1_hp, commit2_hp,
                "Patchs with identical content should have different hp due to Eagle Time"
            );

            // But both should reference the same tree
            let loaded1 = load_patch(&commit1_hp)?;
            let loaded2 = load_patch(&commit2_hp)?;
            assert_eq!(loaded1.tree_hp, loaded2.tree_hp);

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_commit_chain() -> Result<()> {
        let _lock = TEST_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new()?;
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(temp_dir.path())?;

        let result = (|| -> Result<()> {
            // Create chain of 3 commits
            let mut commits = Vec::new();
            let mut parent_hp = None;

            for i in 0..3 {
                let blob = store_blob(format!("content {}", i).as_bytes())?;
                let mut tree_files = HashMap::new();
                tree_files.insert(PathBuf::from(format!("file{}.txt", i)), blob);
                let tree_hp = create_tree(&tree_files)?;

                let commit_info = PatchInfo {
                    message: format!("Patch {}", i),
                    build_hash: *blake3::hash(format!("build {}", i).as_bytes()).as_bytes(),
                    tree_hp,
                    parent_patch_hp: parent_hp,
                    file_diffs: None,
                };

                let commit_hp = create_patch(commit_info)?;
                commits.push(commit_hp);
                parent_hp = Some(commit_hp);
            }

            // Verify chain: commit 2 → commit 1 → commit 0
            let commit2 = load_patch(&commits[2])?;
            assert_eq!(commit2.parent_patch_hp, Some(commits[1]));

            let commit1 = load_patch(&commits[1])?;
            assert_eq!(commit1.parent_patch_hp, Some(commits[0]));

            let commit0 = load_patch(&commits[0])?;
            assert_eq!(commit0.parent_patch_hp, None);

            Ok(())
        })();

        std::env::set_current_dir(original_dir)?;
        result
    }

    #[test]
    fn test_commit_nonexistent() {
        let fake_hp = [0u8; 32];
        assert!(!commit_exists(&fake_hp));
    }
}
