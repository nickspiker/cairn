//! Patch navigation - jump between patches by restoring workspace state (blob-based)

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::blob;
use crate::state::{Blake3Hash, RepositoryState};
use crate::tree;

/// Jump to a patch - restore workspace to the state of the given patch
///
/// This reconstructs files from blobs referenced by the patch's tree.
/// ALL hashes are verified before ANY changes are made (atomic validation).
/// cairn_dir should be the .cairn directory (e.g., /project/.cairn)
pub fn jump_to_patch(cairn_dir: &Path, patch_hash: &str) -> Result<()> {
    // Extract project root from cairn_dir (cairn_dir = /project/.cairn, project_root = /project)
    let project_root = cairn_dir.parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid cairn_dir path"))?;

    println!("🔍 [JUMP] Project root: {:?}", project_root);
    println!("🔍 [JUMP] Cairn dir: {:?}", cairn_dir);
    println!("🔍 [JUMP] Target patch: {}", &patch_hash[..8]);

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

    // 6. PHASE 1: Validate and reconstruct all files atomically
    //    This ensures all hashes are verified BEFORE any filesystem changes
    let files = validate_and_reconstruct_files(cairn_dir, &patch_hp, &file_to_blob)
        .context("Failed to validate files for jump")?;

    println!("🔍 [JUMP] Restoring files to {:?}...", project_root);

    // 7. Determine which files changed
    let changed_files = if state.head.is_empty() {
        // No current state - write all files
        files.keys().cloned().collect()
    } else {
        // Get list of changed files between current and target
        let current_hp_bytes = crate::hash_encoding::base58_decode(&state.head)?;
        let mut current_hp = [0u8; 32];
        current_hp.copy_from_slice(&current_hp_bytes);
        get_changed_files(cairn_dir, &current_hp, &patch_hp)?
    };

    // 8. PHASE 2: Write only changed files to working directory
    //    All validations passed in Phase 1 - safe to modify filesystem
    restore_files(project_root, &files, &changed_files)?;

    // 8. Update state to point to this patch
    state.head = patch_hash.to_string();
    state.save(&cairn_dir.to_path_buf())?;

    println!("✓ Switched to patch {}", &patch_hash[..8]);
    println!("  {} files restored", files.len());

    Ok(())
}

/// Get list of files that changed between two patches
fn get_changed_files(
    cairn_dir: &Path,
    from_patch_hp: &[u8; 32],
    to_patch_hp: &[u8; 32],
) -> Result<std::collections::HashSet<PathBuf>> {
    use std::collections::HashSet;

    let from_patch = crate::patch_storage::load_patch(cairn_dir, from_patch_hp)?;
    let to_patch = crate::patch_storage::load_patch(cairn_dir, to_patch_hp)?;

    let from_tree = tree::load_tree(cairn_dir, &from_patch.tree_hp)?;
    let to_tree = tree::load_tree(cairn_dir, &to_patch.tree_hp)?;

    let mut changed = HashSet::new();

    // Files in target tree - check if hash changed or file is new
    for (path, to_hash) in &to_tree {
        if from_tree.get(path) != Some(to_hash) {
            changed.insert(path.clone());
        }
    }

    // Files deleted (in source but not in target)
    for path in from_tree.keys() {
        if !to_tree.contains_key(path) {
            changed.insert(path.clone());
        }
    }

    Ok(changed)
}

