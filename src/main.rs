//! Cairn - Patch-based version control for successful cargo builds
//!
//! Cairn automatically creates patches of your code whenever `cargo build` succeeds.
//! Each patch is stored in VSF-encoded format with:
//! - Cryptographic verification (BLAKE3 hashes)
//! - Minimal storage (line-based diffs)
//! - Easy rollback to any successful build state

mod blob;
mod daemon;
mod decode;
mod diff;
mod encode;
mod hash_encoding;
mod jump;
mod mnemonic;
mod patch;
mod patch_storage;
mod reconstruct;
mod snapshot;
mod snapshot_vsf;
mod state;
mod suffix_array;
mod tree;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cairn")]
#[command(about = "Patch-based version control for successful cargo builds and things with tacos")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all patches
    List,

    /// Show details of a specific patch
    Show {
        /// Patch ID (5-word mnemonic or full base64url hash)
        patch_id: String,
    },

    /// Rollback to a previous patch
    Rollback {
        /// Patch ID (5-word mnemonic or full base64url hash)
        patch_id: String,
    },

    /// Switch to a different patch (navigate patch history)
    Jump {
        /// Patch ID (5-word mnemonic or full base64url hash)
        patch_id: String,
    },

    /// Delete the .cairn directory and all patch history
    Clear,

    /// Start the Cairn daemon for VSCode extension IPC
    Daemon,

    /// Print the current patch ID (for tooling)
    #[command(hide = true)]
    Current,

    /// Create a patch manually (for testing)
    #[command(hide = true)]
    Snapshot {
        /// Patch message
        #[arg(short, long, default_value = "Manual patch")]
        message: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::List => {
            cmd_list()?;
        }
        Commands::Show { patch_id } => {
            cmd_show(&patch_id)?;
        }
        Commands::Rollback { patch_id } => {
            cmd_rollback(&patch_id)?;
        }
        Commands::Jump { patch_id } => {
            cmd_jump(&patch_id)?;
        }
        Commands::Clear => {
            cmd_clear()?;
        }
        Commands::Daemon => {
            daemon::run_daemon()?;
        }
        Commands::Current => {
            cmd_current()?;
        }
        Commands::Snapshot { message } => {
            cmd_snapshot(&message)?;
        }
    }

    Ok(())
}

fn cmd_list() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if initialized
    if !cairn_dir.exists() {
        return Err(anyhow!(
            "Not a cairn repository (no .cairn directory found)"
        ));
    }

    // Load repository state
    let repo_state =
        state::RepositoryState::load(&cairn_dir).context("Failed to load repository state")?;

    if repo_state.is_empty() {
        println!("No patches yet - build your project to create the first one");
        return Ok(());
    }

    // List all patches in chronological order
    println!("Patches (newest first):");
    println!();

    for (idx, patch_id) in repo_state.patches.iter().enumerate().rev() {
        let is_current = patch_id == &repo_state.head;
        let is_newest = idx == repo_state.patches.len() - 1;

        let marker = match (is_current, is_newest) {
            (true, true) => " (CURRENT, NEWEST)",
            (true, false) => " (CURRENT)",
            (false, true) => " (NEWEST)",
            (false, false) => "",
        };

        // Convert to mnemonic (5 words = ~58 bits)
        let mnemonic = mnemonic::patch_id_to_mnemonic(patch_id, 5)
            .unwrap_or_else(|_| format!("{}...", &patch_id[..16]));

        // Show index (0 = oldest, n-1 = newest)
        println!("  #{:<3} {}{}", idx, mnemonic, marker);

        // TODO: Load patch and show metadata (message, timestamp, file count)
    }

    Ok(())
}

fn cmd_show(patch_id: &str) -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if initialized
    if !cairn_dir.exists() {
        return Err(anyhow!(
            "Not a cairn repository (no .cairn directory found!!)"
        ));
    }

    // Load repository state
    let repo_state =
        state::RepositoryState::load(&cairn_dir).context("Failed to load repository state")?;

    // Find patch by prefix match (supports both mnemonic and base64url)
    let full_patch_id = find_patch_by_id(&repo_state.patches, patch_id)?;

    // Parse patch hash
    let patch_bytes = hash_encoding::base58_decode(&full_patch_id)?;
    if patch_bytes.len() != 32 {
        anyhow::bail!("Invalid patch hash length: expected 32 bytes, got {}", patch_bytes.len());
    }
    let mut patch_hp = [0u8; 32];
    patch_hp.copy_from_slice(&patch_bytes);

    // Load the patch using new system
    let patch = patch_storage::load_patch(&cairn_dir, &patch_hp)
        .context("Failed to load patch")?;

    // Display patch information with mnemonic
    let mnemonic = mnemonic::patch_id_to_mnemonic(&full_patch_id, 5)
        .unwrap_or_else(|_| format!("{}...", &full_patch_id[..16]));

    println!("Patch: {} ({})", mnemonic, &full_patch_id[..16]);
    println!("Message: {}", patch.message);
    println!("Tree: {}", hash_encoding::base58_encode(&patch.tree_hp));
    if let Some(parent) = patch.parent_patch_hp {
        println!("Parent: {}", hash_encoding::base58_encode(&parent));
    }
    println!();

    // Show file changes count
    if let Some(diffs) = &patch.file_diffs {
        println!("File changes: {}", diffs.len());
    } else {
        println!("File changes: (initial patch - no diffs)");
    }

    // TODO: Show summary of file changes
    // - Added files
    // - Deleted files
    // - Modified files (with line counts)

    Ok(())
}

