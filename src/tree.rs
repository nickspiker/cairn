//! Tree storage - directory structure with blob references
//!
//! Trees are VSF documents in the vault mapping file paths to blob hashes (hb).
//! Each tree is identified by its provenance hash (hp), computed from the sorted blob
//! hashes (pure content-based dedup).
//!
//! Paths are normalized before encoding: relative to the project root, forward-slash
//! separated, no `..`/absolute components. That single canonical spelling is what gets
//! hex-encoded into VSF labels, so trees round-trip identically across operating systems.

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use vsf::{VsfBuilder, VsfSection, VsfType};

use crate::state::Blake3Hash;
use crate::vault::{CairnVault, tree_key};

/// Canonicalize a repository-relative path to its stored spelling: forward slashes, no
/// leading `./`, and no absolute/parent/prefix components (those are structural errors —
/// a tree must never reach outside the project root).
pub fn normalize_rel_path(path: &Path) -> Result<String> {
    let mut parts: Vec<&str> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(p) => parts.push(
                p.to_str()
                    .ok_or_else(|| anyhow!("Path contains invalid UTF-8: {:?}", path))?,
            ),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!("Absolute path not allowed in tree: {:?}", path));
            }
            Component::ParentDir => {
                return Err(anyhow!("Parent (..) component not allowed in tree: {:?}", path));
            }
        }
    }
    if parts.is_empty() {
        return Err(anyhow!("Empty path not allowed in tree"));
    }
    Ok(parts.join("/"))
}

/// Encode a normalized path as a VSF label (hex, `f_` prefix — only 0-9a-f, always valid).
fn path_label(path: &Path) -> Result<String> {
    Ok(format!("f_{}", hex::encode(normalize_rel_path(path)?.as_bytes())))
}

/// Decode a VSF label back to a path.
fn label_path(label: &str) -> Result<PathBuf> {
    let encoded = label
        .strip_prefix("f_")
        .ok_or_else(|| anyhow!("Invalid file label: {}", label))?;
    let decoded = hex::decode(encoded).map_err(|e| anyhow!("Failed to decode file label: {e}"))?;
    let path_str =
        String::from_utf8(decoded).map_err(|e| anyhow!("Invalid UTF-8 in decoded path: {e}"))?;
    Ok(PathBuf::from(path_str))
}

/// Create a tree from file→blob mappings
///
/// Returns the tree's provenance hash (hp), computed from the sorted blob hashes.
/// Identical content produces identical hashes; an existing tree is not rewritten.
pub fn create_tree(
    vault: &mut CairnVault,
    file_to_blob: &HashMap<PathBuf, Blake3Hash>,
) -> Result<Blake3Hash> {
    // 1. Content hash from sorted blob hashes ONLY (no paths) — pure content dedup.
    let mut hasher = blake3::Hasher::new();
    let mut sorted_hashes: Vec<_> = file_to_blob.values().copied().collect();
    sorted_hashes.sort();
    for blob_hash in &sorted_hashes {
        hasher.update(blob_hash);
    }
    let content_hash = *hasher.finalize().as_bytes();

    let key = tree_key(&content_hash);
    if vault.exists(&key)? {
        return Ok(content_hash);
    }

    // 2. Normalize paths; warn about case-insensitive collisions (macOS/Windows checkouts
    //    of such a tree would silently merge the files).
    let mut sorted_entries: Vec<(String, &Blake3Hash)> = file_to_blob
        .iter()
        .map(|(path, hash)| Ok((normalize_rel_path(path)?, hash)))
        .collect::<Result<_>>()?;
    sorted_entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut folded: HashMap<String, &str> = HashMap::new();
    for (path, _) in &sorted_entries {
        if let Some(other) = folded.insert(path.to_lowercase(), path) {
            eprintln!(
                "⚠️  Cairn: paths '{other}' and '{path}' differ only by case — they will collide on case-insensitive filesystems"
            );
        }
    }

    // 3. Build and store the tree VSF.
    let mut builder = VsfBuilder::new();
    let mut files = VsfSection::new("files");
    for (path, blob_hash) in sorted_entries {
        files.add_field(
            &format!("f_{}", hex::encode(path.as_bytes())),
            VsfType::hb(blob_hash.to_vec()),
        );
    }
    builder = builder.add_section_direct(files);
    let tree_bytes = builder.build().map_err(|e| anyhow!("{}", e))?;

    vault.put(&key, &tree_bytes).context("Failed to store tree")?;
    Ok(content_hash)
}