/// Validate reconstructed files against expected hashes from tree
///
/// This is Phase 1 of atomic jump: reconstruct and validate ALL files
/// before writing ANY. Returns error if ANY hash mismatch occurs.
///
/// # Arguments
/// * `cairn_dir` - Path to .cairn directory
/// * `patch_hp` - Target patch provenance hash
/// * `file_to_blob` - Expected file→blob hash mappings from tree
///
/// # Returns
/// * `HashMap<PathBuf, Vec<u8>>` - Map of validated file paths to content
fn validate_and_reconstruct_files(
    cairn_dir: &Path,
    patch_hp: &[u8; 32],
    file_to_blob: &HashMap<PathBuf, Blake3Hash>,
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    let mut validated_files = HashMap::new();

    println!("🔍 [VALIDATION] Validating {} files...", file_to_blob.len());

    for (path, expected_hash) in file_to_blob {
        // Reconstruct file content
        let content = reconstruct_file_content(cairn_dir, patch_hp, path)
            .with_context(|| format!("Failed to reconstruct file: {:?}", path))?;

        // Compute BLAKE3 hash of reconstructed content
        let computed_hash = *blake3::hash(&content).as_bytes();

        // Verify hash matches expected
        if &computed_hash != expected_hash {
            // Hash mismatch - abort entire operation
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

        println!(
            "  ✓ Validated: {:?} (hash: {})",
            path,
            crate::hash_encoding::base58_encode(expected_hash)
                .chars()
                .take(8)
                .collect::<String>()
        );

        validated_files.insert(path.clone(), content);
    }

    println!("✓ All {} files validated successfully", validated_files.len());
    Ok(validated_files)
}

/// Restore files to working directory from snapshot
fn restore_files(
    project_root: &Path,
    files: &HashMap<PathBuf, Vec<u8>>,
    changed_files: &std::collections::HashSet<PathBuf>,
) -> Result<()> {
    for (path, content) in files {
        // Skip files that didn't change
        if !changed_files.contains(path) {
            continue;
        }
        // Resolve absolute path relative to project root
        let abs_path = project_root.join(path);

        // Check if file already matches (via content hash)
        if abs_path.exists() {
            if let Ok(existing_content) = fs::read(&abs_path) {
                if existing_content == *content {
                    // File already matches - skip write
                    continue;
                }
            }
        }

        println!("  📝 Writing: {:?} ({} bytes)", abs_path, content.len());

        // Create parent directories
        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        // Write file
        fs::write(&abs_path, content)
            .with_context(|| format!("Failed to write file: {:?}", abs_path))?;
    }

    Ok(())
}

/// Reconstruct file content by walking diff chain from base blob
fn reconstruct_file_content(
    cairn_dir: &Path,
    target_patch_hp: &[u8; 32],
    file_path: &Path,
) -> Result<Vec<u8>> {
    println!("  🔍 Reconstructing: {:?}", file_path);

    // Walk backwards to find base blob and build diff chain
    let (base_blob_hp, diff_chain) = find_base_and_chain(
        cairn_dir,
        target_patch_hp,
        file_path
    )?;

    println!("    📦 Base blob: {}", bs58::encode(&base_blob_hp).into_string().chars().take(8).collect::<String>());
    println!("    🔗 Diff chain length: {}", diff_chain.len());

    // Load base blob
    let mut content = blob::load_blob(cairn_dir, &base_blob_hp)?;
    println!("    📄 Base content: {} bytes", content.len());

    // Apply diffs forward
    for (i, diff_ops) in diff_chain.iter().enumerate() {
        println!("    ⚙️  Applying diff {}: {} operations", i + 1, diff_ops.len());
        for (j, op) in diff_ops.iter().enumerate() {
            match op {
                crate::patch::ByteOp::Copy { start, len } => {
                    println!("      [{}] Copy: start={} len={}", j, start, len);
                }
                crate::patch::ByteOp::Insert { content: insert_content } => {
                    println!("      [{}] Insert: {} bytes", j, insert_content.len());
                }
            }
        }
        let old_len = content.len();
        content = crate::reconstruct::apply_diff_forward(&content, diff_ops)?;
        println!("    ✓ After diff {}: {} -> {} bytes", i + 1, old_len, content.len());
    }

    Ok(content)
}

/// Find base blob and build diff chain for a file
fn find_base_and_chain(
    cairn_dir: &Path,
    target_patch_hp: &[u8; 32],
    file_path: &Path,
) -> Result<([u8; 32], Vec<Vec<crate::patch::ByteOp>>)> {
    let patch = crate::patch_storage::load_patch(cairn_dir, target_patch_hp)?;

    println!("    🔎 Looking for diff in patch");
    println!("    📋 Patch has file_diffs: {}", patch.file_diffs.is_some());

    if let Some(diffs) = &patch.file_diffs {
        println!("    📂 Diffs map has {} entries", diffs.len());
        for (path, _) in diffs {
            println!("      - {:?}", path);
        }

        if let Some(diff) = diffs.get(file_path) {
            println!("    ✓ Found diff for {:?}", file_path);
            println!("      Strategy: {:?}", diff.strategy);
            println!("      Old blob: {}", bs58::encode(&diff.old_blob).into_string().chars().take(8).collect::<String>());
            println!("      New blob: {}", bs58::encode(&diff.new_blob).into_string().chars().take(8).collect::<String>());
            println!("      Ops count: {}", diff.ops.len());

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
        } else {
            println!("    ⚠️  File {:?} not found in diffs map", file_path);
        }
    } else {
        println!("    ⚠️  Patch has no file_diffs");
    }

    // File not in this patch's diffs - load from tree (unchanged from parent or first patch)
    let tree = tree::load_tree(cairn_dir, &patch.tree_hp)?;
    let blob_hash = tree.get(file_path)
        .ok_or_else(|| anyhow::anyhow!("File {:?} not found in tree", file_path))?;
    Ok((*blob_hash, vec![]))
}

/// Get the current patch hash from state
pub fn get_current_patch(cairn_dir: &Path) -> Result<String> {
    let state = RepositoryState::load(&cairn_dir.to_path_buf())?;
    if state.head.is_empty() {
        anyhow::bail!("No current patch (repository is empty)");
    }
    Ok(state.head)
}
