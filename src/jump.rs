//! Patch navigation - jump between patches by restoring workspace state (blob-based)

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::blob;
use crate::state::{Blake3Hash, RepositoryState};
use crate::tree;
use crate::vault::CairnVault;

/// Jump to a patch - restore workspace to the state of the given patch
///
/// This reconstructs files from blobs referenced by the patch's tree.
/// ALL hashes are verified before ANY changes are made (atomic validation).
/// Files present in the current patch but absent from the target are deleted,
/// so the working directory mirrors the target tree exactly.
/// cairn_dir should be the .cairn directory (e.g., /project/.cairn)
pub fn jump_to_patch(cairn_dir: &Path, patch_hash: &str) -> Result<()> {
    // Extract project root from cairn_dir (cairn_dir = /project/.cairn, project_root = /project)
    let project_root = cairn_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid cairn_dir path"))?
        .to_path_buf();

    println!("🔍 [JUMP] Project root: {:?}", project_root);
    println!("🔍 [JUMP] Target patch: {}", &patch_hash[..8.min(patch_hash.len())]);

    let mut vault = CairnVault::open_existing(cairn_dir)?;

    // 1. Load current state
    let mut state = RepositoryState::load(&mut vault)?;

    // 2. Verify patch exists
    if !state.patches.iter().any(|p| p == patch_hash) {
        anyhow::bail!("Patch {} not found in history", patch_hash);
    }

    // 3. Parse patch hash as Blake3Hash
    let patch_hp = decode_patch_id(patch_hash)?;

    // 4. Load the patch to get the tree reference
    let patch = crate::patch_storage::load_patch(&mut vault, &patch_hp)?;

    // 5. Load the tree to get path→blob mappings
    let file_to_blob = tree::load_tree(&mut vault, &patch.tree_hp).context("Failed to load tree")?;

    // 6. PHASE 1: Validate and reconstruct all files atomically
    //    This ensures all hashes are verified BEFORE any filesystem changes
    let files = validate_and_reconstruct_files(&mut vault, &patch_hp, &file_to_blob)
        .context("Failed to validate files for jump")?;

    // 7. Determine which files changed, and which must be deleted
    let (changed_files, doomed_files) = if state.head.is_empty() {
        (files.keys().cloned().collect(), Vec::new())
    } else {
        let current_hp = decode_patch_id(&state.head)?;
        diff_trees(&mut vault, &current_hp, &patch_hp)?
    };

    println!("🔍 [JUMP] Restoring files to {:?}...", project_root);

    // 8. PHASE 2: All validations passed - write changed files, delete orphans
    restore_files(&project_root, &files, &changed_files)?;
    delete_files(&project_root, &doomed_files)?;

    // 9. Update state to point to this patch
    state.head = patch_hash.to_string();
    state.save(&mut vault)?;

    println!("✓ Switched to patch {}", &patch_hash[..8.min(patch_hash.len())]);
    println!("  {} files restored, {} deleted", files.len(), doomed_files.len());

    Ok(())
}

fn decode_patch_id(patch_hash: &str) -> Result<Blake3Hash> {
    let bytes = crate::hash_encoding::base58_decode(patch_hash)?;
    if bytes.len() != 32 {
        anyhow::bail!("Invalid patch hash length: expected 32 bytes, got {}", bytes.len());
    }
    let mut hp = [0u8; 32];
    hp.copy_from_slice(&bytes);
    Ok(hp)
}

/// Compare two patches' trees: (changed files to write, files to delete).
fn diff_trees(
    vault: &mut CairnVault,
    from_patch_hp: &Blake3Hash,
    to_patch_hp: &Blake3Hash,
) -> Result<(HashSet<PathBuf>, Vec<PathBuf>)> {
    let from_patch = crate::patch_storage::load_patch(vault, from_patch_hp)?;
    let to_patch = crate::patch_storage::load_patch(vault, to_patch_hp)?;

    let from_tree = tree::load_tree(vault, &from_patch.tree_hp)?;
    let to_tree = tree::load_tree(vault, &to_patch.tree_hp)?;

    // Files in target tree whose hash changed or which are new.
    let mut changed = HashSet::new();
    for (path, to_hash) in &to_tree {
        if from_tree.get(path) != Some(to_hash) {
            changed.insert(path.clone());
        }
    }

    // Files in source but not in target: delete on restore.
    let mut doomed = Vec::new();
    for path in from_tree.keys() {
        if !to_tree.contains_key(path) {
            doomed.push(path.clone());
        }
    }

    Ok((changed, doomed))
}

