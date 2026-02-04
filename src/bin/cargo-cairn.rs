//! cargo-cairn - Cargo wrapper that automatically creates patches on successful builds
//!
//! Usage: cargo cairn <cargo-command> [args...]
//!
//! This wraps any cargo command and creates a cairn patch if it succeeds.
//! The snapshot is taken BEFORE the build starts, ensuring it matches exactly
//! what cargo compiled.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, exit};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        exit(1);
    }
}

fn run() -> Result<()> {
    // Get args - cargo passes: ["cargo-cairn", "cairn", "build", ...]
    // We want to skip the first two and get: ["build", ...]
    let args: Vec<String> = std::env::args().skip(2).collect();

    if args.is_empty() {
        eprintln!("Usage: cargo cairn <command> [args...]");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  cargo cairn build");
        eprintln!("  cargo cairn build --release");
        eprintln!("  cargo cairn test");
        eprintln!("  cargo cairn check");
        exit(1);
    }

    let command_name = &args[0];

    // Auto-initialize cairn if needed (for build-related commands)
    let cairn_dir = PathBuf::from(".cairn");
    let is_build_command = matches!(
        command_name.as_str(),
        "build" | "b" | "test" | "bench" | "run"
    );

    // Auto-init if this is a build command and cairn isn't initialized
    if is_build_command && !cairn_dir.exists() {
        if let Err(e) = auto_init(&cairn_dir) {
            eprintln!("⚠️  Cairn: Failed to auto-initialize: {}", e);
            eprintln!("   Continuing with build anyway...");
        } else {
            println!("✓ Cairn: Auto-initialized .cairn/");
        }
    }

    // Only snapshot for build-related commands
    let should_snapshot = is_build_command && cairn_dir.exists();

    // If we should snapshot, take one BEFORE the build
    let snapshot_taken = if should_snapshot {
        match create_pending_snapshot() {
            Ok(true) => {
                println!("📸 Cairn: Snapshotting current state...");
                true
            }
            Ok(false) => {
                // No changes detected
                false
            }
            Err(e) => {
                eprintln!("⚠️  Cairn: Failed to create snapshot: {}", e);
                eprintln!("   Continuing with build anyway...");
                false
            }
        }
    } else {
        false
    };

    // Run the actual cargo command
    let status = Command::new("cargo")
        .args(&args)
        .status()
        .context("Failed to execute cargo")?;

    let exit_code = status.code().unwrap_or(1);

    // If build succeeded and we took a snapshot, save it
    if status.success() && snapshot_taken {
        if let Err(e) = save_pending_snapshot() {
            eprintln!("⚠️  Cairn: Failed to create patch: {}", e);
        } else {
            println!("✓ Cairn: Patch created");
            // Note: Daemon will detect new patches when extension refreshes
        }
    } else if snapshot_taken {
        // Build failed - discard the snapshot
        let _ = discard_pending_snapshot();
    }

    exit(exit_code);
}

/// Create a pending snapshot (not yet committed)
/// Returns Ok(true) if snapshot was created, Ok(false) if no changes
fn create_pending_snapshot() -> Result<bool> {
    let cairn_dir = PathBuf::from(".cairn");
    let pending_file = cairn_dir.join(".pending");

    // Load state
    let state = cairn::state::RepositoryState::load(&cairn_dir)
        .context("Failed to load repository state")?;

    // Scan for changes (no persistent cache - recompute each time)
    let scan_result = cairn::snapshot::scan_working_directory(&state, None)
        .context("Failed to scan working directory")?;

    // Check if there are any changes
    let has_changes = !scan_result.added.is_empty()
        || !scan_result.modified.is_empty()
        || !scan_result.deleted.is_empty();

    if !has_changes {
        // No changes
        return Ok(false);
    }

    // Store scan result for commit after successful build
    let pending_data =
        bincode::serialize(&scan_result).context("Failed to serialize pending snapshot")?;
    std::fs::write(&pending_file, pending_data).context("Failed to write pending snapshot")?;

    Ok(true)
}

/// Scan tracked paths and return all files with their content
fn scan_tracked_files(tracked_paths: &[PathBuf]) -> Result<std::collections::HashMap<PathBuf, Vec<u8>>> {
    let mut files = std::collections::HashMap::new();

    for tracked_path in tracked_paths {
        if tracked_path.is_file() {
            let content = std::fs::read(tracked_path)?;
            files.insert(tracked_path.clone(), content);
        } else if tracked_path.is_dir() {
            scan_directory(tracked_path, &mut files)?;
        }
    }

    Ok(files)
}

