# AGENT.md - Cairn Code Generation Rules

## Rule 0: Bounds Checks and Saturating Arithmetic

**IF YOU ADD ANY BOUNDS CHECK OR SATURATING ARITHMETIC, YOU ARE REQUIRED TO:**
0. **STATE WHY** it was added
1. **PROVE** it was necessary
2. **EXPLAIN** what undefined behavior or memory unsafety it prevents

### When Bounds Checks ARE Required:
```rust
// External input from user files
let line_num = extract_usize_from_value(&field.values[1])?;
if line_num < file_lines.len() {
    // Line number from patch might be stale/corrupted
    file_lines[line_num] = new_content;
}
```
**WHY**: Line numbers come from VSF patches on disk. Files may have been edited externally. Check prevents out-of-bounds write.

**PROOF**: Without check, corrupted patch could cause buffer overflow.

### When Bounds Checks Are FORBIDDEN:
```rust
// Internal patch generation - line numbers from our own diff algorithm
for (idx, line) in old_lines.iter().enumerate() {
    operations.push(LineOp::DeleteLine {
        file: path.clone(),
        at: idx,  // idx is MATHEMATICALLY proven in-bounds by enumerate()
        old_content: line.clone(),
    });
}
```

### When to Panic Instead:
```rust
// Repository invariant violation - FAIL LOUD
let patch_bytes = fs::read(patch_path)?;
let patch = Patch::decode_vsf(&patch_bytes)
    .expect("Corrupted patch file in .cairn/ - repository is broken");
```

**If .cairn/ contains invalid VSF, that's a BUG in our encoding, not user error.**

## VSF Serialization: Use High-Level APIs

**ALWAYS use VSF's schema-validated builders. NEVER manually construct bytes.**

### The Right Way: VsfSection + VsfBuilder

```rust
// Build patch with validation
let mut metadata_section = VsfSection::new("metadata");
metadata_section.add_field("author", VsfType::h(31, author.to_vec()));
metadata_section.add_field("message", VsfType::x(message.clone()));

let bytes = VsfBuilder::new()
    .add_section_obj(metadata_section)
    .build()?;
```

### FORBIDDEN: Manual Byte Manipulation

```rust
// NO - manual serialization, error-prone, no validation
let mut bytes = Vec::new();
bytes.extend(b"[d{metadata}");
bytes.extend(author.as_bytes());
// This is wrong and fragile
```

### VSF Type Markers Are Self-Describing

**NEVER rely on position to determine what a value is. The type marker tells you.**

```rust
// WRONG - positional parsing breaks if fields reorder
let author = extract_hash(&values[0])?;
let parent = extract_hash(&values[1])?;

// CORRECT - match on field name
match field.name.as_str() {
    "author" => extract_hash(&field.values[0])?,
    "parent" => extract_hash_opt(&field.values[0]),
    _ => continue,
}
```

## Patch Theory Correctness

### Linear History (Week 1)

```rust
// Parent is OPTIONAL (None for initial patch)
parent: Option<PatchId>

// NOT Vec<PatchId> - we're not doing merges yet
```

### File Operations Must Be Atomic

```rust
// CORRECT - entire patch applies or fails
fn apply_patch(patch: &Patch, repo: &mut RepoState) -> Result<()> {
    // Validate ALL operations first
    for op in &patch.operations {
        validate_operation(op, repo)?;
    }
    
    // Then apply ALL operations
    for op in &patch.operations {
        apply_operation(op, repo)?;
    }
    Ok(())
}

// WRONG - partial application leaves inconsistent state
fn apply_patch(patch: &Patch, repo: &mut RepoState) -> Result<()> {
    for op in &patch.operations {
        apply_operation(op, repo)?;  // If this fails halfway, repo is broken
    }
    Ok(())
}
```

### Patch IDs Must Be Content-Addressed

