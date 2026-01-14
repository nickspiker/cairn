//! Patch creation - the heart of cairn
//!
//! Creates patches by:
//! 1. Scanning working directory for tracked files
//! 2. Computing deltas against previous state
//! 3. Encoding patch with VSF
//! 4. Updating repository state

use crate::diff;
use crate::patch::{Patch, get_author_id};
use crate::state::RepositoryState;
use anyhow::{Context, Result};
use blake3::Hash;
use std::collections::HashMap;
use std::fs;
use vsf::types::eagle_time;
use std::path::PathBuf;
use vsf::verification::compute_provenance_hash;

/// Create a patch from pre-captured file state
///
/// This is used by cargo-cairn to create a patch from files that were
/// captured BEFORE the build started, ensuring the patch matches exactly
/// what was compiled.
pub fn create_snapshot_from_files(
    cairn_dir: &PathBuf,
    message: String,
    build_hash: Hash,
    current_files: HashMap<PathBuf, Vec<u8>>,
) -> Result<String> {
    // Load current repository state
    let mut repo_state = RepositoryState::load(cairn_dir)
        .context("Failed to load repository state")?;

    // Get previous state from repository
    let previous_files = get_previous_files(&repo_state, cairn_dir)?;

    // Compute delta operations
    let operations = diff::compute_diff(cairn_dir, &previous_files, &current_files)
        .context("Failed to compute delta")?;

    if operations.is_empty() {
        println!("No changes detected - skipping patch");
        return Ok(repo_state.head.clone());
    }

    // Create patch - get current Eagle Time as oscillation count
    let timestamp = eagle_time::eagle_time_oscillations();
    let parent = if repo_state.is_empty() {
        None
    } else {
        // Decode parent patch ID from base64url to get the raw hash
        use base64::Engine;
        let parent_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&repo_state.head)
            .context("Failed to decode parent patch ID")?;
        let parent_hash: [u8; 32] = parent_bytes.try_into()
            .map_err(|_| anyhow::anyhow!("Parent patch ID must be 32 bytes"))?;
        Some(parent_hash)
    };

    let patch = Patch::new(
        get_author_id(),
        parent,
        timestamp,
        message,
        operations,
        *build_hash.as_bytes(),
    );

    // Encode patch to VSF
    let patch_bytes = patch.encode_vsf()
        .context("Failed to encode patch")?;

    // Extract VSF provenance hash and encode as base64url for filename
    let provenance_hash = compute_provenance_hash(&patch_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to compute provenance hash: {}", e))?;
    let patch_id = base64_url_encode(&provenance_hash);

    // Save patch file
    let patch_path = cairn_dir.join("patches").join(&patch_id);
    fs::write(&patch_path, patch_bytes)
        .with_context(|| format!("Failed to write patch file: {:?}", patch_path))?;

    // Compute file hashes for state
    let file_hashes = current_files
        .iter()
        .map(|(path, content)| (path.clone(), *blake3::hash(content).as_bytes()))
        .collect();

    // Update repository state
    repo_state.add_patch(patch_id.clone(), file_hashes);
    repo_state.save(cairn_dir)
        .context("Failed to save repository state")?;

    Ok(patch_id)
}

/// Create a patch from the current working directory
///
/// This is the main entry point called after a successful build.
/// Returns the patch ID of the newly created patch.
pub fn create_snapshot(
    cairn_dir: &PathBuf,
    message: String,
    build_hash: Hash,
) -> Result<String> {
    // Scan working directory for files
    let current_files = scan_working_directory()
        .context("Failed to scan working directory")?;

    // Create snapshot from those files
    create_snapshot_from_files(cairn_dir, message, build_hash, current_files)
}

/// Scan working directory for all files (excluding .cairn, target, .git)
pub fn scan_working_directory() -> Result<HashMap<PathBuf, Vec<u8>>> {
    let mut files = HashMap::new();

    // Get current directory
    let current_dir = std::env::current_dir()
        .context("Failed to get current directory")?;

    // Walk directory tree
    scan_directory(&current_dir, &current_dir.clone(), &mut files)?;

    Ok(files)
}

