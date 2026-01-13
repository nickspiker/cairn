//! cargo-cairn - Cargo wrapper that automatically creates patches on successful builds
//!
//! Usage: cargo cairn <cargo-command> [args...]
//!
//! This wraps any cargo command and creates a cairn patch if it succeeds.
//! The snapshot is taken BEFORE the build starts, ensuring it matches exactly
//! what cargo compiled.

use anyhow::{Context, Result};
use std::process::{Command, exit};
use std::path::PathBuf;

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

    // Check if there are any changes
    let state = cairn::state::RepositoryState::load(&cairn_dir)
        .context("Failed to load repository state")?;

    // Scan current files
    let current_files = cairn::snapshot::scan_working_directory()
        .context("Failed to scan working directory")?;

    // Get previous files
    let previous_files = cairn::snapshot::get_previous_files(&state, &cairn_dir)?;

    // Compute diff
    let operations = cairn::diff::compute_diff(&cairn_dir, &previous_files, &current_files)
        .context("Failed to compute diff")?;

    if operations.is_empty() {
        // No changes
        return Ok(false);
    }

    // Store the current file state in pending file
    let pending_data = bincode::serialize(&current_files)
        .context("Failed to serialize pending snapshot")?;
    std::fs::write(&pending_file, pending_data)
        .context("Failed to write pending snapshot")?;

    Ok(true)
}

/// Commit the pending snapshot as a real patch
fn commit_pending_snapshot() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");
    let pending_file = cairn_dir.join(".pending");

    if !pending_file.exists() {
        return Ok(()); // Nothing to commit
    }

    // Read the pending snapshot
    let pending_data = std::fs::read(&pending_file)
        .context("Failed to read pending snapshot")?;
    let snapshot_files: std::collections::HashMap<PathBuf, Vec<u8>> = bincode::deserialize(&pending_data)
        .context("Failed to deserialize pending snapshot")?;

    // Create the actual patch
    let build_hash = blake3::hash(b"cargo-cairn build"); // TODO: Use actual build output hash
    let message = "Successful build".to_string();

    // Directly create the patch using the snapshot we took before the build
    cairn::snapshot::create_snapshot_from_files(&cairn_dir, message, build_hash, snapshot_files)
        .context("Failed to create patch")?;

    // Remove pending file
    std::fs::remove_file(&pending_file)
        .context("Failed to remove pending snapshot")?;

    Ok(())
}

/// Discard the pending snapshot
fn discard_pending_snapshot() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");
    let pending_file = cairn_dir.join(".pending");

    if pending_file.exists() {
        std::fs::remove_file(&pending_file)
            .context("Failed to remove pending snapshot")?;
    }

    Ok(())
}

/// Auto-initialize cairn repository
fn auto_init(cairn_dir: &PathBuf) -> Result<()> {
    use std::fs;

    // Create .cairn directory structure
    fs::create_dir(cairn_dir).context("Failed to create .cairn directory")?;
    fs::create_dir(cairn_dir.join("patches")).context("Failed to create patches directory")?;
    fs::create_dir(cairn_dir.join("blobs")).context("Failed to create blobs directory")?;

    // Create initial empty state
    let initial_state = cairn::state::RepositoryState::new();
    initial_state.save(cairn_dir).context("Failed to save initial state")?;

    Ok(())
}
