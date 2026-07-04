//! The cairn vault — all repository objects in one mirrored manifestus store.
//!
//! Replaces the loose-file layout (blobs/, trees/, patches/, state.vsf, .pending) with a
//! single crash-proof KV engine: `.cairn/vault` plus `.cairn/vault.shadow`, dual-ring
//! mirrored, every block BLAKE3-sealed, every write commit-gated. No hash-named files on
//! disk means no base58/case-sensitivity portability hazards, no check-then-write races,
//! and no torn state.vsf — a crash mid-save simply leaves the previous committed
//! generation as the head.
//!
//! Addressing: 32-byte keys derived with BLAKE3 KDF contexts per object domain
//! (blob/tree/patch/state/pending). Object identity stays the content hash cairn already
//! uses; the KDF only namespaces domains so a source file whose bytes happen to equal a
//! tree's encoding can never collide with it.
//!
//! Concurrency: one exclusive lock file per repository, taken for the lifetime of the
//! open handle. Operations are short (snapshot save, jump); concurrent `cargo cairn`
//! invocations queue on the lock rather than corrupting each other.

use anyhow::{Context, Result, anyhow};
use manifestus::{FileDev, HOST_RING_LOG2, Mirror, Vault, verified_replicate};
use std::fs;
use std::path::{Path, PathBuf};

use crate::state::Blake3Hash;

/// Genesis size: ring (256 blocks) + initial tract. 1280 × 4KB = 5MB per mirror file.
/// Deliberately small — growth is one fallocate + commit, handled automatically on demand.
const INITIAL_BLOCKS: u64 = (1 << HOST_RING_LOG2) + 1024;

/// How long a second cairn process waits on the repository lock before giving up.
const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub struct CairnVault {
    vault: Vault<FileDev, FileDev>,
    /// Held for the lifetime of the handle; the OS releases it if we die.
    _lock: fs::File,
}

// ============================================================================ key schema

fn derive(context: &str, input: &[u8]) -> [u8; 32] {
    blake3::derive_key(context, input)
}

/// Blob address: content hash, domain-separated.
pub fn blob_key(hb: &Blake3Hash) -> [u8; 32] {
    derive("cairn.blob.v0", hb)
}

/// Tree address: tree provenance hash, domain-separated.
pub fn tree_key(hp: &Blake3Hash) -> [u8; 32] {
    derive("cairn.tree.v0", hp)
}

/// Patch address: patch provenance hash, domain-separated.
pub fn patch_key(hp: &Blake3Hash) -> [u8; 32] {
    derive("cairn.patch.v0", hp)
}

/// Repository state singleton.
pub fn state_key() -> [u8; 32] {
    derive("cairn.state.v0", b"state")
}

/// Pending pre-build snapshot singleton (replaces the .pending scratch file).
pub fn pending_key() -> [u8; 32] {
    derive("cairn.pending.v0", b"pending")
}

// ============================================================================ open / lock

impl CairnVault {
    /// Open (or create) the vault for the repository at `cairn_dir` (the `.cairn` directory).
    /// Takes the exclusive repository lock, converges the mirror pair if they diverged,
    /// then resumes from the committed head (or geneses a fresh pair).
    pub fn open(cairn_dir: &Path) -> Result<Self> {
        fs::create_dir_all(cairn_dir)
            .with_context(|| format!("Failed to create {:?}", cairn_dir))?;

        let lock = take_lock(&cairn_dir.join("lock"))?;

        let path_a = cairn_dir.join("vault");
        let path_b = cairn_dir.join("vault.shadow");

        // create() adopts an existing file (growing to at least INITIAL_BLOCKS, never truncating).
        let mut a = FileDev::create(&path_a, INITIAL_BLOCKS)
            .map_err(|e| anyhow!("Failed to open vault: {e}"))?;
        let mut b = FileDev::create(&path_b, INITIAL_BLOCKS)
            .map_err(|e| anyhow!("Failed to open vault shadow: {e}"))?;

        // Heal divergence (crash with one mirror behind, or a dropped shadow) before composing.
        verified_replicate(&mut a, &mut b, HOST_RING_LOG2)
            .map_err(|e| anyhow!("Vault mirror replication failed: {e}"))?;

        let vault = Vault::open(Mirror::new(a, b), HOST_RING_LOG2, unix_now())
            .map_err(|e| anyhow!("Failed to open vault: {e}"))?;

        Ok(Self { vault, _lock: lock })
    }

    /// Open the vault for the repository containing `cairn_dir`'s project, erroring (not
    /// creating) when no repository exists.
    pub fn open_existing(cairn_dir: &Path) -> Result<Self> {
        if !cairn_dir.join("vault").exists() {
            anyhow::bail!("Not a cairn repository (no vault in {:?})", cairn_dir);
        }
        Self::open(cairn_dir)
    }

    // ======================================================================== KV API

    pub fn get(&mut self, key: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.vault
            .get(key)
            .map_err(|e| anyhow!("Vault read failed: {e}"))
    }