/// Recursively scan a directory, collecting all files
fn scan_directory(
    base_dir: &PathBuf,
    current_dir: &PathBuf,
    files: &mut HashMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    for entry in fs::read_dir(current_dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();

        // Skip excluded directories
        if path.is_dir() {
            let name = file_name.to_string_lossy();
            if name == ".cairn"
                || name == "target"
                || name == ".git"
                || name == "node_modules"
                || name == "out"
                || name == "dist"
                || name.starts_with('.')
            {
                continue;
            }

            // Recurse into subdirectory
            scan_directory(base_dir, &path.to_path_buf(), files)?;
        } else if path.is_file() {
            // Skip hidden files and build artifacts
            let name = file_name.to_string_lossy();
            if name.starts_with('.')
                || name.ends_with(".vsix")
                || name.ends_with(".wasm")
                || name == "package-lock.json"
                || name == "Cargo.lock"
            {
                continue;
            }

            // Read file content
            let content = fs::read(&path)
                .with_context(|| format!("Failed to read file: {:?}", path))?;

            // Store with relative path from base_dir
            let relative_path = path.strip_prefix(base_dir)
                .expect("Path should be under base_dir")
                .to_path_buf();

            files.insert(relative_path, content);
        }
    }

    Ok(())
}

/// Get the file tree from the previous patch
pub fn get_previous_files(
    repo_state: &RepositoryState,
    cairn_dir: &PathBuf,
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    if repo_state.is_empty() {
        // No previous patch - return empty state
        return Ok(HashMap::new());
    }

    // Reconstruct file tree by applying all patches up to CURRENT
    let mut current_files = HashMap::new();

    for patch_id in &repo_state.patches {
        let patch_path = cairn_dir.join("patches").join(patch_id);
        let patch_bytes = fs::read(&patch_path)
            .with_context(|| format!("Failed to read patch: {:?}", patch_path))?;

        let patch = crate::patch::Patch::decode_vsf(&patch_bytes)
            .context("Failed to decode patch")?;

        current_files = crate::apply::apply_operations(cairn_dir, &current_files, &patch.operations)?;
    }

    Ok(current_files)
}

/// Base64 URL-safe encoding (no padding)
fn base64_url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let files = scan_working_directory().unwrap();
        assert_eq!(files.len(), 0);
    }

    #[test]
    fn test_scan_with_files() {
        let temp_dir = TempDir::new().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // Create test files
        fs::write(temp_dir.path().join("test.txt"), b"content").unwrap();
        fs::create_dir(temp_dir.path().join("subdir")).unwrap();
        fs::write(temp_dir.path().join("subdir/nested.txt"), b"nested").unwrap();

        let files = scan_working_directory().unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files.get(&PathBuf::from("test.txt")), Some(&b"content".to_vec()));
        assert_eq!(files.get(&PathBuf::from("subdir/nested.txt")), Some(&b"nested".to_vec()));
    }

    #[test]
    fn test_scan_excludes_cairn_dir() {
        let temp_dir = TempDir::new().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // Create .cairn directory with files
        fs::create_dir(temp_dir.path().join(".cairn")).unwrap();
        fs::write(temp_dir.path().join(".cairn/state.vsf"), b"state").unwrap();

        // Create normal file
        fs::write(temp_dir.path().join("test.txt"), b"content").unwrap();

        let files = scan_working_directory().unwrap();

        // Should only have test.txt, not .cairn/state.vsf
        // (Allow for hidden files that might exist in temp_dir)
        assert!(files.contains_key(&PathBuf::from("test.txt")));
        assert!(!files.contains_key(&PathBuf::from(".cairn/state.vsf")));
        assert_eq!(files.get(&PathBuf::from("test.txt")), Some(&b"content".to_vec()));
    }

    #[test]
    fn test_base64_url_encode() {
        let input = b"hello world";
        let encoded = base64_url_encode(input);

        // Should be URL-safe (no +, /, =)
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));
    }

}
