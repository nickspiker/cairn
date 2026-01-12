
# Cairn

**Patch-based version control for successful cargo builds.**

## The Problem

You spend 3 hours coding. You run `cargo build`. 500 errors. You don't remember what you changed.

Git shows you a diff with 800 lines across 12 files. You spend 2 hours trying to undo your changes. You give up and `git reset --hard` to your last commit from yesterday, losing everything.

**Cairn solves this:** Every successful build is automatically captured as a patch. One command rolls back to any previous working state.

---

### Different Approach: Build-Gated Versioning

Git is a general-purpose version control system that works for any file type. This flexibility means it doesn't know whether your code compiles.

Cairn takes a different approach for Rust projects: it only saves states that successfully build.

**With manual versioning:**
```bash
$ git log --oneline
a3f8d9e (HEAD) WIP: refactoring parser
d7e2c1f Fix tests
c4b9a2e Update dependencies
9f1d3e7 Initial commit
```

When you roll back, you don't know which states actually compiled.

**With build-gated versioning:**
```bash
$ cairn list
● a3f8d9e1 - just now - "Successful build"
● d7e2c1f - 1 hr ago - "Successful build"
● 9f1d3e7 - yesterday - "Successful build"
```

Every state in history is guaranteed to build. No guessing.

### Cairn's Solution: Build-Gated Patches

**Every patch in cairn history is guaranteed to build successfully.**

```bash
$ # Make changes that break the build
$ cargo build
error[E0425]: cannot find function `parse_token` in this scope
  → Nothing happens. No patch created.

$ # Fix the build
$ cargo build
    Finished dev [unoptimized + debuginfo] target(s) in 2.34s
✓ Cairn: Patch created (a3f8d9e1)

$ cairn list
● a3f8d9e1 - just now - "Successful build"  ← GUARANTEED TO WORK
● d7e2c1f - 1 hr ago - "Successful build"   ← GUARANTEED TO WORK
● 9f1d3e7 - yesterday - "Successful build"  ← GUARANTEED TO WORK
```

**When you rollback, you KNOW it will build:**

```bash
$ cairn rollback d7e2c1f
✓ Restored to patch d7e2c1f

$ cargo build
    Finished dev [unoptimized + debuginfo] target(s) in 0.23s  ← Always works
```

**No manual intervention required:**
- Build succeeds → Cairn saves it automatically
- Build fails → Nothing happens
- **You cannot pollute history with broken code**

This is why Cairn has **no manual patch command** - the compiler is the gatekeeper.

---

## What Cairn Does

**Automatic patches:**
- `cargo build` succeeds → Cairn creates a patch automatically
- `cargo build` fails → Nothing happens
- **Every patch in history is a working build**

**Instant rollback:**
```bash
$ cairn rollback a3f8d9e1
Restored to patch a3f8d9e1 (37 minutes ago)
✓ Rolled back 10,000 files (1 changed) in 16ms

$ cargo build
    Finished dev [unoptimized + debuginfo] target(s) in 0.23s
```

**Patch-based storage:**
- Stores *what changed*, not entire file copies
- 420 bytes for a typical patch vs 700+ bytes for JSON
- Cryptographically verified (BLAKE3)
- Stored in VSF format (efficient, self-describing)

---

## How It Works

### Build-Gated Patches

Cairn watches for successful `cargo build` and captures:
1. What cargo actually compiled (from fingerprint, not current disk state)
2. Computes delta from last successful build
3. Creates VSF-encoded patch with Reed-Solomon error correction
4. Stores patch in `.cairn/patches/`

**Race condition solved:** If you edit files while build is running, Cairn captures what cargo *actually compiled*, not what's on disk when the build finishes.

### Patch Theory

Based on the mathematical theory of patches from Pijul/Darcs:

- **Patches are content-addressed** (BLAKE3 hash of operations)
- **Patches are composable** (can be applied in sequence)
- **Patches have inverses** (rollback = apply inverse patch)
- **Linear history** (Week 1: no merges, that comes later with commutation)

### Atomic Operations

Every state change is atomic:

1. Build complete patch in temp directory
2. Use hardlinks for unchanged files (zero copy)
3. Atomic directory swap (`rename` is atomic on all filesystems)
4. Update state file (single atomic write with Reed-Solomon)

**If crash happens:** Repository stays consistent. Recovery on next startup.

---

## Installation

```bash
cargo install cairn
```

Or build from source:

```bash
git clone https://github.com/nickspiker/cairn
cd cairn
cargo build --release
cargo install --path .
```

---

## Usage

### Initialize repository

```bash
$ cd your-rust-project
$ cairn init
Initialized .cairn/ directory
No patches yet - run 'cargo build' to create first patch
```

### Build your project (automatic patch creation)

```bash
$ cargo build
   Compiling myproject v0.0.0
    Finished dev [unoptimized + debuginfo] target(s) in 2.34s
✓ Cairn: Patch created (a3f8d9e1)
  10 files tracked
```

Cairn automatically creates a patch after **every successful build**. No manual command needed.

### List patch history

