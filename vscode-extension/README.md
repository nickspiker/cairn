# Cairn VSCode Extension

Automatic patch creation on successful cargo builds with visual timeline.

## Features

- **Automatic Patches**: Creates a patch after every successful `cargo build`
- **Visual Timeline**: Tree view showing patch history with CURRENT and NEWEST markers
- **Status Bar**: Shows the current patch at a glance
- **One-Click Rollback**: Right-click any patch to restore that state
- **Patch Details**: View detailed information about any patch

## Commands

- `Cairn: Initialize Repository` - Initialize cairn in the current workspace
- `Cairn: List Patches` - Show all patches in the terminal
- `Cairn: Rollback to Patch` - Select and rollback to a previous patch
- `Cairn: Show Patch Details` - View detailed information about a patch
- `Cairn: Refresh History` - Refresh the patch history view

## Requirements

- VSCode 1.85.0 or higher
- Cairn CLI installed (`cargo install cairn`)
- Rust project with `cargo` available

## Installation

1. Build the extension:
   ```bash
   cd vscode-extension
   npm install
   npm run compile
   ```

2. Package the extension:
   ```bash
   npm install -g @vscode/vsce
   vsce package
   ```

3. Install the `.vsix` file in VSCode

## Usage

1. Open a Rust project in VSCode
2. Initialize cairn: `Cairn: Initialize Repository`
3. Build your project: `cargo build`
4. View patch history in the Cairn sidebar
5. Right-click any patch to rollback

## Development

```bash
# Install dependencies
npm install

# Compile TypeScript
npm run compile

# Watch for changes
npm run watch

# Run linter
npm run lint
```

## License

MIT or Apache-2.0