    pub fn exists(&mut self, key: &[u8; 32]) -> Result<bool> {
        Ok(self.get(key)?.is_some())
    }

    /// Insert/overwrite, durable on return. Values of any size land as one extent in the
    /// engine; a full tract grows automatically — doubling, or more when the value alone
    /// demands it — and retries.
    pub fn put(&mut self, key: &[u8; 32], value: &[u8]) -> Result<()> {
        for _ in 0..8 {
            match self.vault.put(key, value, unix_now()) {
                Ok(()) => return Ok(()),
                Err(manifestus::Error::TractFull) | Err(manifestus::Error::Fenced(_)) => {
                    let len = self.vault.tract_blocks();
                    // Enough for a doubling, or for this value with a lap of slack.
                    let value_blocks = (value.len() as u64 / 4096 + 2) * 2;
                    let target = (len * 2).max(len + value_blocks + 64);
                    self.vault
                        .grow(target, unix_now())
                        .map_err(|e| anyhow!("Vault grow to {target} blocks failed: {e}"))?;
                }
                Err(e) => return Err(anyhow!("Vault write failed: {e}")),
            }
        }
        Err(anyhow!("Vault write failed: tract exhausted after repeated grows"))
    }

    /// Store only when absent (content-addressed dedup). Returns true when a write happened.
    /// Race-free: the repository lock makes this handle the only writer.
    pub fn put_if_absent(&mut self, key: &[u8; 32], value: &[u8]) -> Result<bool> {
        if self.exists(key)? {
            return Ok(false);
        }
        self.put(key, value)?;
        Ok(true)
    }

    /// Delete, durable on return. Absent keys are a no-op returning false.
    pub fn delete(&mut self, key: &[u8; 32]) -> Result<bool> {
        self.vault
            .delete(key, unix_now())
            .map_err(|e| anyhow!("Vault delete failed: {e}"))
    }

    /// True when the shadow mirror was lost or healed this session — data is safe on the
    /// primary, but redundancy is reduced until the next clean open.
    pub fn degraded(&mut self) -> bool {
        self.vault.degraded()
    }
}

/// Take the exclusive repository lock, waiting out concurrent cairn invocations.
fn take_lock(path: &PathBuf) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("Failed to open lock file {:?}", path))?;

    let deadline = std::time::Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "Another cairn process has held the repository lock for over {}s",
                        LOCK_TIMEOUT.as_secs()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(anyhow!("Failed to lock repository: {e}"));
            }
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_put_get_roundtrip_and_resume() -> Result<()> {
        let dir = TempDir::new()?;
        let cairn_dir = dir.path().join(".cairn");
        let key = blob_key(blake3::hash(b"hello").as_bytes());
        {
            let mut v = CairnVault::open(&cairn_dir)?;
            assert!(v.get(&key)?.is_none());
            assert!(v.put_if_absent(&key, b"hello")?);
            assert!(!v.put_if_absent(&key, b"hello")?, "second store is a dedup no-op");
            assert_eq!(v.get(&key)?.as_deref(), Some(&b"hello"[..]));
        }
        // Reopen: committed state survives the handle.
        let mut v = CairnVault::open(&cairn_dir)?;
        assert_eq!(v.get(&key)?.as_deref(), Some(&b"hello"[..]));
        Ok(())
    }

    #[test]
    fn large_values_roundtrip() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = CairnVault::open(&dir.path().join(".cairn"))?;
        let big = vec![0xA7u8; 300_000]; // furrow territory, several dozen blocks
        let key = blob_key(blake3::hash(&big).as_bytes());
        v.put(&key, &big)?;
        assert_eq!(v.get(&key)?, Some(big));
        Ok(())
    }

    #[test]
    fn multi_megabyte_values_roundtrip() -> Result<()> {
        let dir = TempDir::new()?;
        let mut v = CairnVault::open(&dir.path().join(".cairn"))?;
        // Far past the old per-value cap AND the initial 4MB tract (grow fires) — the
        // engine's extent leaves carry any size natively now.
        let big: Vec<u8> = (0..6_000_000u32).map(|i| i.wrapping_mul(2654435761) as u8).collect();
        let key = blob_key(blake3::hash(&big).as_bytes());
        v.put(&key, &big)?;
        assert_eq!(v.get(&key)?.as_deref(), Some(&big[..]));

        // Overwrite with a shorter value, then delete.
        let smaller = vec![0x5Au8; 2_500_000];
        v.put(&key, &smaller)?;
        assert_eq!(v.get(&key)?.as_deref(), Some(&smaller[..]));
        assert!(v.delete(&key)?);
        assert!(v.get(&key)?.is_none());
        assert!(!v.delete(&key)?);
        Ok(())
    }

    #[test]
    fn domains_do_not_collide() {
        let h = *blake3::hash(b"same input").as_bytes();
        let keys = [blob_key(&h), tree_key(&h), patch_key(&h)];
        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[1], keys[2]);
        assert_ne!(keys[0], keys[2]);
    }

    #[test]
    fn state_and_pending_are_distinct_singletons() {
        assert_ne!(state_key(), pending_key());
    }
}
