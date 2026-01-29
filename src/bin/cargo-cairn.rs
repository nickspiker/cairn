//! cargo-cairn - Cargo wrapper that automatically creates patches on successful builds
//!
//! Usage: cargo cairn <cargo-command> [args...]
//!
//! This wraps any cargo command and creates a cairn patch if it succeeds.
//! The snapshot is taken BEFORE the build starts, ensuring it matches exactly
//! what cargo compiled.

use anyhow::{Context, Result};
use std::path::PathBuf;
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

    // If build succeeded and we took a snapshot, commit it
    if status.success() && snapshot_taken {
        if let Err(e) = commit_pending_snapshot() {
            eprintln!("⚠️  Cairn: Failed to commit snapshot: {}", e);
        } else {
            println!("✓ Cairn: Patch created");
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

/// Commit the pending snapshot as a real patch using Git-style architecture
fn commit_pending_snapshot() -> Result<()> {
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

    // Get parent commit hash (None for first commit)
    let parent_commit_hp = if let Some(head) = state.latest() {
        // Decode base58 head to Blake3Hash
        let parent_bytes = bs58::decode(head)
            .into_vec()
            .context("Failed to decode parent commit hash")?;
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

    // 1. Scan tracked files and store blobs
    let file_to_blob = cairn::blob::scan_and_store_blobs(&state.tracked_paths)
        .context("Failed to scan and store blobs")?;

    // 2. Create tree from blob references
    let tree_hp = cairn::tree::create_tree(&file_to_blob)
        .context("Failed to create tree")?;

    // 3. Compute diffs from parent (if exists)
    let file_diffs = if let Some(parent_hp) = parent_commit_hp {
        // Load parent commit and tree
        let parent_commit = cairn::patch_storage::load_commit(&parent_hp)
            .context("Failed to load parent commit")?;
        let parent_tree = cairn::tree::load_tree(&parent_commit.tree_hp)
            .context("Failed to load parent tree")?;

        let mut diffs = std::collections::HashMap::new();

        // For each file in current tree, check if it changed
        for (path, new_blob_hash) in &file_to_blob {
            if let Some(old_blob_hash) = parent_tree.get(path) {
                // File exists in parent - check if content changed
                if old_blob_hash != new_blob_hash {
                    // File modified - compute byte-level diff
                    let old_content = cairn::blob::load_blob(old_blob_hash)
                        .context("Failed to load old blob")?;
                    let new_content = cairn::blob::load_blob(new_blob_hash)
                        .context("Failed to load new blob")?;

                    let diff_ops = cairn::diff::compute_byte_level_diff(&old_content, &new_content);

                    // TODO: Implement chain optimization using should_create_new_snapshot
                    // For now, always use diff strategy
                    let file_diff = cairn::patch_storage::FileDiff {
                        strategy: cairn::patch_storage::DiffStrategy::Diff,
                        old_blob: *old_blob_hash,
                        new_blob: *new_blob_hash,
                        ops: diff_ops,
                    };
                    diffs.insert(path.clone(), file_diff);
                }
                // If hashes same, no diff needed (file unchanged)
            } else {
                // File added (not in parent) - store full content as Insert
                let new_content = cairn::blob::load_blob(new_blob_hash)
                    .context("Failed to load new blob")?;
                let file_diff = cairn::patch_storage::FileDiff {
                    strategy: cairn::patch_storage::DiffStrategy::Diff,
                    old_blob: [0u8; 32],  // No old blob for new file
                    new_blob: *new_blob_hash,
                    ops: vec![cairn::patch::ByteOp::Insert {
                        content: new_content,
                    }],
                };
                diffs.insert(path.clone(), file_diff);
            }
        }

        // Note: Deleted files (in parent but not current) are represented
        // by absence from current tree - no explicit diff needed

        Some(diffs)
    } else {
        None // First commit - no parent, no diffs
    };

    // 4. Create commit with tree reference and diffs
    let build_hash = *blake3::hash(b"cargo-cairn build").as_bytes();
    let commit_info = cairn::patch_storage::CommitInfo {
        message: "Successful build".to_string(),
        build_hash,
        tree_hp,
        parent_commit_hp,
        file_diffs,
    };

    let commit_hp = cairn::patch_storage::create_commit(commit_info)
        .context("Failed to create commit")?;

    // 4. Update state with new commit (use base58 encoding for consistency)
    let commit_hash_str = bs58::encode(&commit_hp).into_string();

    // For snapshot hash, use the tree's hp (directory structure snapshot)
    state.add_patch(commit_hash_str, tree_hp);
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
