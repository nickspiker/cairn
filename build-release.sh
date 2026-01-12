#!/bin/bash
set -e

echo "Building cairn release binaries for all platforms..."
echo ""

# Build Linux x64
echo "Building Linux x64..."
cargo build --release --target x86_64-unknown-linux-gnu
echo "✓ Linux x64 built"
echo ""

# Build Windows x64
echo "Building Windows x64..."
cargo build --release --target x86_64-pc-windows-gnu
echo "✓ Windows x64 built"
echo ""

# Build macOS Intel
echo "Building macOS Intel..."
export CC_x86_64_apple_darwin=/mnt/Octopus/Code/osxcross/target/bin/x86_64-apple-darwin-clang-wrapper
export CXX_x86_64_apple_darwin=/mnt/Octopus/Code/osxcross/target/bin/x86_64-apple-darwin-clang-wrapper
export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER=$CC_x86_64_apple_darwin
cargo build --release --target x86_64-apple-darwin
echo "✓ macOS Intel built"
echo ""

# Build macOS Apple Silicon
echo "Building macOS ARM64..."
export CC_aarch64_apple_darwin=/mnt/Octopus/Code/osxcross/target/bin/aarch64-apple-darwin-clang-wrapper
export CXX_aarch64_apple_darwin=/mnt/Octopus/Code/osxcross/target/bin/aarch64-apple-darwin-clang-wrapper
export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=$CC_aarch64_apple_darwin
cargo build --release --target aarch64-apple-darwin
echo "✓ macOS ARM64 built"
echo ""

# Generate BLAKE3 hashes
echo "Generating BLAKE3 hashes..."

hash_file() {
    local file=$1
    local hash=$(b3sum "$file" | cut -d' ' -f1)
    echo "$hash" > "$file.b3"
    echo "  $(basename $file): $hash"
}

hash_file target/x86_64-unknown-linux-gnu/release/cairn
hash_file target/x86_64-pc-windows-gnu/release/cairn.exe
hash_file target/x86_64-apple-darwin/release/cairn
hash_file target/aarch64-apple-darwin/release/cairn

echo ""
echo "All binaries built successfully!"
echo ""
echo "Release artifacts:"
echo "  Linux:        target/x86_64-unknown-linux-gnu/release/cairn"
echo "  Windows:      target/x86_64-pc-windows-gnu/release/cairn.exe"
echo "  macOS Intel:  target/x86_64-apple-darwin/release/cairn"
echo "  macOS ARM64:  target/aarch64-apple-darwin/release/cairn"
