use cairn::snapshot_vsf::create_snapshot;
use std::collections::HashMap;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    // Create test files with nested directories
    let mut files = HashMap::new();

    files.insert(
        PathBuf::from("src/main.rs"),
        b"fn main() {\n    println!(\"Hello, world!\");\n}\n".to_vec(),
    );

    files.insert(
        PathBuf::from("src/lib.rs"),
        b"pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n".to_vec(),
    );

    files.insert(
        PathBuf::from("Cargo.toml"),
        b"[package]\nname = \"test\"\nversion = \"0.1.0\"\n".to_vec(),
    );

    files.insert(
        PathBuf::from("README.md"),
        b"# Test Project\n\nThis is a test.\n".to_vec(),
    );

    // Create a binary file to test binary encoding
    files.insert(
        PathBuf::from("data.bin"),
        vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0xFD],
    );

    // Create snapshot
    let temp_dir = std::path::PathBuf::from("/tmp/test-cairn-snapshot");
    std::fs::create_dir_all(&temp_dir)?;

    let hash = create_snapshot(&files, &temp_dir)?;

    println!("Created snapshot: {}", hex::encode(&hash));
    println!(
        "Path: {}/snapshots/{}.vsf",
        temp_dir.display(),
        hex::encode(&hash)
    );

    Ok(())
}
