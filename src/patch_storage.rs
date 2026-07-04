//! Patch storage - metadata with tree references and parent chain
//!
//! Patches are VSF files that record successful builds with:
//! - Metadata (message) - stored but not part of patch identity
//! - Tree reference (root directory structure)
//! - Optional parent reference (for patch chain)
//!
//! Each patch is identified by its provenance hash (hp), which is computed from
//! ONLY the source code content (tree_hp + parent_hp + diffs), NOT metadata.
//! This ensures identical source code produces identical patch IDs, enabling
//! true content-based deduplication.
//!
//! Patch VSF format:
//! ```text
//! [patch_metadata
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
use std::path::PathBuf;
use vsf::{VsfBuilder, VsfSection, VsfType};

use crate::hash_encoding::base58_encode;
use crate::patch::ByteOp;
use crate::state::Blake3Hash;
use crate::tree::{hex_label_to_path, path_to_hex_label};
use crate::vault::{CairnVault, patch_key};
use std::collections::HashMap;

/// Information needed to create a patch
#[derive(Debug, Clone)]
pub struct PatchInfo {
    /// Patch message (e.g., "Successful build")
    pub message: String,
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
    /// Tree provenance hash
    pub tree_hp: Blake3Hash,
    /// Parent patch provenance hash (None for first patch)
    pub parent_patch_hp: Option<Blake3Hash>,
    /// Per-file diffs (only present when parent exists)
    pub file_diffs: Option<HashMap<PathBuf, FileDiff>>,
}

/// Serialize diff operations into one compact byte payload (stored as a `v(b'O', ...)`
/// wrapped value — the VSF idiom for opaque structured bytes, in place of the removed
/// nested-field encoding). Per op: tag byte (0=Copy, 1=Insert), then LE u64 operands.
fn encode_ops(ops: &[ByteOp]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            ByteOp::Copy { start, len } => {
                out.push(0);
                out.extend_from_slice(&(*start as u64).to_le_bytes());
                out.extend_from_slice(&(*len as u64).to_le_bytes());
            }
            ByteOp::Insert { content } => {
                out.push(1);
                out.extend_from_slice(&(content.len() as u64).to_le_bytes());
                out.extend_from_slice(content);
            }
        }
    }
    out
}

fn decode_ops(bytes: &[u8]) -> Result<Vec<ByteOp>> {
    let mut ops = Vec::new();
    let mut p = 0usize;
    let take_u64 = |bytes: &[u8], p: &mut usize| -> Result<u64> {
        let end = *p + 8;
        let v = bytes
            .get(*p..end)
            .ok_or_else(|| anyhow!("Truncated diff ops payload"))?;
        *p = end;
        Ok(u64::from_le_bytes(v.try_into().unwrap()))
    };
    while p < bytes.len() {
        let tag = bytes[p];
        p += 1;
        match tag {
            0 => {
                let start = take_u64(bytes, &mut p)? as usize;
                let len = take_u64(bytes, &mut p)? as usize;
                ops.push(ByteOp::Copy { start, len });
            }
            1 => {
                let len = take_u64(bytes, &mut p)? as usize;
                let content = bytes
                    .get(p..p + len)
                    .ok_or_else(|| anyhow!("Truncated diff insert payload"))?
                    .to_vec();
                p += len;
                ops.push(ByteOp::Insert { content });
            }
            t => return Err(anyhow!("Unknown diff op tag: {t}")),
        }
    }
    Ok(ops)
}

