//! Patch creation - the heart of cairn (blob-based architecture)
//!
//! Creates patches by:
//! 1. Storing each file as a blob in .cairn/blobs/
//! 2. Creating a tree mapping paths → blob hashes
//! 3. Creating a patch referencing the tree
//! 4. Updating repository state

use crate::blob;
use crate::hash_encoding::base58_encode;
use crate::patch_storage::{PatchInfo, create_patch};
use crate::state::RepositoryState;
use crate::tree;
use anyhow::{Context, Result};
use blake3::Hash;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Create a patch from pre-captured file state (blob-based architecture)
///
/// This is used by cargo-cairn to create a patch from files that were
/// captured BEFORE the build started, ensuring the patch matches exactly
/// what was compiled.
///
/// Workflow:
/// 1. Store each file as a blob in .cairn/blobs/
/// 2. Create a tree mapping paths → blob hashes
/// 3. Create a patch referencing the tree
pub fn create_snapshot_from_files(
    cairn_dir: &PathBuf,
    message: String,
    build_hash: Hash,
    current_files: HashMap<PathBuf, Vec<u8>>,
) -> Result<String> {
    // Load current repository state
    let mut repo_state =
        RepositoryState::load(cairn_dir).context("Failed to load repository state")?;

    // 1. Store all files as blobs and get their hashes
    let mut file_to_blob = HashMap::new();
    for (path, content) in &current_files {
        let blob_hash = blob::store_blob(content)
            .with_context(|| format!("Failed to store blob for {:?}", path))?;
        file_to_blob.insert(path.clone(), blob_hash);
    }

    // 2. Create tree from file→blob mappings
    let tree_hp = tree::create_tree(&file_to_blob)
        .context("Failed to create tree")?;

    // 3. Get parent commit hash (if exists)
    let parent_patch_hp = if repo_state.is_empty() {
        None
    } else {
        // Decode parent patch ID from base58 to get the raw hash
        let parent_bytes = crate::hash_encoding::base58_decode(&repo_state.head)
            .context("Failed to decode parent patch ID")?;
        if parent_bytes.len() != 32 {
            anyhow::bail!("Parent patch ID must be 32 bytes");
        }
        let mut parent_hash = [0u8; 32];
        parent_hash.copy_from_slice(&parent_bytes);
        Some(parent_hash)
    };

    // 4. Check if there are any changes (compare tree hashes)
    if let Some(parent_hp) = parent_patch_hp {
        let parent_patch = crate::patch_storage::load_patch(cairn_dir, &parent_hp)
            .context("Failed to load parent patch")?;

        if parent_patch.tree_hp == tree_hp {
            println!("No changes detected - skipping patch");
            return Ok(repo_state.head.clone());
        }
    }

    // 5. Create patch with tree reference
    let patch_info = PatchInfo {
        message,
        build_hash: *build_hash.as_bytes(),
        tree_hp,
        parent_patch_hp,
        file_diffs: None,  // TODO: Compute diffs for space optimization
    };

    let patch_hp = create_patch(patch_info)
        .context("Failed to create patch")?;

    // 6. Encode patch hash as base58 for the patch ID
    let patch_id = base58_encode(&patch_hp);

    // 7. Update repository state with new patch and tree hash
    repo_state.add_patch(patch_id.clone(), tree_hp);
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

    // Scan for changes (no cache on initial snapshot)
    let scan_result = scan_working_directory(&state, None)
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
/// Scan working directory for changes
///
/// The cache parameter is an optional in-memory cache from a previous scan.
/// If provided, it enables fast mtime+size checks to avoid re-hashing unchanged files.
pub fn scan_working_directory(
    state: &crate::state::RepositoryState,
    cache: Option<&crate::state::BuildCache>,
) -> Result<ScanResult> {
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
                cache,
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
                    cache,
                    &mut added,
                    &mut modified,
                    &mut updated_cache,
                    &mut seen_paths,
                )?;
            }
        }
    }

    // Find deleted files (in cache but not on disk)
    let deleted: Vec<PathBuf> = if let Some(cache) = cache {
        cache
            .files
            .keys()
            .filter(|path| !seen_paths.contains(*path))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

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
    _state: &crate::state::RepositoryState,
    cache: Option<&crate::state::BuildCache>,
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

    // Check cache (if provided)
    if let Some(cache) = cache {
        if let Some(cached) = cache.files.get(&relative_path) {
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
            return Ok(());
        }
    }

    // File not in cache (or no cache provided) - treat as new file
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
        let result = scan_working_directory(&state, None).unwrap();

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

}
