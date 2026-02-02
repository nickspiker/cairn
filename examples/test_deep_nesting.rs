use cairn::snapshot_vsf::create_snapshot;
use std::collections::HashMap;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    // Create test files with deeper nesting
    let mut files = HashMap::new();

    // Root files
    files.insert(
        PathBuf::from("README.md"),
        b"# Test Project\n\nDeep nesting test.\n".to_vec(),
    );
    files.insert(
        PathBuf::from("Cargo.toml"),
        b"[package]\nname = \"test\"\nversion = \"0.1.0\"\n".to_vec(),
    );

    // Deeply nested source files
    files.insert(
        PathBuf::from("src/main.rs"),
        b"fn main() {\n    println!(\"Hello\");\n}\n".to_vec(),
    );
    files.insert(
        PathBuf::from("src/lib.rs"),
        b"pub mod commands;\npub mod utils;\n".to_vec(),
    );
    files.insert(
        PathBuf::from("src/commands/mod.rs"),
        b"pub mod git;\n".to_vec(),
    );
    files.insert(
        PathBuf::from("src/commands/git/init.rs"),
        b"pub fn init() {\n    println!(\"git init\");\n}\n".to_vec(),
    );
    files.insert(
        PathBuf::from("src/commands/git/patch.rs"),
        b"pub fn patch() {\n    println!(\"git patch\");\n}\n".to_vec(),
    );
    files.insert(
        PathBuf::from("src/utils/format.rs"),
        b"pub fn format(s: &str) -> String {\n    s.to_uppercase()\n}\n".to_vec(),
    );

    // Test files
    files.insert(
        PathBuf::from("tests/integration/test_git.rs"),
        b"#[test]\nfn test_init() {\n    assert!(true);\n}\n".to_vec(),
    );

    // Binary file
    files.insert(
        PathBuf::from("assets/icon.png"),
        vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], // PNG header
    );

    println!("Creating snapshot with {} files", files.len());

    // Create snapshot
    let temp_dir = std::path::PathBuf::from("/tmp/test-deep-cairn");
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
