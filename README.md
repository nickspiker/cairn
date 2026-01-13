# Cairn

**Build-gated version control for Rust projects.**

> **Status:** v0.0.1 - Actively tested on Linux. Windows/macOS binaries included but untested. Report issues!

---

## The Problem

You code for 3 hours. Run `cargo build`. 500 errors. Don't remember what changed.

Git shows 800 lines across 12 files. You spend 2 hours trying to undo changes. Finally `git reset --hard`, losing everything.

**Cairn solves this:** Every successful build auto-captures as a patch. Instant rollback to any working state.

---

## Quick Start

### VSCode Extension (Recommended)

1. Install from VSCode Marketplace: search "Cairn"
2. Open a Rust project
3. Click **Build** in the Cairn Build Commands panel
4. Done! Cairn auto-initializes and creates your first patch

### CLI

```bash
# Install cargo-cairn (the build wrapper)
cargo install cairn

# Use it instead of cargo build
cargo cairn build
cargo cairn test
cargo cairn build --release
```

On first run, cairn auto-creates `.cairn/` - no init needed.

---

## How It Works

```bash
$ cargo cairn build
📸 Cairn: Snapshotting current state...
   Compiling myproject v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 2.34s
✓ Cairn: Patch created

$ cairn list
Patches (newest first):

  #2   maple-crane-forest-pixel-dance (CURRENT, NEWEST)
  #1   cloud-river-stone-bright-moon
  #0   swift-ocean-light-paper-wind
```

**Key insight:** `cargo cairn` snapshots your code BEFORE the build starts, then only commits the patch if the build succeeds. This guarantees the patch matches exactly what was compiled.

---

## Features

**Automatic patches on successful builds:**
- Build succeeds → patch created automatically
- Build fails → nothing saved
- No manual commands (compiler is the gatekeeper)

**Instant rollback:**
```bash
$ cairn rollback cloud-river-stone-bright-moon
✓ Rolled back to patch cloud-river-stone-bright-moon
  47 files updated

$ cargo build
    Finished dev in 0.23s  # Guaranteed to work
```

**Mnemonic patch IDs:**
- 5-word phrases instead of git hashes
- Easy to remember: "maple-crane-forest-pixel-dance"
- Partial matching: `cairn rollback maple` works

**VSCode Extension:**
- Visual patch history in Explorer sidebar
- Click-to-rollback (white dot = current, black dot = click to go there)
- Configurable build command buttons
- No terminal spawning - clean notifications

---

## Installation

### From Source (Recommended for now)

```bash
git clone https://github.com/nickspiker/cairn
cd cairn
cargo install --path .
```

This installs both `cairn` (CLI) and `cargo-cairn` (build wrapper).

### VSCode Extension

Install from marketplace: search "Cairn" by nickspiker

Or install manually:
```bash
cd vscode-extension
npm install
npm run compile
vsce package
# Then: Extensions → Install from VSIX
```

---

## CLI Reference

```bash
cairn                    # Show help
cairn init               # Initialize .cairn/ (optional - auto-inits on first build)
cairn list               # List all patches
cairn show <patch-id>    # Show patch details
cairn rollback <patch-id> # Rollback to a patch

cargo cairn build        # Build with auto-snapshot
cargo cairn test         # Test with auto-snapshot
cargo cairn build --release  # Release build with auto-snapshot
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
├── state.vsf           # Current state + patch list
└── patches/            # VSF-encoded patches (content-addressed)
    ├── xiN2nO...lKI
    └── a3f8d9...e1b
```

**Patch format (VSF-encoded):**
- Author ID (BLAKE3 hash of git user.name + email)
- Parent patch pointer (cryptographic chain)
- Timestamp
- Delta operations (InsertLine, DeleteLine, AddFile, etc.)
- Build hash

**Excluded from snapshots:**
- `.cairn/`, `target/`, `.git/`, `node_modules/`, `out/`, `dist/`
- `*.vsix`, `*.wasm`, `Cargo.lock`, `package-lock.json`
- Hidden files (`.env`, etc.)

---

## Works With Git

Cairn complements Git—use both:
- **Git** for commits, branches, collaboration
- **Cairn** for build-gated local snapshots

Cairn doesn't replace Git. It solves a different problem: "give me the last state that compiled."

```bash
# Typical workflow
git checkout -b feature
cargo cairn build    # Snapshot as you develop
cargo cairn build    # Each successful build = new patch
cairn rollback ...   # Oops, go back
cargo cairn build    # Try again
git add -A && git commit  # When ready, commit to git
```

---

## Current Limitations

- **Linear history** - No merges yet
- **Rust-only** - Assumes cargo build (other languages planned)
- **Local-only** - No remote sync yet
- **Linux tested** - Windows/macOS builds included but untested

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

**Nick Spiker** - Building correct tools from first principles.

---

**Cairn: Because every successful build deserves to be saved.**