/// Create a patch from metadata
///
/// Returns the patch's provenance hash (hp), which is computed from ONLY
/// the source code content (tree_hp + parent_hp + diffs). Message is
/// stored in the VSF but NOT included in the hash calculation,
/// ensuring identical source code produces identical patch IDs.
///
/// # Arguments
/// * `info` - Patch metadata (message, build hash, tree, parent)
///
/// # Returns
/// * `Blake3Hash` - The patch's content-based provenance hash (hp)
pub fn create_patch(vault: &mut CairnVault, info: PatchInfo) -> Result<Blake3Hash> {
    // 1. Patch ID = tree hash (pure content-based)
    // Identical source files → identical tree hash → identical patch ID
    // Parent is stored IN the patch VSF, but not part of the ID
    let content_hash = info.tree_hp;

    // 2. Check if patch with this content already exists
    let key = patch_key(&content_hash);
    if vault.exists(&key)? {
        // Duplicate patch - return existing hash without writing
        println!("  Patch already exists (content-based deduplication)");
        return Ok(content_hash);
    }

    println!("  Creating new patch...");

    // 3. Build VSF for new patch
    let mut builder = VsfBuilder::new();

    // Metadata section
    let mut metadata = VsfSection::new("patch_metadata");
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

        // Diffs as parallel multi-value fields — index i across the five fields is one
        // file's diff. Sorted by normalized path for deterministic encoding.
        if let Some(ref diffs) = info.file_diffs {
            let mut sorted_diffs: Vec<(String, &FileDiff)> = diffs
                .iter()
                .map(|(path, diff)| Ok((path_to_hex_label(path)?, diff)))
                .collect::<Result<_>>()?;
            sorted_diffs.sort_by(|(a, _), (b, _)| a.cmp(b));

            if !sorted_diffs.is_empty() {
                let mut paths = Vec::new();
                let mut strategies = Vec::new();
                let mut olds = Vec::new();
                let mut news = Vec::new();
                let mut ops = Vec::new();
                for (label, diff) in sorted_diffs {
                    paths.push(VsfType::x(label));
                    strategies.push(VsfType::u3(diff.strategy as u8));
                    olds.push(VsfType::hb(diff.old_blob.to_vec()));
                    news.push(VsfType::hb(diff.new_blob.to_vec()));
                    ops.push(VsfType::v(b'O', encode_ops(&diff.ops)));
                }
                parent_section.add_field_multi("diff_path", paths);
                parent_section.add_field_multi("diff_strategy", strategies);
                parent_section.add_field_multi("diff_old", olds);
                parent_section.add_field_multi("diff_new", news);
                parent_section.add_field_multi("diff_ops", ops);
            }
        }

        builder = builder.add_section_direct(parent_section);
    }

    // 4. Build and store VSF
    let patch_bytes = builder.build().map_err(|e| anyhow!("{}", e))?;
    vault
        .put(&key, &patch_bytes)
        .with_context(|| format!("Failed to store patch {}", base58_encode(&content_hash)))?;

    println!("  Wrote patch: {}", base58_encode(&content_hash));
    Ok(content_hash)
}