/// Recursively scan a directory for files
fn scan_directory(dir: &Path, files: &mut std::collections::HashMap<PathBuf, Vec<u8>>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let dir_name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" {
                continue;
            }
            scan_directory(&path, files)?;
        } else if path.is_file() {
            let content = std::fs::read(&path)?;
            files.insert(path, content);
        }
    }

    Ok(())
}

/// Commit the pending snapshot as a real patch using Git-style architecture
fn save_pending_snapshot() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");
    let pending_file = cairn_dir.join(".pending");

    if !pending_file.exists() {
        return Ok(()); // Nothing to commit
    }

    // Read the pending scan result (used to detect if snapshot needed)
    let pending_data = std::fs::read(&pending_file).context("Failed to read pending snapshot")?;
    let _scan_result: cairn::snapshot::ScanResult =
        bincode::deserialize(&pending_data).context("Failed to deserialize pending snapshot")?;

    // Load current state to get parent commit (if exists)
    let mut state = cairn::state::RepositoryState::load(&cairn_dir)
        .context("Failed to load repository state")?;

    // Get parent patch hash (None for first patch)
    let parent_patch_hp = if let Some(head) = state.latest() {
        // Decode base58 head to Blake3Hash
        let parent_bytes = bs58::decode(head)
            .into_vec()
            .context("Failed to decode parent patch hash")?;
        if parent_bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&parent_bytes);
            Some(arr)
        } else {
            None
        }
    } else {
        None
    };

    // 1. Load parent tree if exists (needed for selective blob storage)
    let parent_tree = if let Some(parent_hp) = parent_patch_hp {
        let parent_patch = cairn::patch_storage::load_patch(&cairn_dir, &parent_hp)
            .context("Failed to load parent patch")?;
        Some(cairn::tree::load_tree(&cairn_dir, &parent_patch.tree_hp)
            .context("Failed to load parent tree")?)
    } else {
        None
    };

    // 2. Scan all files from tracked paths (handles directories)
    let all_files = scan_tracked_files(&state.tracked_paths)?;

    // 3. Selective blob storage: store full blobs only for new files or chain resets
    let mut file_to_blob = std::collections::HashMap::new();
    let mut file_diffs_data = std::collections::HashMap::new(); // Store diff info while building

    for (path, new_content) in all_files {

        if let Some(ref parent_tree_map) = parent_tree {
            if let Some(old_blob_hash) = parent_tree_map.get(&path) {
                // Check if file actually changed by comparing content hashes
                let new_content_hash = blake3::hash(&new_content);
                let new_hash_bytes = *new_content_hash.as_bytes();

                if new_hash_bytes == *old_blob_hash {
                    // File UNCHANGED - just reference existing blob, no diff entry
                    file_to_blob.insert(path.clone(), *old_blob_hash);
                    // Don't add to file_diffs_data - unchanged files omitted from diffs
                } else {
                    // File MODIFIED - create diff entry
                    let old_content = cairn::blob::load_blob(&cairn_dir, old_blob_hash)
                        .context("Failed to load old blob")?;
                    let diff_ops = cairn::diff::compute_byte_level_diff(&old_content, &new_content);

                    let chain_size = compute_chain_size(&cairn_dir, &path,
                        &parent_patch_hp.expect("parent_hp should exist here"))
                        .unwrap_or(0);

                    if chain_size < new_content.len() {
                        // Use diff strategy - DON'T store new blob, use content hash
                        file_to_blob.insert(path.clone(), new_hash_bytes);

                        file_diffs_data.insert(path.clone(), (
                            cairn::patch_storage::DiffStrategy::Diff,
                            *old_blob_hash,
                            new_hash_bytes,
                            diff_ops
                        ));
                    } else {
                        // Chain reset - store new blob
                        let new_blob_hash = cairn::blob::store_blob(&new_content)
                            .context("Failed to store blob")?;
                        file_to_blob.insert(path.clone(), new_blob_hash);

                        file_diffs_data.insert(path.clone(), (
                            cairn::patch_storage::DiffStrategy::NewBlob,
                            *old_blob_hash,
                            new_blob_hash,
                            vec![] // No diff ops for NewBlob strategy
                        ));
                    }
                }
            } else {
                // NEW file (not in parent) - store full blob as base
                let new_blob_hash = cairn::blob::store_blob(&new_content)
                    .context("Failed to store blob")?;
                file_to_blob.insert(path.clone(), new_blob_hash);

                file_diffs_data.insert(path.clone(), (
                    cairn::patch_storage::DiffStrategy::Diff,
                    [0u8; 32], // No old blob
                    new_blob_hash,
                    vec![cairn::patch::ByteOp::Insert { content: new_content }]
                ));
            }
        } else {
            // No parent - store all files as full blobs
            let new_blob_hash = cairn::blob::store_blob(&new_content)
                .context("Failed to store blob")?;
            file_to_blob.insert(path.clone(), new_blob_hash);
        }
    }

    // 3. Create tree from blob references (virtual or real)
    let tree_hp = cairn::tree::create_tree(&file_to_blob)
        .context("Failed to create tree")?;

    // 4. Build file_diffs from collected data
    let file_diffs = if parent_patch_hp.is_some() {
        let mut diffs = std::collections::HashMap::new();

        for (path, (strategy, old_blob, new_blob, ops)) in file_diffs_data {
            let file_diff = cairn::patch_storage::FileDiff {
                strategy,
                old_blob,
                new_blob,
                ops,
            };
            diffs.insert(path, file_diff);
        }

        Some(diffs)
    } else {
        None // First patch - no parent, no diffs
    };

    // 4. Create patch with tree reference and diffs
    let patch_info = cairn::patch_storage::PatchInfo {
        message: "Successful build".to_string(),
        tree_hp,
        parent_patch_hp,
        file_diffs,
    };

    let patch_hp = cairn::patch_storage::create_patch(patch_info)
        .context("Failed to create patch")?;

    // 4. Update state with new commit (use base58 encoding for consistency)
    let patch_hash_str = bs58::encode(&patch_hp).into_string();

    // For snapshot hash, use the tree's hp (directory structure snapshot)
    state.add_patch(patch_hash_str, tree_hp);
    state.save(&cairn_dir).context("Failed to save state")?;

    // Remove pending file
    std::fs::remove_file(&pending_file).context("Failed to remove pending snapshot")?;

    Ok(())
}

