//! Patch navigation - jump between patches by restoring workspace state (blob-based)

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::blob;
use crate::state::RepositoryState;
use crate::tree;

/// Jump to a patch - restore workspace to the state of the given patch
///
/// This reconstructs files from blobs referenced by the patch's tree.
pub fn jump_to_patch(cairn_dir: &Path, patch_hash: &str) -> Result<()> {
    // 1. Load current state
    let mut state = RepositoryState::load(&cairn_dir.to_path_buf())?;

    // 2. Verify patch exists
    if !state.patches.iter().any(|p| p == patch_hash) {
        anyhow::bail!("Patch {} not found in history", patch_hash);
    }

    // 3. Parse patch hash as Blake3Hash
    let patch_bytes = crate::hash_encoding::base58_decode(patch_hash)?;
    if patch_bytes.len() != 32 {
        anyhow::bail!("Invalid patch hash length: expected 32 bytes, got {}", patch_bytes.len());
    }
    let mut patch_hp = [0u8; 32];
    patch_hp.copy_from_slice(&patch_bytes);

    // 4. Load the patch to get the tree reference
    let patch = crate::patch_storage::load_patch(cairn_dir, &patch_hp)?;

    // 5. Load the tree to get path→blob mappings
    let file_to_blob = tree::load_tree(cairn_dir, &patch.tree_hp)
        .context("Failed to load tree")?;

    // 6. Load each blob and reconstruct files
    let mut files = HashMap::new();
    for (path, blob_hash) in &file_to_blob {
        let content = blob::load_blob(cairn_dir, blob_hash)
            .with_context(|| format!("Failed to load blob for {:?}", path))?;
        files.insert(path.clone(), content);
    }

    // 7. Write files to working directory
    restore_files(&files)?;

    // 8. Update state to point to this patch
    state.head = patch_hash.to_string();
    state.save(&cairn_dir.to_path_buf())?;

    println!("✓ Switched to patch {}", &patch_hash[..8]);
    println!("  {} files restored", files.len());

    Ok(())
}

/// Restore files to working directory from snapshot
fn restore_files(files: &HashMap<PathBuf, Vec<u8>>) -> Result<()> {
    for (path, content) in files {
        // Create parent directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        // Write file
        fs::write(path, content)
            .with_context(|| format!("Failed to write file: {:?}", path))?;
    }

    Ok(())
}

/// Get the current patch hash from state
pub fn get_current_patch(cairn_dir: &Path) -> Result<String> {
    let state = RepositoryState::load(&cairn_dir.to_path_buf())?;
    if state.head.is_empty() {
        anyhow::bail!("No current patch (repository is empty)");
    }
    Ok(state.head)
}
