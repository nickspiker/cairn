//! Patch creation - the heart of cairn
//!
//! Creates patches by:
//! 1. Scanning working directory for tracked files
//! 2. Computing deltas against previous state
//! 3. Encoding patch with VSF
//! 4. Updating repository state

use crate::diff;
use crate::patch::{Patch, get_author_id};
use crate::snapshot_vsf;
use crate::state::RepositoryState;
use anyhow::{Context, Result};
use blake3::Hash;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use vsf::types::eagle_time;
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
    let mut repo_state =
        RepositoryState::load(cairn_dir).context("Failed to load repository state")?;

    // Create snapshot VSF for current state (new snapshot)
    let new_snapshot = snapshot_vsf::create_snapshot(&current_files, cairn_dir)
        .context("Failed to create new snapshot VSF")?;

    // Get old snapshot hash from repository state
    let old_snapshot = if repo_state.is_empty() {
        // No previous snapshot - create empty one
        let empty_files = HashMap::new();
        snapshot_vsf::create_snapshot(&empty_files, cairn_dir)
            .context("Failed to create empty initial snapshot")?
    } else {
        repo_state.latest_snapshot
    };

    // Compute delta operations on x-encoded snapshots
    let operations = diff::compute_diff(cairn_dir, &old_snapshot, &new_snapshot)
        .context("Failed to compute delta on snapshots")?;

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
        let parent_hash: [u8; 32] = parent_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Parent patch ID must be 32 bytes"))?;
        Some(parent_hash)
    };

    let patch = Patch::new(
        get_author_id(),
        parent,
        timestamp as usize,
        message,
        operations,
        *build_hash.as_bytes(),
    );

    // Encode patch to VSF
    let patch_bytes = patch.encode_vsf().context("Failed to encode patch")?;

    // Extract VSF provenance hash and encode as base64url for filename
    let provenance_hash = compute_provenance_hash(&patch_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to compute provenance hash: {}", e))?;
    let patch_id = base64_url_encode(&provenance_hash);

    // Save patch file
    let patch_path = cairn_dir.join("patches").join(&patch_id);
    fs::write(&patch_path, patch_bytes)
        .with_context(|| format!("Failed to write patch file: {:?}", patch_path))?;

    // Update repository state with new snapshot hash (already created above)
    repo_state.add_patch(patch_id.clone(), new_snapshot);
    repo_state
        .save(cairn_dir)
        .context("Failed to save repository state")?;

    Ok(patch_id)
}

/// Create a patch from the current working directory
///
/// This is the main entry point called after a successful build.
/// Returns the patch ID of the newly created patch.
pub fn create_snapshot(cairn_dir: &PathBuf, message: String, build_hash: Hash) -> Result<String> {
    // Scan working directory for files
    let current_files = scan_working_directory().context("Failed to scan working directory")?;

    // Create snapshot from those files
    create_snapshot_from_files(cairn_dir, message, build_hash, current_files)
}

