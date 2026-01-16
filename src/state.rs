//! Repository state management
//!
//! Tracks the current patch and file tree state.
//! State is persisted to `.cairn/state.vsf` using VSF encoding.

use anyhow::{Result, anyhow};
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

    /// Latest snapshot hash (complete workspace state in VSF)
    /// This is the provenance hash of the snapshot VSF file
    pub latest_snapshot: Blake3Hash,
}

impl RepositoryState {
    /// Create empty initial state
    pub fn new() -> Self {
        Self {
            head: String::new(),
            patches: Vec::new(),
            latest_snapshot: [0u8; 32],
        }
    }

    /// Encode state to VSF format
    ///
    /// VSF structure:
    /// - metadata section: current patch ID + latest snapshot hash
    /// - patches section: chronological list of patch IDs
    pub fn encode_vsf(&self) -> Result<Vec<u8>> {
        // 1. Metadata section
        let mut metadata_section = VsfSection::new("metadata");
        metadata_section.add_field("current", VsfType::l(self.head.clone()));
        metadata_section.add_field("snapshot", VsfType::hp(self.latest_snapshot.to_vec()));

        // 2. Patches section (insertion order)
        let mut patches_section = VsfSection::new("patches");
        for (idx, patch_id) in self.patches.iter().enumerate() {
            patches_section.add_field(&format!("p{}", idx), VsfType::l(patch_id.clone()));
        }

        // 3. Build VSF file
        let builder = VsfBuilder::new()
            .add_section_direct(metadata_section)
            .add_section_direct(patches_section);

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
        let mut latest_snapshot = [0u8; 32];

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
                    if let Some(field) = section.get_field("current") {
                        if let Some(VsfType::l(s)) = field.values.first() {
                            head = s.clone();
                        }
                    }
                    if let Some(field) = section.get_field("snapshot") {
                        if let Some(hash) = extract_hash(field.values.first().unwrap()) {
                            latest_snapshot = hash;
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
                _ => {
                    // Unknown section, skip
                }
            }
        }

        Ok(RepositoryState {
            head,
            patches,
            latest_snapshot,
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
    pub fn add_patch(&mut self, patch_id: String, snapshot_hash: Blake3Hash) {
        self.patches.push(patch_id.clone());
        self.head = patch_id;
        self.latest_snapshot = snapshot_hash;
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
        let snapshot_hash = *blake3::hash(b"fake snapshot content").as_bytes();

        state.add_patch(patch_id.clone(), snapshot_hash);

        assert!(!state.is_empty());
        assert_eq!(state.head, patch_id);
        assert_eq!(state.latest(), Some(&patch_id));
        assert_eq!(state.patches.len(), 1);
        assert_eq!(state.latest_snapshot, snapshot_hash);
    }

    #[test]
    fn test_roundtrip_empty() {
        let original = RepositoryState::new();
        let encoded = original.encode_vsf().unwrap();
        let decoded = RepositoryState::decode_vsf(&encoded).unwrap();

        assert_eq!(decoded.head, original.head);
        assert_eq!(decoded.patches, original.patches);
        assert_eq!(decoded.latest_snapshot, original.latest_snapshot);
    }

    #[test]
    fn test_roundtrip_with_data() {
        let mut state = RepositoryState::new();

        // Add first patch
        let snapshot1 = *blake3::hash(b"snapshot 1 content").as_bytes();
        state.add_patch("patch1".to_string(), snapshot1);

        // Add second patch
        let snapshot2 = *blake3::hash(b"snapshot 2 content").as_bytes();
        state.add_patch("patch2".to_string(), snapshot2);

        // Roundtrip
        let encoded = state.encode_vsf().unwrap();
        let decoded = RepositoryState::decode_vsf(&encoded).unwrap();

        assert_eq!(decoded.head, "patch2");
        assert_eq!(decoded.patches, vec!["patch1", "patch2"]);
        assert_eq!(decoded.latest_snapshot, snapshot2);
    }

    #[test]
    fn test_insertion_order_preserved() {
        let mut state = RepositoryState::new();

        for i in 0..10 {
            let patch_id = format!("patch{}", i);
            let snapshot = *blake3::hash(format!("snapshot {}", i).as_bytes()).as_bytes();
            state.add_patch(patch_id, snapshot);
        }

        let encoded = state.encode_vsf().unwrap();
        let decoded = RepositoryState::decode_vsf(&encoded).unwrap();

        // Verify chronological order preserved
        for i in 0..10 {
            assert_eq!(decoded.patches[i], format!("patch{}", i));
        }
    }
}
