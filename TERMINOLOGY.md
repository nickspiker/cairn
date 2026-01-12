# Cairn Terminology

Clear, consistent vocabulary for cairn's version control concepts.

## Core Concepts

### State
Repository state (`.cairn/state.vsf`)
- Tracks the history of patches
- Maintains current patch pointer
- Stores file tree hashes

### Patch
VSF file containing delta + metadata (`.cairn/patches/{hash}`)
- Immutable snapshot of changes
- Content-addressed by VSF provenance hash
- Contains author, parent, timestamp, message, and operations

### Delta
Set of changes in a patch
- Collection of operations that transform one state to another
- Applied atomically (all-or-nothing)

### Change
Single operation (InsertLine, AddFile, ModifyLine, etc.)
- Line-based modifications
- File additions, deletions, renames
- Content-verified before application

### History
Chronological chain of patches
- Linear sequence from first to latest
- Each patch (except first) has a parent
- Forms a Merkle chain via parent pointers

### Current
Active patch (what working directory matches)
- Marked with `(CURRENT)` in list output
- Stored in state as `head` field
- Can be moved via rollback
- May differ from NEWEST if you've rolled back

### Newest
Most recent patch chronologically
- Marked with `(NEWEST)` in list output
- Last element in patches array
- May differ from CURRENT if you've rolled back
- When CURRENT == NEWEST, shown as `(CURRENT, NEWEST)`

### Collection
The `.cairn/` directory
- Contains all patches and state
- Structure:
  ```
  .cairn/
  ├── state.vsf       # Repository state
  └── patches/        # Patch files (base64url names)
      ├── vZIKTSs8UwuXOm6x...
      └── 1eV7nXiZCbTHTjy1...
  ```

### Tree
File paths → content hashes map
- Snapshot of all tracked files
- Used for incremental updates
- Excludes `.cairn`, `target`, `.git`, and hidden files

## Patch Identification

### Mnemonic
5-word human-readable identifier
- Example: `courtesy-turbine-vision-dental-kinetic`
- ~58 bits of entropy (288 quadrillion unique values)
- Fuzzy matching for typo correction
- No prefix matching - full 5 words required

### Base64url Hash
43-character provenance hash
- Example: `1eV7nXiZCbTHTjy1qhB4yC_IQvYHzGdRx8KmXCwPQYA`
- 256 bits of cryptographic security
- Content-addressed (hash of VSF encoding)
- URL-safe, no padding

## Operations

### Snapshot
Create a new patch from current working directory
- Scans files, computes delta from current patch
- Generates VSF encoding with provenance hash
- Updates state to point to new patch

### Rollback
Move current to a previous patch
- Replays history from start to target patch
- Overwrites working directory files
- Updates state pointer

### List
Display all patches in chronological order
- Shows mnemonic identifiers
- Marks current patch
- Newest first by default

### Show
Display details of a specific patch
- Author, message, timestamp
- Number of operations
- File change summary (TODO)

## Terminology We Avoid

- ~~HEAD~~ → **CURRENT** (more intuitive)
- ~~commit~~ → **patch** (emphasizes immutability)
- ~~branch~~ → (not yet implemented)
- ~~checkout~~ → **rollback** (clearer action)
- ~~staging~~ → (automatic on build)
- ~~diff~~ → **delta** (when referring to stored changes)
- ~~snapshot~~ → **state**