/// Load a patch by its provenance hash
///
/// # Arguments
/// * `cairn_dir` - Path to .cairn directory
/// * `hp` - Provenance hash (BLAKE3) of the patch to load
///
/// # Returns
/// * `Patch` - Parsed patch data
pub fn load_patch(vault: &mut CairnVault, hp: &Blake3Hash) -> Result<Patch> {
    let bytes = vault
        .get(&patch_key(hp))?
        .ok_or_else(|| anyhow!("Patch not found: {}", base58_encode(hp)))?;

    // Parse VSF header
    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode patch VSF header: {}", e))?;

    // Parse sections
    let mut message = String::new();
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

        match field.name.as_str() {
            "patch_metadata" => {
                // Extract message
                if let Some(msg_field) = section.get_field("message") {
                    if let Some(text) = msg_field.values.first().and_then(|v| v.as_string()) {
                        message = text.to_string();
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

                // Diffs are parallel multi-value fields: index i across the five fields
                // is one file's diff.
                if let Some(paths_field) = section.get_field("diff_path") {
                    let strategies = section.get_field("diff_strategy");
                    let olds = section.get_field("diff_old");
                    let news = section.get_field("diff_new");
                    let ops_field = section.get_field("diff_ops");

                    let mut diffs_map = std::collections::HashMap::new();
                    for (i, path_value) in paths_field.values.iter().enumerate() {
                        let Some(label) = path_value.as_string() else {
                            return Err(anyhow!("diff_path[{i}] is not a text value"));
                        };
                        let path = hex_label_to_path(label)?;

                        let strategy = match strategies.and_then(|f| f.values.get(i)) {
                            Some(VsfType::u3(v)) => DiffStrategy::from_u8(*v)?,
                            _ => return Err(anyhow!("Missing diff_strategy[{i}]")),
                        };

                        let mut old_blob = [0u8; 32];
                        if let Some(VsfType::hb(h)) = olds.and_then(|f| f.values.get(i)) {
                            if h.len() == 32 {
                                old_blob.copy_from_slice(h);
                            }
                        }

                        let mut new_blob = [0u8; 32];
                        if let Some(VsfType::hb(h)) = news.and_then(|f| f.values.get(i)) {
                            if h.len() == 32 {
                                new_blob.copy_from_slice(h);
                            }
                        }

                        let ops = match ops_field.and_then(|f| f.values.get(i)) {
                            Some(VsfType::v(b'O', bytes)) => decode_ops(bytes)?,
                            _ => return Err(anyhow!("Missing diff_ops[{i}]")),
                        };

                        diffs_map.insert(path, FileDiff { strategy, old_blob, new_blob, ops });
                    }

                    file_diffs = Some(diffs_map);
                }
            }
            _ => {
                // Unknown section, skip
            }
        }
    }

    Ok(Patch {
        message,
        tree_hp,
        parent_patch_hp,
        file_diffs,
    })
}

/// Check if a patch exists
pub fn commit_exists(vault: &mut CairnVault, hp: &Blake3Hash) -> bool {
    vault.exists(&patch_key(hp)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::store_blob;
    use crate::tree::create_tree;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn vault(dir: &TempDir) -> CairnVault {
        CairnVault::open(&dir.path().join(".cairn")).unwrap()
    }

    #[test]
    fn test_create_and_load_patch_no_parent() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let blob_hash = store_blob(&mut v, b"test content")?;
        let mut file_to_blob = HashMap::new();
        file_to_blob.insert(PathBuf::from("test.txt"), blob_hash);
        let tree_hp = create_tree(&mut v, &file_to_blob)?;

        let commit_info = PatchInfo {
            message: "Initial commit".to_string(),
            tree_hp,
            parent_patch_hp: None,
            file_diffs: None,
        };
        let commit_hp = create_patch(&mut v, commit_info)?;
        assert!(commit_exists(&mut v, &commit_hp));

        let loaded = load_patch(&mut v, &commit_hp)?;
        assert_eq!(loaded.message, "Initial commit");
        assert_eq!(loaded.tree_hp, tree_hp);
        assert_eq!(loaded.parent_patch_hp, None);
        Ok(())
    }

    #[test]
    fn test_create_and_load_patch_with_parent() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let blob1 = store_blob(&mut v, b"content 1")?;
        let mut tree1_files = HashMap::new();
        tree1_files.insert(PathBuf::from("file1.txt"), blob1);
        let tree1_hp = create_tree(&mut v, &tree1_files)?;

        let commit1_hp = create_patch(&mut v, PatchInfo {
            message: "First commit".to_string(),
            tree_hp: tree1_hp,
            parent_patch_hp: None,
            file_diffs: None,
        })?;

        let blob2 = store_blob(&mut v, b"content 2")?;
        let mut tree2_files = HashMap::new();
        tree2_files.insert(PathBuf::from("file2.txt"), blob2);
        let tree2_hp = create_tree(&mut v, &tree2_files)?;

        let commit2_hp = create_patch(&mut v, PatchInfo {
            message: "Second commit".to_string(),
            tree_hp: tree2_hp,
            parent_patch_hp: Some(commit1_hp),
            file_diffs: None,
        })?;

        let loaded = load_patch(&mut v, &commit2_hp)?;
        assert_eq!(loaded.message, "Second commit");
        assert_eq!(loaded.tree_hp, tree2_hp);
        assert_eq!(loaded.parent_patch_hp, Some(commit1_hp));
        Ok(())
    }

    #[test]
    fn test_commit_content_based_hashing() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let blob_hash = store_blob(&mut v, b"test")?;
        let mut file_to_blob = HashMap::new();
        file_to_blob.insert(PathBuf::from("test.txt"), blob_hash);
        let tree_hp = create_tree(&mut v, &file_to_blob)?;

        let commit1_hp = create_patch(&mut v, PatchInfo {
            message: "Test commit".to_string(),
            tree_hp,
            parent_patch_hp: None,
            file_diffs: None,
        })?;

        // Identical tree, different message → same patch ID (message is metadata only).
        let commit2_hp = create_patch(&mut v, PatchInfo {
            message: "Different message".to_string(),
            tree_hp,
            parent_patch_hp: None,
            file_diffs: None,
        })?;
        assert_eq!(commit1_hp, commit2_hp);

        let loaded = load_patch(&mut v, &commit1_hp)?;
        assert_eq!(loaded.tree_hp, tree_hp);
        assert_eq!(loaded.message, "Test commit");
        Ok(())
    }

    #[test]
    fn test_commit_chain() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let mut commits = Vec::new();
        let mut parent_hp = None;
        for i in 0..3 {
            let blob = store_blob(&mut v, format!("content {}", i).as_bytes())?;
            let mut tree_files = HashMap::new();
            tree_files.insert(PathBuf::from(format!("file{}.txt", i)), blob);
            let tree_hp = create_tree(&mut v, &tree_files)?;

            let commit_hp = create_patch(&mut v, PatchInfo {
                message: format!("Patch {}", i),
                tree_hp,
                parent_patch_hp: parent_hp,
                file_diffs: None,
            })?;
            commits.push(commit_hp);
            parent_hp = Some(commit_hp);
        }

        assert_eq!(load_patch(&mut v, &commits[2])?.parent_patch_hp, Some(commits[1]));
        assert_eq!(load_patch(&mut v, &commits[1])?.parent_patch_hp, Some(commits[0]));
        assert_eq!(load_patch(&mut v, &commits[0])?.parent_patch_hp, None);
        Ok(())
    }

    #[test]
    fn test_commit_nonexistent() {
        let dir = TempDir::new().unwrap();
        let mut v = vault(&dir);
        assert!(!commit_exists(&mut v, &[0u8; 32]));
    }
}
