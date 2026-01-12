//! Repository state management
//!
//! Tracks the current patch head and file tree state.
//! State is persisted to `.cairn/state.vsf` using VSF encoding.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use vsf::{VsfBuilder, VsfSection, VsfType};

/// BLAKE3 hash type alias (32 bytes)
pub type Blake3Hash = [u8; 32];

/// Current repository state
///
/// Persisted to .cairn/state.vsf
#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryState {
    /// Current patch ID (VSF provenance hash, base64url encoded)
    pub head: String,

    /// Patch history in insertion order (chronological)
    /// patches[0] = first build, patches[n-1] = latest
    pub patches: Vec<String>,

    /// Current file tree state (path → BLAKE3 hash)
    /// Used for hardlink optimization during rollback
    pub files: HashMap<PathBuf, Blake3Hash>,
}

impl RepositoryState {
    /// Create empty initial state
    pub fn new() -> Self {
        Self {
            head: String::new(),
            patches: Vec::new(),
            files: HashMap::new(),
        }
    }

    /// Encode state to VSF format
    ///
    /// VSF structure:
    /// - metadata section: head patch ID
    /// - patches section: chronological list of patch IDs
    /// - files section: file paths with BLAKE3 hashes
    pub fn encode_vsf(&self) -> Result<Vec<u8>> {
        // 1. Metadata section
        let mut metadata_section = VsfSection::new("metadata");
        metadata_section.add_field("head", VsfType::l(self.head.clone()));

        // 2. Patches section (insertion order)
        let mut patches_section = VsfSection::new("patches");
        for (idx, patch_id) in self.patches.iter().enumerate() {
            patches_section.add_field(&format!("p{}", idx), VsfType::l(patch_id.clone()));
        }

        // 3. Files section (path → hash)
        let mut files_section = VsfSection::new("files");
        for (idx, (path, hash)) in self.files.iter().enumerate() {
            files_section.add_field_multi(
                &format!("f{}", idx),
                vec![
                    VsfType::l(path.to_string_lossy().to_string()),
                    VsfType::hp(hash.to_vec()),
                ],
            );
        }

        // 4. Build VSF file
        let builder = VsfBuilder::new()
            .add_section_direct(metadata_section)
            .add_section_direct(patches_section)
            .add_section_direct(files_section);

        builder.build().map_err(|e| anyhow!(e))
    }

    /// Decode state from VSF format
    pub fn decode_vsf(bytes: &[u8]) -> Result<Self> {
        use vsf::VsfHeader;

        // 1. Decode header
        let (header, _header_len) =
            VsfHeader::decode(bytes).map_err(|e| anyhow!("Failed to decode VSF header: {}", e))?;

        // 2. Parse sections
        let mut head = String::new();
        let mut patches = Vec::new();
        let mut files = HashMap::new();

        for field in &header.fields {
            // Skip empty sections
            if field.size_bytes == 0 {
                continue;
            }

            let mut ptr = field.offset_bytes;
            if ptr >= bytes.len() {
                continue;
            }

            let section = vsf::VsfSection::parse(bytes, &mut ptr)
                .map_err(|e| anyhow!("Failed to parse section '{}': {}", field.name, e))?;

            match section.name.as_str() {
                "metadata" => {
                    if let Some(field) = section.get_field("head") {
                        if let Some(VsfType::l(s)) = field.values.first() {
                            head = s.clone();
                        }
                    }
                }
                "patches" => {
                    // Patches stored as p0, p1, p2, ... in insertion order
                    let mut patch_fields: Vec<_> = section.fields.iter().collect();
                    patch_fields.sort_by_key(|f| {
                        f.name
                            .strip_prefix('p')
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(usize::MAX)
                    });

                    for field in patch_fields {
                        if let Some(VsfType::l(patch_id)) = field.values.first() {
                            patches.push(patch_id.clone());
                        }
                    }
                }
                "files" => {
                    // Files stored as f0, f1, f2, ... with [path, hash] values
                    for field in &section.fields {
                        if field.values.len() >= 2 {
                            // Extract path from first value
                            if let Some(VsfType::l(path_str)) = field.values.first() {
                                let path = PathBuf::from(path_str);
                                // Extract hash from second value
                                if let Some(hash) = extract_hash(&field.values[1]) {
                                    files.insert(path, hash);
                                }
                            }
                        }
                    }
                }
                _ => {
                    // Unknown section, skip
                }
            }
        }

        Ok(RepositoryState {
            head,
            patches,
            files,
        })
    }

