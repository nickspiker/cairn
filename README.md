# Cairn

**Build-gated version control for Rust projects.**

> **⚠️ EXPERIMENTAL - v0.0.1 - NOT PRODUCTION READY**
>
> **Do not use on critical projects.** While all data is hash-verified and should not corrupt, this extension is new and not extensively tested yet:
>
> **Use at your own risk. Always have backups.**

---

## The Problem

You code for 3 hours. Run `cargo build`. 500 errors. Don't remember what changed.

Git shows 800 lines across 12 files. You spend 2 hours trying to undo changes. Finally `git reset --hard`, losing everything.

**Cairn solves this:** Every successful build auto-captures as a patch. Jump to any working state instantly.

---

## Quick Start

### VSCode Extension (Recommended)

1. Install from VSCode Marketplace: search "Cairn"
2. Open a Rust project
3. Click **Build** in the Cairn Build Commands panel or by using the terminal extension with `cargo cairn build`
4. Cairn auto-initializes and creates your first patch if the build is successful

### CLI for agent use and those who prefer terminal commands

```bash
# Install cargo-cairn (the build wrapper)
cargo install cairn

# Use it instead of cargo build
cairn build
cairn test
cairn build --release
```

On first run, cairn auto-creates `.cairn/` - no init needed.

---

## How It Works

```bash
$ cairn build
📸 Cairn: Snapshotting current state...
   Compiling myproject v0.0.0
    Finished dev [unoptimized + debuginfo] target(s) in 2.34s
✓ Cairn: Patch created

$ cairn list
Patches (newest first):

  #2   maple-crane-forest-pixel-dance (CURRENT, NEWEST)
  #1   cloud-river-stone-bright-moon
  #0   swift-ocean-light-paper-wind
```

**Key insight:** `cargo cairn` snapshots your code BEFORE the build starts, then only saves the patch if the build succeeds. This guarantees the patch matches exactly what was compiled even if edits are done during compilation.

---

## Features

**Automatic patches on successful builds:**
- Build succeeds → patch created automatically
- Build fails → patch created and discarded, nothing saved

**Switch between patches:**
```bash
$ cairn jump cloud-river-stone-bright-moon
✓ Switched to patch cloud-river-stone-bright-moon
  47 files restored

$ cargo build
    Finished dev in 0.23s
```

