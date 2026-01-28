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
/// Create a snapshot from current working directory (for compatibility)
///
/// This is a legacy function that creates a complete snapshot.
/// New code should use the incremental scan_working_directory + create_snapshot_from_files.
pub fn create_snapshot(cairn_dir: &PathBuf, message: String, build_hash: Hash) -> Result<String> {
    // Load state
    let state = crate::state::RepositoryState::load(cairn_dir)
        .context("Failed to load repository state")?;

    // Scan for changes
    let scan_result = scan_working_directory(&state)
        .context("Failed to scan working directory")?;

    // Combine all files (added + modified) for snapshot
    let mut all_files = scan_result.added.clone();
    all_files.extend(scan_result.modified.clone());

    // Create snapshot from those files
    create_snapshot_from_files(cairn_dir, message, build_hash, all_files)
}

/// Scan working directory for all files
///
/// Respects .gitignore patterns and automatically excludes:
/// - .cairn/ directory
/// - target/ directory
/// - .git/ directory
/// - Hidden files/directories (starting with .)
/// - All patterns in .gitignore
/// Result of scanning working directory for changes
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanResult {
    pub added: HashMap<PathBuf, Vec<u8>>,      // New files with content
    pub modified: HashMap<PathBuf, Vec<u8>>,   // Changed files with new content
    pub deleted: Vec<PathBuf>,                 // Removed files
    pub updated_cache: HashMap<PathBuf, crate::state::FileInfo>,  // Updated file cache
}

/// Scan working directory for changes (incremental with mtime/size/hash checking)
///
/// Only scans explicitly tracked paths (from state.tracked_paths).
/// Only reads files that have potentially changed based on mtime+size comparison.
/// Returns lists of added, modified, and deleted files for efficient patch creation.
pub fn scan_working_directory(state: &crate::state::RepositoryState) -> Result<ScanResult> {
    let current_dir = std::env::current_dir().context("Failed to get current directory")?;
    let mut added = HashMap::new();
    let mut modified = HashMap::new();
    let mut updated_cache = HashMap::new();
    let mut seen_paths = std::collections::HashSet::new();
    let mut file_count = 0;
    const MAX_FILES: usize = 10000; // Safety limit

    // Only scan explicitly tracked paths
    for tracked_path in &state.tracked_paths {
        let full_path = current_dir.join(tracked_path);

        if !full_path.exists() {
            continue; // Tracked path doesn't exist (yet)
        }

        if full_path.is_file() {
            // Tracked path is a single file
            scan_file(
                &full_path,
                &current_dir,
                state,
                &mut added,
                &mut modified,
                &mut updated_cache,
                &mut seen_paths,
            )?;
            file_count += 1;
        } else if full_path.is_dir() {
            // Tracked path is a directory - walk it recursively
            for entry in walkdir::WalkDir::new(&full_path)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();

                // Only process files (not directories)
                if !path.is_file() {
                    continue;
                }

                file_count += 1;

                if file_count > MAX_FILES {
                    return Err(anyhow::anyhow!(
                        "Too many files encountered (>{}) - possible directory explosion. \
                         Check your tracked paths or use .gitignore to exclude large directories.",
                        MAX_FILES
                    ));
                }

                scan_file(
                    path,
                    &current_dir,
                    state,
                    &mut added,
                    &mut modified,
                    &mut updated_cache,
                    &mut seen_paths,
                )?;
            }
        }
    }

    // Find deleted files (in cache but not on disk)
    let deleted: Vec<PathBuf> = state
        .files
        .keys()
        .filter(|path| !seen_paths.contains(*path))
        .cloned()
        .collect();

    Ok(ScanResult {
        added,
        modified,
        deleted,
        updated_cache,
    })
}

/// Scan a single file and update tracking data
fn scan_file(
    path: &std::path::Path,
    current_dir: &std::path::Path,
    state: &crate::state::RepositoryState,
    added: &mut HashMap<PathBuf, Vec<u8>>,
    modified: &mut HashMap<PathBuf, Vec<u8>>,
    updated_cache: &mut HashMap<PathBuf, crate::state::FileInfo>,
    seen_paths: &mut std::collections::HashSet<PathBuf>,
) -> Result<()> {
    use std::time::SystemTime;

    // Get relative path
    let relative_path = path
        .strip_prefix(current_dir)
        .context("Path should be under current directory")?
        .to_path_buf();

    seen_paths.insert(relative_path.clone());

    // Get file metadata (fast - no read)
    let metadata = fs::metadata(path)
        .with_context(|| format!("Failed to get metadata for: {:?}", path))?;

    let size = metadata.len();
    let mtime = metadata.modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Check cache
    if let Some(cached) = state.files.get(&relative_path) {
        // File exists in cache - check if potentially changed
        if cached.mtime == mtime && cached.size == size {
            // mtime+size unchanged - definitely not modified, skip read
            updated_cache.insert(relative_path.clone(), cached.clone());
            return Ok(());
        }

        // mtime or size changed - read and hash to confirm
        let content = fs::read(path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;
        let content_hash = *blake3::hash(&content).as_bytes();

        if content_hash == cached.content_hash {
            // Content actually unchanged - update mtime/size in cache
            updated_cache.insert(
                relative_path,
                crate::state::FileInfo {
                    mtime,
                    size,
                    content_hash,
                },
            );
        } else {
            // Content changed - file modified
            modified.insert(relative_path.clone(), content);
            updated_cache.insert(
                relative_path,
                crate::state::FileInfo {
                    mtime,
                    size,
                    content_hash,
                },
            );
        }
    } else {
        // File not in cache - new file
        let content = fs::read(path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;
        let content_hash = *blake3::hash(&content).as_bytes();

        added.insert(relative_path.clone(), content);
        updated_cache.insert(
            relative_path,
            crate::state::FileInfo {
                mtime,
                size,
                content_hash,
            },
        );
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

    #[test]
    fn test_scan_result_structure() {
        // Simple test to verify ScanResult structure
        let scan_result = ScanResult {
            added: HashMap::new(),
            modified: HashMap::new(),
            deleted: Vec::new(),
            updated_cache: HashMap::new(),
        };

        assert_eq!(scan_result.added.len(), 0);
        assert_eq!(scan_result.modified.len(), 0);
        assert_eq!(scan_result.deleted.len(), 0);
    }

    #[test]
    fn test_state_tracked_paths_default() {
        // Verify default tracked paths
        let state = crate::state::RepositoryState::new();
        assert_eq!(state.tracked_paths, vec![PathBuf::from("src"), PathBuf::from("Cargo.toml")]);
    }

    #[test]
    fn test_scan_respects_tracked_paths() {
        // This test runs in the actual cairn project directory
        // which has src/ and Cargo.toml, so it should find files
        let state = crate::state::RepositoryState::new();
        let result = scan_working_directory(&state).unwrap();

        // The cairn project has src/ and Cargo.toml (tracked by default)
        // So we should find at least those tracked paths
        // We don't assert exact counts since the project structure may change

        // Verify we got some added files (first scan has no cache)
        assert!(result.added.len() > 0, "Should find files in tracked paths");

        // Verify we're only finding files in src/ or Cargo.toml
        for path in result.added.keys() {
            let path_str = path.to_string_lossy();
            assert!(
                path_str.starts_with("src") || path_str == "Cargo.toml",
                "Found file outside tracked paths: {:?}",
                path
            );
        }
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