/// Load a tree by its provenance hash
pub fn load_tree(
    vault: &mut CairnVault,
    hp: &Blake3Hash,
) -> Result<HashMap<PathBuf, Blake3Hash>> {
    let bytes = vault.get(&tree_key(hp))?.ok_or_else(|| {
        anyhow!("Tree not found: {}", crate::hash_encoding::base58_encode(hp))
    })?;

    let (header, _) = vsf::VsfHeader::decode(&bytes)
        .map_err(|e| anyhow!("Failed to decode tree VSF header: {}", e))?;

    let files_field = header
        .fields
        .iter()
        .find(|f| f.name == "files")
        .context("Tree missing 'files' section")?;

    if files_field.size_bytes == 0 {
        return Ok(HashMap::new());
    }

    let mut ptr = files_field.offset_bytes;
    let files_section = VsfSection::parse(&bytes, &mut ptr)
        .map_err(|e| anyhow!("Failed to parse files section: {}", e))?;

    let mut file_to_blob = HashMap::new();
    for field in &files_section.fields {
        let path = label_path(&field.name)?;
        if let Some(VsfType::hb(blob_hash_vec)) = field.values.first() {
            if blob_hash_vec.len() == 32 {
                let mut blob_hash = [0u8; 32];
                blob_hash.copy_from_slice(blob_hash_vec);
                file_to_blob.insert(path, blob_hash);
            } else {
                return Err(anyhow!(
                    "Invalid blob hash length for {}: {} bytes",
                    field.name,
                    blob_hash_vec.len()
                ));
            }
        } else {
            return Err(anyhow!("Missing or invalid blob hash for file: {}", field.name));
        }
    }

    Ok(file_to_blob)
}

/// Check if a tree exists
pub fn tree_exists(vault: &mut CairnVault, hp: &Blake3Hash) -> bool {
    vault.exists(&tree_key(hp)).unwrap_or(false)
}

/// Encode a path for the diffs field in patches (same label scheme as trees).
pub fn path_to_hex_label(path: &Path) -> Result<String> {
    path_label(path)
}

/// Decode a diffs-field label back to a path.
pub fn hex_label_to_path(label: &str) -> Result<PathBuf> {
    label_path(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::store_blob;
    use tempfile::TempDir;

    fn vault(dir: &TempDir) -> CairnVault {
        CairnVault::open(&dir.path().join(".cairn")).unwrap()
    }

    #[test]
    fn test_create_and_load_tree() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let blob1 = store_blob(&mut v, b"fn main() {}")?;
        let blob2 = store_blob(&mut v, b"fn test() {}")?;

        let mut file_to_blob = HashMap::new();
        file_to_blob.insert(PathBuf::from("src/main.rs"), blob1);
        file_to_blob.insert(PathBuf::from("src/lib.rs"), blob2);

        let tree_hp = create_tree(&mut v, &file_to_blob)?;
        assert!(tree_exists(&mut v, &tree_hp));

        let loaded = load_tree(&mut v, &tree_hp)?;
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(&PathBuf::from("src/main.rs")), Some(&blob1));
        assert_eq!(loaded.get(&PathBuf::from("src/lib.rs")), Some(&blob2));
        Ok(())
    }

    #[test]
    fn test_tree_content_based_hashing() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let blob_hash = store_blob(&mut v, b"test content")?;
        let mut file_to_blob = HashMap::new();
        file_to_blob.insert(PathBuf::from("test.txt"), blob_hash);

        let tree1_hp = create_tree(&mut v, &file_to_blob)?;
        let tree2_hp = create_tree(&mut v, &file_to_blob)?;
        assert_eq!(tree1_hp, tree2_hp);

        let loaded = load_tree(&mut v, &tree1_hp)?;
        assert_eq!(loaded.get(&PathBuf::from("test.txt")), Some(&blob_hash));
        Ok(())
    }

    #[test]
    fn test_empty_tree() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let tree_hp = create_tree(&mut v, &HashMap::new())?;
        let loaded = load_tree(&mut v, &tree_hp)?;
        assert_eq!(loaded.len(), 0);
        Ok(())
    }

    #[test]
    fn test_tree_with_complex_paths() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = vault(&dir);

        let blob1 = store_blob(&mut v, b"content1")?;
        let blob2 = store_blob(&mut v, b"content2")?;
        let blob3 = store_blob(&mut v, b"content3")?;

        let mut file_to_blob = HashMap::new();
        file_to_blob.insert(PathBuf::from("src/nested/deep/file.rs"), blob1);
        file_to_blob.insert(PathBuf::from("Cargo.toml"), blob2);
        file_to_blob.insert(PathBuf::from("tests/integration_test.rs"), blob3);

        let tree_hp = create_tree(&mut v, &file_to_blob)?;
        let loaded = load_tree(&mut v, &tree_hp)?;

        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.get(&PathBuf::from("src/nested/deep/file.rs")), Some(&blob1));
        assert_eq!(loaded.get(&PathBuf::from("Cargo.toml")), Some(&blob2));
        assert_eq!(loaded.get(&PathBuf::from("tests/integration_test.rs")), Some(&blob3));
        Ok(())
    }

    #[test]
    fn test_tree_nonexistent() {
        let dir = TempDir::new().unwrap();
        let mut v = vault(&dir);
        assert!(!tree_exists(&mut v, &[0u8; 32]));
    }

    #[test]
    fn test_path_normalization() {
        assert_eq!(normalize_rel_path(Path::new("./src/main.rs")).unwrap(), "src/main.rs");
        assert!(normalize_rel_path(Path::new("/abs/path")).is_err());
        assert!(normalize_rel_path(Path::new("a/../b")).is_err());
        assert!(normalize_rel_path(Path::new("")).is_err());
    }
}