**Mnemonic encoding:**
- Uses VSF's 3177-word list (11.63 ish bits per word)
- 5 words = 58 bits ≈ 288 quadrillion combinations
- Birthday paradox: 50% collision at ~537 million patches
- 288 quadrillion combinations
- Birthday bound: 537M patches (1000 patches/day for 1000 years won't reach it)
- Easy to remember: "maple-crane-forest-pixel-dance"

**VSCode Extension:**
- Visual patch history in Explorer sidebar
- Click to switch patches (white dot = current, black dot = click to jump there)
- Configurable build command buttons
- Daemon-based communication with clean notifications

---

## Installation

### VSCode Extension (Recommended)

Install from VSCode Marketplace: search "Cairn" by nickspiker

The extension automatically handles the CLI installation.

### CLI Only (Optional)

If you prefer using the CLI without the VSCode extension:

```bash
cargo install cairn
```

This installs both `cairn` (CLI) and `cargo-cairn` (build wrapper).

---

## CLI Reference

```bash
cairn list               # List all patches
cairn show <patch-id>    # Show patch details
cairn jump <patch-id>    # Switch to a different patch
cairn clear              # Delete all patch history

cairn build              # Build with auto-snapshot
cairn test               # Test with auto-snapshot
cairn run                # Run with auto-snapshot
cairn build --release    # Any cargo args work
```

---

## VSCode Extension Settings

Configure build commands in settings.json:

```json
{
  "cairn.buildCommands": [
    {"label": "Build", "command": "build", "icon": "tools"},
    {"label": "Release", "command": "build --release", "icon": "rocket"},
    {"label": "Test", "command": "test", "icon": "beaker"},
    {"label": "Check", "command": "check", "icon": "check"}
  ]
}
```

---

## Technical Details

**Repository structure:**
```
.cairn/
├── vault           # All repository objects in one mirrored store (manifestus engine)
├── vault.shadow    # Second mirror ring — every block write-verified on both
└── lock            # Exclusive repository lock (concurrent cairn invocations queue)
```

**Storage architecture (vault-based, the same engine as Photon's storage stack):**

All objects live in a single crash-proof key-value vault (manifestus): every 4KB block
is BLAKE3-sealed, every write is verified by read-back and mirrored, and every
operation commits a generation — power loss at any byte boundary leaves the previous
committed state, never a torn file. The vault starts at 5MB per mirror and grows
automatically. No hash-named loose files means no case-sensitivity or partial-write
hazards on any filesystem.

Object model (inspired by Git), addressed by BLAKE3-derived 32-byte keys:

1. **Blobs** - Raw file content, stored once, referenced by BLAKE3 hash
   - Deduplicated: identical files share the same blob
   - Content-addressed: same content = same hash = stored once
   - Any size: multi-gigabyte files are single extent-addressed objects in the vault

2. **Trees** - Directory snapshots mapping file paths to blob hashes
   - Paths normalized (project-relative, forward-slash) so trees round-trip across OSes
   - Enables fast diff comparison between patches

3. **Patches** - Contain:
   - Tree reference (directory snapshot)
   - Parent patch pointer (cryptographic chain)
   - Per-file diffs (binary operations)
   - Message ("Successful build")

**Diff-only storage:**
- **First file version**: Full blob stored
- **Modified files**: Only diff operations stored (no redundant blob)
- **Unchanged files**: Tree points to existing blob (no diff, no new blob)
- **Chain resets**: When diff chain grows larger than file, store new full blob

**Binary diff format:**
- `Copy {start, len}` - Copy bytes from old version
- `Insert {content}` - Insert new bytes
- Files reconstructed by: load base blob → apply diff chain forward

**Excluded from snapshots:**
- `.cairn/`, `target/`, `.git/`, `node_modules/`, `out/`, `dist/`
- `*.vsix`, `*.wasm`, `Cargo.lock`, `package-lock.json`
- Hidden files (`.env`, etc.)

---

## Works With Git

Cairn complements Git—use both:
- **Git** for commits, branches, collaboration
- **Cairn** for build-gated local snapshots

Cairn never touches `.git/`, and jump can only ever write inside the tracked
paths — so `git restore .` is always a complete undo of anything cairn does
to the working tree. On first init inside a git repo, cairn adds `.cairn/`
to your `.gitignore` (announced, idempotent, skipped if already covered) so
the vault never ends up in a commit.

Cairn doesn't replace Git. It solves a different problem: "give me the last state that compiled."

```bash
# Typical workflow
git checkout -b feature
cargo cairn build    # Snapshot as you develop
cargo cairn build    # Each successful build = new patch
cairn jump ...       # Oops, jump to previous working patch
cargo cairn build    # Try again
git add -A && git commit  # When ready, commit to git
```

---

## Known Issues & Limitations

**Current stability issues:**
- Random build hangs (investigating)

**Open items (not pressing):**
- `cairn rollback` is disabled pending redesign; use `cairn jump`
- Pre-vault `.cairn/` layouts (loose `blobs/`, `trees/`, `patches/`) are not
  migrated — the vault starts fresh alongside them; `cairn clear` removes both
- The vault is opened per operation (lock + index walk); if that ever gets slow
  on huge histories, the daemon should own a persistent handle and the CLI
  should talk to it

**Design limitations:**
- **Linear history** - No merges yet
- **Rust-only** - Assumes cargo build (other languages planned)
- **Local-only** - No remote sync yet
- **Linux tested** - Windows/macOS builds included but untested

**Recommendation:** Use alongside Git. Cairn should complement, not replace, your version control.

---

## Roadmap

- [ ] LLM-generated patch summaries on hover
- [ ] Orphaned patches section (diverged history)
- [ ] Reed-Solomon error correction
- [ ] Multi-language support
- [ ] Cloud sync

---

## License

MIT or Apache-2.0, your choice.

---

## Author

**Nick Spiker** - Distilling tools down to the correct thing that was always underneath.

---

**Cairn: Because every successful build deserves to be saved.**