/// Discard the pending snapshot
fn discard_pending_snapshot() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");
    let pending_file = cairn_dir.join(".pending");

    if pending_file.exists() {
        std::fs::remove_file(&pending_file).context("Failed to remove pending snapshot")?;
    }

    Ok(())
}

/// Compute cumulative diff chain size for a file
fn compute_chain_size(
    cairn_dir: &Path,
    file_path: &Path,
    parent_hp: &[u8; 32],
) -> Result<usize> {
    let mut chain_size = 0;
    let mut current_hp = Some(*parent_hp);

    while let Some(hp) = current_hp {
        let patch = cairn::patch_storage::load_patch(cairn_dir, &hp)?;

        if let Some(diffs) = &patch.file_diffs {
            if let Some(diff) = diffs.get(file_path) {
                match diff.strategy {
                    cairn::patch_storage::DiffStrategy::Diff => {
                        chain_size += estimate_diff_size(&diff.ops);
                    }
                    cairn::patch_storage::DiffStrategy::NewBlob => {
                        break; // Chain reset
                    }
                }
            }
        }
        current_hp = patch.parent_patch_hp;
    }

    Ok(chain_size)
}

/// Estimate storage size of diff operations
fn estimate_diff_size(ops: &[cairn::patch::ByteOp]) -> usize {
    ops.iter().map(|op| match op {
        cairn::patch::ByteOp::Copy { .. } => 8, // Overhead for copy operation
        cairn::patch::ByteOp::Insert { content } => content.len()
    }).sum()
}

/// Auto-initialize cairn repository with flat directory structure
fn auto_init(cairn_dir: &PathBuf) -> Result<()> {
    use std::fs;

    // Create .cairn directory structure (flattened)
    fs::create_dir(cairn_dir).context("Failed to create .cairn directory")?;
    fs::create_dir(cairn_dir.join("blobs")).context("Failed to create blobs directory")?;
    fs::create_dir(cairn_dir.join("trees")).context("Failed to create trees directory")?;
    fs::create_dir(cairn_dir.join("patches")).context("Failed to create patches directory")?;

    // Create initial empty state
    let initial_state = cairn::state::RepositoryState::new();
    initial_state
        .save(cairn_dir)
        .context("Failed to save initial state")?;

    Ok(())
}