    /// Save state to .cairn/state.vsf
    pub fn save(&self, cairn_dir: &PathBuf) -> Result<()> {
        let state_path = cairn_dir.join("state.vsf");
        let bytes = self.encode_vsf()?;
        std::fs::write(&state_path, bytes)?;
        Ok(())
    }

    /// Load state from .cairn/state.vsf
    pub fn load(cairn_dir: &PathBuf) -> Result<Self> {
        let state_path = cairn_dir.join("state.vsf");
        if !state_path.exists() {
            return Ok(Self::new());
        }
        let bytes = std::fs::read(&state_path)?;
        Self::decode_vsf(&bytes)
    }

    /// Add a new patch to history
    pub fn add_patch(&mut self, patch_id: String, new_files: HashMap<PathBuf, Blake3Hash>) {
        self.patches.push(patch_id.clone());
        self.head = patch_id;
        self.files = new_files;
    }

    /// Get the latest patch ID
    pub fn latest(&self) -> Option<&String> {
        self.patches.last()
    }

    /// Check if repository has any patches
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }
}

impl Default for RepositoryState {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract BLAKE3 hash from VsfType
fn extract_hash(value: &VsfType) -> Option<Blake3Hash> {
    match value {
        VsfType::hp(bytes) | VsfType::hb(bytes) => {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(bytes);
                Some(arr)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_state() {
        let state = RepositoryState::new();
        assert!(state.is_empty());
        assert!(state.latest().is_none());
        assert_eq!(state.head, "");
    }

    #[test]
    fn test_add_patch() {
        let mut state = RepositoryState::new();

        let patch_id = "a3f8d9e1".to_string();
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("src/main.rs"),
            *blake3::hash(b"fn main() {}").as_bytes(),
        );

        state.add_patch(patch_id.clone(), files);

        assert!(!state.is_empty());
        assert_eq!(state.head, patch_id);
        assert_eq!(state.latest(), Some(&patch_id));
        assert_eq!(state.patches.len(), 1);
        assert_eq!(state.files.len(), 1);
    }

    #[test]
    fn test_roundtrip_empty() {
        let original = RepositoryState::new();
        let encoded = original.encode_vsf().unwrap();
        let decoded = RepositoryState::decode_vsf(&encoded).unwrap();

        assert_eq!(decoded.head, original.head);
        assert_eq!(decoded.patches, original.patches);
        assert_eq!(decoded.files, original.files);
    }

    #[test]
    fn test_roundtrip_with_data() {
        let mut state = RepositoryState::new();

        // Add first patch
        let mut files1 = HashMap::new();
        files1.insert(
            PathBuf::from("src/main.rs"),
            *blake3::hash(b"fn main() {}").as_bytes(),
        );
        state.add_patch("patch1".to_string(), files1.clone());

        // Add second patch
        let mut files2 = HashMap::new();
        files2.insert(
            PathBuf::from("src/main.rs"),
            *blake3::hash(b"fn main() { println!(\"hi\"); }").as_bytes(),
        );
        files2.insert(
            PathBuf::from("src/lib.rs"),
            *blake3::hash(b"pub fn test() {}").as_bytes(),
        );
        state.add_patch("patch2".to_string(), files2);

        // Roundtrip
        let encoded = state.encode_vsf().unwrap();
        let decoded = RepositoryState::decode_vsf(&encoded).unwrap();

        assert_eq!(decoded.head, "patch2");
        assert_eq!(decoded.patches, vec!["patch1", "patch2"]);
        assert_eq!(decoded.files.len(), 2);
        assert!(decoded.files.contains_key(&PathBuf::from("src/main.rs")));
        assert!(decoded.files.contains_key(&PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn test_insertion_order_preserved() {
        let mut state = RepositoryState::new();

        for i in 0..10 {
            let patch_id = format!("patch{}", i);
            state.add_patch(patch_id, HashMap::new());
        }

        let encoded = state.encode_vsf().unwrap();
        let decoded = RepositoryState::decode_vsf(&encoded).unwrap();

        // Verify chronological order preserved
        for i in 0..10 {
            assert_eq!(decoded.patches[i], format!("patch{}", i));
        }
    }
}
