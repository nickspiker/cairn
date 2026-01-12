#!/bin/bash
set -e

VERSION="v0.0.0"

echo "Deploying cairn $VERSION to GitHub"
echo ""

# Create release directory
RELEASE_DIR="release-$VERSION"
mkdir -p "$RELEASE_DIR"

# Copy and rename binaries for GitHub release
echo "Preparing release artifacts..."

cp target/x86_64-unknown-linux-gnu/release/cairn "$RELEASE_DIR/cairn-linux-x64"
cp target/x86_64-unknown-linux-gnu/release/cairn.b3 "$RELEASE_DIR/cairn-linux-x64.b3"

cp target/x86_64-pc-windows-gnu/release/cairn.exe "$RELEASE_DIR/cairn-windows-x64.exe"
cp target/x86_64-pc-windows-gnu/release/cairn.exe.b3 "$RELEASE_DIR/cairn-windows-x64.exe.b3"

cp target/x86_64-apple-darwin/release/cairn "$RELEASE_DIR/cairn-darwin-x64"
cp target/x86_64-apple-darwin/release/cairn.b3 "$RELEASE_DIR/cairn-darwin-x64.b3"

cp target/aarch64-apple-darwin/release/cairn "$RELEASE_DIR/cairn-darwin-arm64"
cp target/aarch64-apple-darwin/release/cairn.b3 "$RELEASE_DIR/cairn-darwin-arm64.b3"

echo "✓ Release artifacts prepared in $RELEASE_DIR/"
echo ""

# Git operations
echo "Git operations..."

# Check if we're in a git repo
if [ ! -d .git ]; then
    echo "Initializing git repository..."
    git init
    git branch -M trunk
fi

# Add all files
git add .

# Check if there are changes to commit
if git diff --staged --quiet; then
    echo "No changes to commit"
else
    echo "Creating commit..."
    git commit -m "Release $VERSION

- Multi-platform cairn binaries (Linux, Windows, macOS Intel/ARM64)
- VSCode extension with auto-download
- BLAKE3 hash verification
- Initial v0.0.0 release"
fi

# Check if remote exists
if ! git remote get-url origin &>/dev/null; then
    echo "Adding GitHub remote..."
    git remote add origin https://github.com/nickspiker/cairn.git
fi

# Push to trunk
echo "Pushing to trunk..."
git push -u origin trunk

# Create and push tag
echo "Creating tag $VERSION..."
git tag -a "$VERSION" -m "Release $VERSION" || echo "Tag already exists locally"
git push origin "$VERSION" || echo "Tag already exists on remote"

echo ""
echo "Creating GitHub release..."

# Create GitHub release with binaries
gh release create "$VERSION" \
    --title "$VERSION - Initial Release" \
    --notes "# Cairn v0.0.0

**Patch-based version control for successful Rust builds**

## Features
- Auto-creates patches after successful \`cargo build\`
- VSCode extension with automatic binary download
- Multi-platform support (Linux, Windows, macOS Intel/ARM64)
- BLAKE3 hash verification
- Mnemonic patch IDs using VSF word list

## Installation

### VSCode Extension
\`\`\`bash
# Install from marketplace (coming soon)
code --install-extension nickspiker.cairn
\`\`\`

### Manual Installation
Download the appropriate binary for your platform and place it in your PATH.

## Platform Support
- Linux x64
- Windows x64
- macOS Intel (x64)
- macOS Apple Silicon (ARM64)

All binaries include BLAKE3 hash files for verification.

🦀 Generated with Claude Code" \
    "$RELEASE_DIR/cairn-linux-x64" \
    "$RELEASE_DIR/cairn-linux-x64.b3" \
    "$RELEASE_DIR/cairn-windows-x64.exe" \
    "$RELEASE_DIR/cairn-windows-x64.exe.b3" \
    "$RELEASE_DIR/cairn-darwin-x64" \
    "$RELEASE_DIR/cairn-darwin-x64.b3" \
    "$RELEASE_DIR/cairn-darwin-arm64" \
    "$RELEASE_DIR/cairn-darwin-arm64.b3"

echo ""
echo "✓ Release $VERSION deployed successfully!"
echo ""
echo "Release URL: https://github.com/nickspiker/cairn/releases/tag/$VERSION"
echo ""
echo "Next steps:"
echo "  1. Test extension download: code --install-extension vscode-extension/cairn-0.0.0.vsix --force"
echo "  2. Reload VSCode and test auto-download"
echo "  3. If all works, publish to marketplace: cd vscode-extension && vsce publish"
