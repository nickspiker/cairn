# Cairn

**Build-gated version control for Rust projects.**

---

## The Problem

You code for 3 hours. Run `cargo build`. 500 errors. Don't remember what changed.

Git shows 800 lines across 12 files. You spend 2 hours trying to undo changes. Finally `git reset --hard`, losing everything.

**Cairn solves this:** Every successful build auto-captures as a patch. Instant rollback to any working state.

---

## Core Concept: Build-Gated Versioning

**Git doesn't know if your code compiles. Cairn only saves states that build.**

```bash
$ # Make breaking changes
$ cargo build
error[E0425]: cannot find function `parse_token`
  → Nothing happens. No patch created.

$ # Fix the build
$ cargo build
    Finished dev [unoptimized + debuginfo] target(s) in 2.34s
✓ Patch created: maple-crane-forest-pixel-dance

$ cairn list
● maple-crane-forest-pixel-dance [CURRENT]
  "Modified 3 files (+47, -23) in decode.rs, apply.rs"
  2 minutes ago

● cloud-river-stone-bright-moon
  "Refactored error handling in parser.rs"
  1 hour ago
```

**Every patch in history is guaranteed to build.** No broken states, no guessing.

---

## Features

**Automatic patches on successful builds:**
- Build succeeds → patch created automatically
- Build fails → nothing happens
- No manual commands (compiler is the gatekeeper)

**Instant rollback:**
```bash
$ cairn rollback cloud-river-stone-bright-moon
✓ Rolled back 10,000 files (47 changed) in 16ms

$ cargo build
    Finished dev in 0.23s  ← Guaranteed to work
```

**Mnemonic patch IDs and LLM summary:**
- Local LLM summarizes edits into 1 sentence
- Basic stats also show upon hover, patch size, patch file location
- 5-word phrases instead of git hashes
- Easy to remember: "maple-crane-forest-pixel-dance"
- Fuzzy matching: "maple crane" finds the patch

**Slide through time:**
- Click any patch → instant rollback (no confirmation)
- Keep clicking to explore history
- Build with changes → creates new patch, orphans future ones

**VSF-encoded patches:**
- ~420 bytes typical patch (vs 700+ for JSON)
- BLAKE3 cryptographic verification
- Reed-Solomon error correction (2x size, 10000x reliability)

---

## Installation

```bash
cargo install cairn
```

Or from source:
```bash
git clone https://github.com/nickspiker/cairn
cd cairn
cargo build --release
cargo install --path .
```

---

## Usage

**Initialize:**
```bash
$ cd your-rust-project
$ cairn init
Initialized .cairn/
No patches yet - run 'cargo build' to create first
```

**Build (automatic patch):**
```bash
$ cargo build
    Finished dev in 2.34s
✓ Patch created: maple-crane-forest-pixel-dance
  10 files tracked
```

**List history:**
```bash
$ cairn list
● maple-crane-forest [CURRENT] - 2 min ago
  "Modified decode.rs, apply.rs (+47, -23)"
  
● cloud-river-stone - 1 hr ago
  "Refactored error handling"

○ swift-ocean-light - yesterday
  "Initial commit"
```

**Rollback:**
```bash
$ cairn rollback cloud-river
✓ Restored to cloud-river-stone-bright-moon (1 hour ago)
  47 files changed in 23ms

$ cargo build
    Finished dev in 0.31s
```

**Fuzzy matching works:**
```bash
$ cairn rollback maple    # Finds maple-crane-forest-pixel-dance
$ cairn rollback cloud r  # Finds cloud-river-stone-bright-moon
```

---

## VSCode Extension

Install from marketplace: `cairn`

**Features:**
- Auto-patch on successful build
- Visual timeline with mnemonics
- Hover for AI-generated summaries
- Click patch → instant rollback
- Orphaned patches section

```
HISTORY
● maple-crane-forest-pixel-dance [CURRENT]
  ↑ Hover: "Refactored error handling in decode.rs..."
  ↑ Click: instant rollback

● cloud-river-stone-bright-moon
● swift-ocean-light-paper-wind

ORPHANED (parent: cloud-river-stone)
○ broken-attempt-one
○ broken-attempt-two
  (These were orphaned when you rolled back and built)
```