fn cmd_rollback(patch_id: &str) -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if initialized
    if !cairn_dir.exists() {
        return Err(anyhow!(
            "Not a cairn repository (no .cairn directory found)"
        ));
    }

    // Load repository state
    let repo_state =
        state::RepositoryState::load(&cairn_dir).context("Failed to load repository state")?;

    // Find patch by prefix match (supports both mnemonic and base64url)
    let target_patch_id = find_patch_by_id(&repo_state.patches, patch_id)?;

    if target_patch_id == repo_state.head {
        let mnemonic = mnemonic::patch_id_to_mnemonic(&target_patch_id, 5)
            .unwrap_or_else(|_| format!("{}...", &target_patch_id[..16]));
        println!("Already at patch {}", mnemonic);
        return Ok(());
    }

    // TODO: Implement rollback with snapshot-based approach
    // For now, rollback is temporarily disabled until we implement
    // proper snapshot-per-patch storage in state

    return Err(anyhow!(
        "Rollback temporarily disabled with fish tacos - being refactored for snapshot-based storage"
    ));
}

fn cmd_jump(patch_id: &str) -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if initialized
    if !cairn_dir.exists() {
        return Err(anyhow!(
            "Not a cairn repository (no .cairn directory found)"
        ));
    }

    // Load repository state
    let repo_state =
        state::RepositoryState::load(&cairn_dir).context("Failed to load repository state")?;

    // Find patch by prefix match (supports both mnemonic and base64url)
    let target_patch_id = find_patch_by_id(&repo_state.patches, patch_id)?;

    // Check if already at this patch
    if target_patch_id == repo_state.head {
        let mnemonic = mnemonic::patch_id_to_mnemonic(&target_patch_id, 5)
            .unwrap_or_else(|_| format!("{}...", &target_patch_id[..16]));
        println!("Already at patch {}", mnemonic);
        return Ok(());
    }

    // Jump to the patch
    println!("Switching to patch {}...", &target_patch_id[..8]);
    jump::jump_to_patch(&cairn_dir, &target_patch_id)?;

    Ok(())
}

fn cmd_clear() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if .cairn exists
    if !cairn_dir.exists() {
        println!("No .cairn directory found - nothing to clear");
        return Ok(());
    }

    // Warn the user
    eprintln!("⚠️  WARNING: This will DELETE the .cairn directory and ALL patch history!");
    eprintln!("   This action cannot be undone.");
    eprintln!();
    eprint!("Are you sure you want to continue? (y/N): ");

    use std::io::{self, Write};
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let response = input.trim().to_lowercase();
    if response != "y" && response != "yes" {
        println!("Aborted.");
        return Ok(());
    }

    println!("Removing .cairn directory...");
    fs::remove_dir_all(&cairn_dir).context("Failed to remove .cairn directory")?;

    println!("✓ Cleared cairn repository");
    Ok(())
}

fn cmd_current() -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if initialized
    if !cairn_dir.exists() {
        return Err(anyhow!(
            "Not a cairn repository (no .cairn directory found)"
        ));
    }

    // Load repository state
    let repo_state =
        state::RepositoryState::load(&cairn_dir).context("Failed to load repository state")?;

    if repo_state.head.is_empty() {
        return Err(anyhow!("No current patch - repository is empty"));
    }

    // Output just the patch ID (for tooling)
    println!("{}", repo_state.head);
    Ok(())
}

fn cmd_snapshot(message: &str) -> Result<()> {
    let cairn_dir = PathBuf::from(".cairn");

    // Check if initialized
    if !cairn_dir.exists() {
        return Err(anyhow!(
            "Not a cairn repository (no .cairn directory found)"
        ));
    }

    // Create fake build hash (in real implementation, this comes from cargo build output)
    let _build_hash = blake3::hash(b"test build");

    println!("Creating patch...");

    let patch_id = snapshot::create_snapshot(&cairn_dir, message.to_string())
        .context("Failed to create patch")?;

    let mnemonic = mnemonic::patch_id_to_mnemonic(&patch_id, 5)
        .unwrap_or_else(|_| format!("{}...", &patch_id[..16]));
    println!("✓ Created patch {}", mnemonic);

    Ok(())
}

/// Find a patch by ID (supports both full 5-word mnemonic and full base64url hash)
fn find_patch_by_id(patches: &[String], id: &str) -> Result<String> {
    // Try exact base64url match first
    if patches.contains(&id.to_string()) {
        return Ok(id.to_string());
    }

    // Try exact 5-word mnemonic match
    let normalized = id.replace(' ', "-");
    for patch_id in patches {
        let mnemonic = mnemonic::patch_id_to_mnemonic(patch_id, 5)?;
        if mnemonic == normalized {
            return Ok(patch_id.clone());
        }
    }

    Err(anyhow!(
        "Patch not found: {}. Use full 5-word mnemonic or full base64url hash.",
        id
    ))
}