```rust
// CORRECT - patch ID is deterministic hash of contents
pub fn id(&self) -> PatchId {
    let encoded = self.encode_vsf().expect("Encoding failed");
    *blake3::hash(&encoded).as_bytes()
}

// WRONG - random or sequential IDs break content addressing
pub fn id(&self) -> PatchId {
    rand::random()  // NO
}
```

## Multi-File Tree Structure

### Path Handling

```rust
use std::path::PathBuf;

// File paths in patches are relative to repository root
LineOp::InsertLine {
    file: PathBuf::from("src/main.rs"),  // NOT absolute paths
    // ...
}

// Store in VSF with 'l' (literal string) type
VsfType::l(path.to_string_lossy().to_string())
```

### Repository State

```rust
// State tracks entire tree, not single file
struct RepoState {
    // All files in the working directory
    files: HashMap<PathBuf, Vec<u8>>,
    
    // Last applied patch
    head: Option<PatchId>,
}
```

## Zero-Indexing

**Start counting at 0, not 1. This is not negotiable.**

```rust
// Line operations use 0-based indexing
LineOp::InsertLine {
    after: 0,  // Insert before first line (0 = before line 0)
    // ...
}

// Loop indices start at 0
for idx in 0..file_lines.len() {
    // idx: 0, 1, 2, ...
}
```

## Author Identity

**Nick Spiker's author ID is deterministic and constant:**

```rust
pub fn nick_spiker_author_id() -> AuthorId {
    *blake3::hash(b"Nick Spiker <nick@spiker.dev>").as_bytes()
}

// NOT username@hostname (varies by machine)
// NOT random (non-deterministic)
```

## Error Handling Philosophy

0. **Encoding/decoding bugs should panic** - if our VSF is invalid, WE broke it
1. **User file corruption should error** - return `Result` so user can fix/rollback
2. **Repository invariants should be asserts** - if .cairn/ structure is wrong, fail loud

```rust
// Corrupted user files - return error
pub fn decode_vsf(bytes: &[u8]) -> Result<Self> {
    let header = VsfHeader::decode(bytes)
        .context("Invalid VSF header - file may be corrupted")?;
    // ...
}

// Our encoding broken - panic
pub fn encode_vsf(&self) -> Vec<u8> {
    VsfBuilder::new()
        .add_section_obj(metadata)
        .build()
        .expect("BUG: Failed to encode patch we just created")
}

// Repository invariant - assert in debug
fn get_patch(&self, id: PatchId) -> &Patch {
    debug_assert!(self.patches.contains_key(&id), "Patch missing from index");
    &self.patches[&id]
}
```

## Code Style

### Prefer Explicit Over "Safe"

```rust
// YES - direct access, clear intent
file_lines[idx] = new_content;

// NO - hiding potential bugs
if let Some(line) = file_lines.get_mut(idx) {
    *line = new_content;
}
```

### Terminal Commands

```bash
# DON'T add comments in commands
./build-development.sh

# DO separate commands that should be run individually
cargo clean
./build-development.sh
```

## Testing Requirements

Every module MUST have `#[cfg(test)]` section with:
- Roundtrip tests (encode → decode → verify)
- Edge cases (empty files, large files, Unicode)
- Failure cases (corrupted VSF, invalid operations)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_patch() {
        let original = Patch::new(...);
        let encoded = original.encode_vsf();
        let decoded = Patch::decode_vsf(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_corrupted_vsf_fails() {
        let bad_bytes = vec![0xFF; 100];
        assert!(Patch::decode_vsf(&bad_bytes).is_err());
    }
}
```

## Build Commands

**Development builds ONLY unless explicitly requested:**

```bash
# CORRECT - development build
./build-development.sh

# FORBIDDEN - release builds without permission
cargo build --release  # NO
```

## When Unsure

**ASK.** Don't add "defensive" checks. If you're not sure:
0. State what you're unsure about
1. Show the code without the check
2. Explain what would happen if the invariant is violated
3. Let the human decide

---

*Cairn is building patch-based version control from first principles. The math of patch commutation is correct. Trust it. Fail loud when invariants break. Use VSF's type system - it exists for a reason.*