---

## How It Works

**Build-gated capture:**
1. `cargo build` succeeds
2. Check if files changed (prevent duplicate patches)
3. Compute delta from current state
4. Create VSF-encoded patch
5. Store in `.cairn/patches/`

**Race condition solved:** Uses cargo's fingerprint to capture exactly what was compiled, not current disk state.

**Atomic rollback:**
1. Build file tree in temp directory
2. Hardlink unchanged files (zero copy)
3. Atomic directory swap (`rename` is atomic)
4. Update state file (Reed-Solomon protected)

If crash happens: repo stays consistent, recovery on next startup.

**Orphaning:** After rollback + build with changes, future patches move to ORPHANED section. Nothing is lost—you can restore orphaned patches.

---

## Technical Details

**Repository structure:**
```
.cairn/
├── state.vsf              # Current state + history
├── patches/               # VSF-encoded patches (by provenance hash)
│   └── xiN2nO...lKI.vsf
└── objects/               # Content-addressed storage
    └── a3/f8d9e1...
```

**Patch format:**
```rust
struct Patch {
    metadata: {
        author: [u8; 32],       // BLAKE3("Nick Spiker <email>")
        parent: Option<PatchId>,
        timestamp: f64,
        ai_summary: String,     // Auto-generated
    },
    delta: Vec<Change> {
        InsertLine { file, after, content },
        DeleteLine { file, at, old_content },
        ModifyLine { file, at, old, new },
        AddFile { path, content },
        DeleteFile { path, old_content },
        RenameFile { from, to },
    },
    build_hash: [u8; 32],
}
```

**AI summaries:** Generated using embedded `lm.rs` model (~50MB, zero dependencies). Example: "Refactored error handling in decode.rs, added empty section support"

**Terminology:**
- **State**: Repository snapshot (`.cairn/state.vsf`)
- **Patch**: VSF file containing delta + metadata
- **Delta**: Set of changes in a patch
- **Change**: Single operation (InsertLine, AddFile, etc.)
- **History**: Chronological chain of patches
- **Current**: Active patch (working directory state)
- **Orphaned**: Patches cut off after rollback + new build

---

## Works With Git

Cairn complements Git—use both:
- **Git** for commits, branches, collaboration
- **Cairn** for build-gated local snapshots

Cairn doesn't replace Git's collaboration features. It solves a different problem: "give me the last state that compiled."

---

## Prior Art

Built on:
- **Pijul/Darcs** - Patch theory with commutation
- **Git** - Content-addressed storage
- **BTRFS** - Copy-on-write snapshots

Novel contributions:
- Build-gated snapshots (not time/manual)
- Cargo fingerprint for race-free capture
- Mnemonic patch IDs (5-word phrases)
- VSF encoding (efficient, verified)
- Hardlink optimization (instant rollback)
- Embedded AI summaries (zero dependencies)

---

## Current Limitations

**Linear history:**
- No merges yet (coming with patch commutation)
- Multi-dev: rebase required
- Orphaned patches handle divergence

**Rust-only:**
- Assumes `cargo build`
- Multi-language support planned

**Local-only:**
- No remote sync yet
- Copy `.cairn/` to sync manually

---

## Roadmap

**v0.1.0** (Week 2):
- Patch commutation (merge support)
- Multi-developer workflows
- Conflict detection

**v0.2.0** (Week 3):
- Cloud sync (encrypted)
- TOKEN-signed patches
- Multi-language support

---

## License

MIT or Apache-2.0, your choice.

---

## Author

**Nick Spiker**

Building correct tools from first principles:
- [Spirix](https://github.com/nickspiker/spirix) - Two's complement floating point
- [TOKEN](https://github.com/nickspiker/token) - Cryptographic identity
- [VSF](https://github.com/nickspiker/vsf) - Versatile Storage Format
- [ferros](https://ferros.org) - Kill-switch ready OS

---

## Contributing

Pull requests welcome.

Before contributing:
- Read `AGENT.md` for guidelines
- All tests must pass (`cargo test`)
- No bounds checks without proof
- Use VSF high-level APIs only

Questions? Open an issue.

---

**Cairn: Because every successful build deserves to be saved.**