/// Scan working directory for all files
///
/// Respects .gitignore patterns and automatically excludes:
/// - .cairn/ directory
/// - target/ directory
/// - .git/ directory
/// - Hidden files/directories (starting with .)
/// - All patterns in .gitignore
pub fn scan_working_directory() -> Result<HashMap<PathBuf, Vec<u8>>> {
    use ignore::WalkBuilder;

    let mut files = HashMap::new();
    let current_dir = std::env::current_dir().context("Failed to get current directory")?;

    // Build walker that respects .gitignore
    let walker = WalkBuilder::new(&current_dir)
        .hidden(true) // Skip hidden files/dirs (starting with .)
        .git_ignore(true) // Respect .gitignore
        .git_exclude(true) // Respect .git/info/exclude
        .require_git(false) // Don't require git repo
        .add_custom_ignore_filename(".cairnignore") // Support .cairnignore too
        .build();

    for result in walker {
        let entry = result.context("Failed to read directory entry")?;
        let path = entry.path();

        // Only process files (not directories)
        if !path.is_file() {
            continue;
        }

        // Always skip .cairn directory explicitly
        if path.starts_with(&current_dir.join(".cairn")) {
            continue;
        }

        // Skip lock files and build artifacts
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if file_name == "Cargo.lock"
            || file_name == "package-lock.json"
            || file_name.ends_with(".vsix")
            || file_name.ends_with(".wasm")
        {
            continue;
        }

        // Read file content
        let content = fs::read(path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;

        // Store with relative path
        let relative_path = path
            .strip_prefix(&current_dir)
            .context("Path should be under current directory")?
            .to_path_buf();

        files.insert(relative_path, content);
    }

    Ok(files)
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

    // Get the snapshot hash from the latest patch in the repo state
    let latest_snapshot_hash = &repo_state.latest_snapshot;

    // Read all files from the snapshot VSF
    read_all_files_from_snapshot(cairn_dir, latest_snapshot_hash)
}

/// Read all files from a snapshot VSF
fn read_all_files_from_snapshot(
    cairn_dir: &PathBuf,
    snapshot_hash: &[u8; 32],
) -> Result<HashMap<PathBuf, Vec<u8>>> {
    let snapshot_path = cairn_dir
        .join("snapshots")
        .join(format!("{}.vsf", hex::encode(snapshot_hash)));

    let bytes = fs::read(&snapshot_path).context(format!(
        "Failed to load snapshot: {}",
        hex::encode(snapshot_hash)
    ))?;

    // Parse VSF header
    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow::anyhow!("Failed to decode VSF header: {}", e))?;

    // Find the "files" section
    let files_field = header
        .fields
        .iter()
        .find(|f| f.name == "files")
        .context("Snapshot missing 'files' section")?;

    // Parse the files section
    let mut ptr = files_field.offset_bytes;
    let files_section = vsf::file_format::VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow::anyhow!("Failed to parse files section: {}", e))?;

    // Recursively extract all files from the section tree
    let mut files = HashMap::new();
    extract_files_recursive(&files_section, &PathBuf::new(), &mut files)?;

    Ok(files)
}

/// Recursively extract files from nested VSF sections
fn extract_files_recursive(
    section: &vsf::file_format::VsfSection,
    current_path: &PathBuf,
    files: &mut HashMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    // Check if this section has a "content" field (it's a file)
    if let Some(content_field) = section.get_field("content") {
        if let Some(value) = content_field.values.first() {
            let content = match value {
                vsf::VsfType::x(text) => text.as_bytes().to_vec(),
                vsf::VsfType::v(b'b', bytes) => bytes.clone(),
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unexpected content type in section: {:?}",
                        section.name
                    ));
                }
            };

            // Denormalize just the filename (last component) since the directory path
            // is already built up correctly in current_path
            let denormalized_path = if let Some(parent) = current_path.parent() {
                parent.join(denormalize_name(&section.name))
            } else {
                PathBuf::from(denormalize_name(&section.name))
            };
            files.insert(denormalized_path, content);
        }

        // File sections shouldn't have subsections - skip recursion to avoid path doubling
        return Ok(());
    }

    // This is a directory section - recurse into subdirectories and files
    for subsection in &section.subsections {
        let subsection_name = denormalize_name(&subsection.name);
        let subsection_path = current_path.join(subsection_name);
        extract_files_recursive(subsection, &subsection_path, files)?;
    }

    Ok(())
}

/// Denormalize a VSF-compliant name back to original form
/// Reverses the normalize_name transformation
fn denormalize_name(name: &str) -> String {
    // Find the last underscore that separates base from extension
    if let Some(last_underscore) = name.rfind('_') {
        let (base, ext) = name.split_at(last_underscore);
        // If the extension looks like a file extension (2-5 chars), restore the dot
        let ext_part = &ext[1..]; // Skip the underscore
        if ext_part.len() >= 2
            && ext_part.len() <= 5
            && ext_part.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return format!("{}.{}", base, ext_part);
        }
    }
    // No valid extension found, just return as-is
    name.to_string()
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
        assert_eq!(
            files.get(&PathBuf::from("test.txt")),
            Some(&b"content".to_vec())
        );
        assert_eq!(
            files.get(&PathBuf::from("subdir/nested.txt")),
            Some(&b"nested".to_vec())
        );
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
        assert_eq!(
            files.get(&PathBuf::from("test.txt")),
            Some(&b"content".to_vec())
        );
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