```bash
$ cairn list
● d8e9f1a2 - 2 min ago - "Fixed parser edge case"
  src/parser.rs (+12, -3)

● b7c2a4f3 - 1 hr ago - "Refactored error handling"
  src/error.rs (+45, -12)
  src/lib.rs (+8, -2)

● a3f8d9e1 - yesterday - "Initial patch"
  src/main.rs, src/lib.rs, Cargo.toml
```

### Rollback to previous build

```bash
$ cairn rollback b7c2a4f3
Restoring to patch b7c2a4f3 (1 hour ago)...
✓ Restored 10,000 files (47 changed)
Rollback complete in 23ms

$ cargo build
    Finished dev [unoptimized + debuginfo] target(s) in 0.31s
```

### Show patch details

```bash
$ cairn show d8e9f1a2
Patch: d8e9f1a2
Author: Nick Spiker <nick@spiker.dev>
Date: 2026-01-11 14:30:22
Message: Fixed parser edge case

Changes:
  src/parser.rs
    + Line 42: if token.is_empty() { return Err(...); }
    - Line 38: let result = parse_token(token);
```

---

## VSCode Extension

Automatic patch creation on successful build with visual timeline:

```
CAIRN HISTORY
├─ ● d8e9f1a2 (2 min ago)
│    "Fixed parser edge case"
│    [View Delta] [Rollback]
│
├─ ● b7c2a4f3 (1 hr ago)
│    "Refactored error handling"
│    [View Delta] [Rollback]
```

Status bar shows last successful build. One-click rollback.

---

## Technical Details

### VSF Encoding

Patches stored in [Versatile Storage Format](https://github.com/nickspiker/vsf):

```
420 Bytes total
├─ metadata: 105 bytes (author, parent, message)
├─ operations: 63 bytes (line-based deltas with file paths)
└─ build_output: 69 bytes (cargo hash for verification)

✓ BLAKE3 provenance hash (integrity)
✓ Reed-Solomon error correction (2x bloat, 10000x reliability)
✓ Huffman compression on strings (36% smaller)
```

### Repository Structure

```
.cairn/
├── state.vsf              # Current repository state (with Reed-Solomon)
├── patches/               # VSF-encoded patches
│   ├── a3f8d9e1...vsf
│   ├── b7c2a4f3...vsf
│   └── d8e9f1a2...vsf
└── objects/               # Content-addressed file storage
    ├── a3/f8d9e1...       # File content (by BLAKE3 hash)
    └── b7/c2a4f3...
```

### Patch Format

```rust
struct Patch {
    metadata: PatchMetadata {
        author: [u8; 32],           // BLAKE3 of "Nick Spiker <email>"
        parent: Option<[u8; 32]>,   // Linear history (no merges yet)
        timestamp: f64,              // Eagle Time (not Unix time)
        message: String,
    },
    operations: Vec<LineOp> {
        InsertLine { file: PathBuf, after: usize, content: Vec<u8> },
        DeleteLine { file: PathBuf, at: usize, old_content: Vec<u8> },
        ModifyLine { file: PathBuf, at: usize, old: Vec<u8>, new: Vec<u8> },
        AddFile { path: PathBuf, content: Vec<u8> },
        DeleteFile { path: PathBuf, old_content: Vec<u8> },
        RenameFile { from: PathBuf, to: PathBuf },
    },
    build_hash: [u8; 32],           // BLAKE3 of cargo output
}
```

---

## Prior Art

Cairn builds on:

- **Pijul** - Patch theory with commutation properties
- **Darcs** - Original theory of patches (2002)
- **Git** - Content-addressed storage and widespread adoption
- **BTRFS** - Copy-on-write snapshots

**What's novel:**
- Build-gated patches (not time-based or manual)
- Cargo fingerprint as source-of-truth (solves race conditions)
- VSF encoding (smaller, faster, cryptographically verified)
- Hardlink optimization (instant rollback for large projects)

---

## Limitations

**Linear history only:**
- No merges yet (coming with commutation)
- Multi-developer: last push wins, rebase required
- Use `cairn pull` before `cairn push`

**Rust-specific:**
- Assumes `cargo build` as build command
- Can be adapted for other languages later

**Local only:**
- No cloud sync yet
- Copy `.cairn/` directory to sync across machines

---

## License

MIT or Apache-2.0, your choice.

---

## Author

**Nick Spiker**

Building tools from first principles:
- [Spirix](https://github.com/nickspiker/spirix) - Two's complement floating point
- [Photon](https://github.com/nickspiker/photon) - Unfuckwithable comms
- [VSF](https://github.com/nickspiker/vsf) - Versatile Storage Format
- [ferros](https://ferros.org) - Kill-switch ready OS

---

## Contributing

Cairn is in active development. Pull requests welcome.

**Before contributing:**
- Read `AGENT.md` for coding guidelines
- Run `cargo test` (all tests must pass)
- No bounds checks without proof of necessity
- Use VSF's high-level APIs (never manual byte manipulation)

**Questions?** Open an issue.

---

**Cairn: Because every successful build deserves to be saved.**