/// Validate reconstructed files against expected hashes from tree
///
/// This is Phase 1 of atomic jump: reconstruct and validate ALL files
/// before writing ANY. Returns error if ANY hash mismatch occurs.
fn validate_and_reconstruct_files(
    vault: &mut CairnVault,
    patch_hp: &Blake3Hash,
    file_to_blob: &HashMap<PathBuf, Blake3Hash>,
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    let mut validated_files = HashMap::new();

    println!("🔍 [VALIDATION] Validating {} files...", file_to_blob.len());

    for (path, expected_hash) in file_to_blob {
        let content = reconstruct_file_content(vault, patch_hp, path)
            .with_context(|| format!("Failed to reconstruct file: {:?}", path))?;

        let computed_hash = *blake3::hash(&content).as_bytes();
        if &computed_hash != expected_hash {
            anyhow::bail!(
                "Hash validation failed for {:?}\n\
                 Expected: {}\n\
                 Computed: {}\n\
                 This indicates corruption in the blob store or diff chain.\n\
                 No files have been modified.",
                path,
                crate::hash_encoding::base58_encode(expected_hash),
                crate::hash_encoding::base58_encode(&computed_hash)
            );
        }

        validated_files.insert(path.clone(), content);
    }

    println!("✓ All {} files validated successfully", validated_files.len());
    Ok(validated_files)
}

/// Restore files to working directory from snapshot
fn restore_files(
    project_root: &Path,
    files: &HashMap<PathBuf, Vec<u8>>,
    changed_files: &HashSet<PathBuf>,
) -> Result<()> {
    for (path, content) in files {
        // Skip files that didn't change
        if !changed_files.contains(path) {
            continue;
        }
        let abs_path = project_root.join(path);

        // Check if file already matches (via content comparison)
        if abs_path.exists() {
            if let Ok(existing_content) = fs::read(&abs_path) {
                if existing_content == *content {
                    continue;
                }
            }
        }

        println!("  📝 Writing: {:?} ({} bytes)", abs_path, content.len());

        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        fs::write(&abs_path, content)
            .with_context(|| format!("Failed to write file: {:?}", abs_path))?;
    }

    Ok(())
}

/// Delete files present in the current patch but absent from the target, pruning
/// directories they leave empty (up to, never including, the project root).
fn delete_files(project_root: &Path, doomed: &[PathBuf]) -> Result<()> {
    for path in doomed {
        let abs_path = project_root.join(path);
        if !abs_path.exists() {
            continue; // already gone - nothing to do
        }

        println!("  🗑️  Deleting: {:?}", abs_path);
        fs::remove_file(&abs_path)
            .with_context(|| format!("Failed to delete file: {:?}", abs_path))?;

        // Prune now-empty parents; remove_dir refuses non-empty directories.
        let mut dir = abs_path.parent();
        while let Some(d) = dir {
            if d == project_root || fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
    Ok(())
}

/// Reconstruct file content by walking diff chain from base blob
fn reconstruct_file_content(
    vault: &mut CairnVault,
    target_patch_hp: &Blake3Hash,
    file_path: &Path,
) -> Result<Vec<u8>> {
    // Walk backwards to find base blob and build diff chain
    let (base_blob_hp, diff_chain) = find_base_and_chain(vault, target_patch_hp, file_path)?;

    // Load base blob
    let mut content = blob::load_blob(vault, &base_blob_hp)?;

    // Apply diffs forward
    for diff_ops in diff_chain.iter() {
        content = crate::reconstruct::apply_diff_forward(&content, diff_ops)?;
    }

    Ok(content)
}

/// Find base blob and build diff chain for a file
fn find_base_and_chain(
    vault: &mut CairnVault,
    target_patch_hp: &Blake3Hash,
    file_path: &Path,
) -> Result<(Blake3Hash, Vec<Vec<crate::patch::ByteOp>>)> {
    let patch = crate::patch_storage::load_patch(vault, target_patch_hp)?;

    if let Some(diffs) = &patch.file_diffs {
        if let Some(diff) = diffs.get(file_path) {
            match diff.strategy {
                crate::patch_storage::DiffStrategy::Diff => {
                    // old_blob is the base, ops is the diff to apply
                    return Ok((diff.old_blob, vec![diff.ops.clone()]));
                }
                crate::patch_storage::DiffStrategy::NewBlob => {
                    // Chain reset - this blob is the base (full content stored)
                    return Ok((diff.new_blob, vec![]));
                }
            }
        }
    }

    // File not in this patch's diffs - load from tree (unchanged from parent or first patch)
    let tree = tree::load_tree(vault, &patch.tree_hp)?;
    let blob_hash = tree
        .get(file_path)
        .ok_or_else(|| anyhow::anyhow!("File {:?} not found in tree", file_path))?;
    Ok((*blob_hash, vec![]))
}

/// Get the current patch hash from state
pub fn get_current_patch(cairn_dir: &Path) -> Result<String> {
    let mut vault = CairnVault::open_existing(cairn_dir)?;
    let state = RepositoryState::load(&mut vault)?;
    if state.head.is_empty() {
        anyhow::bail!("No current patch (repository is empty)");
    }
    Ok(state.head)
}
