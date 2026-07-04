//! Cairn daemon - long-running process for VSCode extension IPC via stdin/stdout

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write, BufRead, BufReader};
use std::path::PathBuf;

// Command bytes (single byte commands)
const CMD_PING: u8   = 0x01;
const CMD_JUMP: u8   = 0x02;
const CMD_BUILD: u8  = 0x04;
const CMD_CLEAR: u8  = 0x08;
const CMD_TEST: u8   = 0x10;
const CMD_CHECK: u8  = 0x20;
const CMD_RUN: u8    = 0x40;
const CMD_LIST: u8   = 0x80;

#[derive(Debug, Serialize, Deserialize)]
struct DaemonRequest {
    cwd: Option<PathBuf>,
    args: Vec<String>,
}

#[derive(Debug, Serialize)]
enum ResponseStatus {
    Success,
    Error,
}

#[derive(Debug, Serialize)]
struct DaemonResponse {
    status: ResponseStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

impl DaemonResponse {
    fn success<S: Into<String>>(message: S) -> Self {
        Self {
            status: ResponseStatus::Success,
            message: message.into(),
            data: None,
        }
    }

    fn error<S: Into<String>>(message: S) -> Self {
        Self {
            status: ResponseStatus::Error,
            message: message.into(),
            data: None,
        }
    }

    fn with_data<S: Into<String>>(mut self, data: S) -> Self {
        self.data = Some(data.into());
        self
    }
}

/// Run the daemon main loop
pub fn run_daemon() -> Result<()> {
    eprintln!("✓ Cairn daemon started");
    eprintln!("  Listening on stdin/stdout for commands...");

    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::stdout();

    loop {
        // Read single command byte
        let mut cmd_byte = [0u8; 1];
        match stdin.read_exact(&mut cmd_byte) {
            Ok(_) => {},
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                eprintln!("✓ Daemon stopped (stdin closed)");
                break;
            }
            Err(e) => {
                eprintln!("[DAEMON] Error reading command: {}", e);
                continue;
            }
        }

        let cmd = cmd_byte[0];
        eprintln!("[DAEMON] Received command: 0x{:02x}", cmd);

        // Read request data (JSON, newline-terminated)
        let mut request_line = String::new();
        let mut reader = BufReader::new(&mut stdin);
        if let Err(e) = reader.read_line(&mut request_line) {
            eprintln!("[DAEMON] Error reading request: {}", e);
            continue;
        }

        let request: DaemonRequest = match serde_json::from_str(&request_line.trim()) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("[DAEMON] Error parsing request: {}", e);
                let response = DaemonResponse::error(format!("Invalid request: {}", e));
                write_response(&mut stdout, &response);
                continue;
            }
        };

        // Dispatch command
        let response = match cmd {
            CMD_PING => handle_ping(),
            CMD_JUMP => handle_jump(&request),
            CMD_BUILD => handle_build(&request),
            CMD_CLEAR => handle_clear(&request),
            CMD_TEST => handle_test(&request),
            CMD_CHECK => handle_check(&request),
            CMD_RUN => handle_run(&request),
            CMD_LIST => handle_list(&request),
            _ => DaemonResponse::error(format!("Unknown command: 0x{:02x}", cmd)),
        };

        write_response(&mut stdout, &response);
    }

    Ok(())
}

fn write_response(stdout: &mut io::Stdout, response: &DaemonResponse) {
    match serde_json::to_string(&response) {
        Ok(json) => {
            if let Err(e) = writeln!(stdout, "{}", json) {
                eprintln!("[DAEMON] Error writing response: {}", e);
            }
            if let Err(e) = stdout.flush() {
                eprintln!("[DAEMON] Error flushing stdout: {}", e);
            }
        }
        Err(e) => {
            eprintln!("[DAEMON] Error serializing response: {}", e);
        }
    }
}

/// Handle PING command
fn handle_ping() -> DaemonResponse {
    eprintln!("[DAEMON] PING");
    DaemonResponse::success("pong")
}

/// Handle JUMP command
fn handle_jump(req: &DaemonRequest) -> DaemonResponse {
    if req.args.is_empty() {
        return DaemonResponse::error("No patch hash provided");
    }

    let patch_hash = &req.args[0];
    let cairn_dir = req.cwd.as_ref()
        .map(|p| p.join(".cairn"))
        .unwrap_or_else(|| PathBuf::from(".cairn"));

    match crate::jump::jump_to_patch(&cairn_dir, patch_hash) {
        Ok(()) => {
            let short_hash = &patch_hash[..8.min(patch_hash.len())];
            DaemonResponse::success(format!("Switched to patch {}", short_hash))
        }
        Err(e) => DaemonResponse::error(format!("Jump failed: {}", e)),
    }
}

/// Handle BUILD command
fn handle_build(req: &DaemonRequest) -> DaemonResponse {
    let is_release = req.args.contains(&"--release".to_string());
    let mode = if is_release { "release" } else { "debug" };

    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("build")
        .args(&req.args)
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    match output {
        Ok(output) if output.status.success() => {
            DaemonResponse::success(format!("Build ({}) succeeded", mode))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Build failed: {}", stderr))
        }
        Err(e) => {
            DaemonResponse::error(format!("Failed to run build: {}", e))
        }
    }
}

/// Handle TEST command
fn handle_test(req: &DaemonRequest) -> DaemonResponse {
    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("test")
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    match output {
        Ok(output) if output.status.success() => {
            DaemonResponse::success("Tests passed")
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Tests failed: {}", stderr))
        }
        Err(e) => {
            DaemonResponse::error(format!("Failed to run tests: {}", e))
        }
    }
}

/// Handle CHECK command
fn handle_check(req: &DaemonRequest) -> DaemonResponse {
    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("check")
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    match output {
        Ok(output) if output.status.success() => {
            DaemonResponse::success("Check passed")
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Check failed: {}", stderr))
        }
        Err(e) => {
            DaemonResponse::error(format!("Failed to run check: {}", e))
        }
    }
}

/// Handle RUN command
fn handle_run(req: &DaemonRequest) -> DaemonResponse {
    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("run")
        .args(&req.args)
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            DaemonResponse::success("Run completed").with_data(stdout.to_string())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Run failed: {}", stderr))
        }
        Err(e) => {
            DaemonResponse::error(format!("Failed to run: {}", e))
        }
    }
}

/// Handle CLEAR command
fn handle_clear(req: &DaemonRequest) -> DaemonResponse {
    let cairn_dir = req.cwd.as_ref()
        .map(|p| p.join(".cairn"))
        .unwrap_or_else(|| PathBuf::from(".cairn"));

    if !cairn_dir.exists() {
        return DaemonResponse::error("No .cairn directory found");
    }

    match std::fs::remove_dir_all(&cairn_dir) {
        Ok(()) => {
            DaemonResponse::success("Cleared cairn directory")
        }
        Err(e) => DaemonResponse::error(format!("Failed to clear: {}", e)),
    }
}

/// Handle LIST command
fn handle_list(req: &DaemonRequest) -> DaemonResponse {
    let cairn_dir = req.cwd.as_ref()
        .map(|p| p.join(".cairn"))
        .unwrap_or_else(|| PathBuf::from(".cairn"));

    if !cairn_dir.join("vault").exists() {
        return DaemonResponse::success("No patches").with_data("[]");
    }

    // Chronological patch list from the committed repository state.
    let patches = match crate::vault::CairnVault::open_existing(&cairn_dir)
        .and_then(|mut v| crate::state::RepositoryState::load(&mut v))
    {
        Ok(state) => state.patches,
        Err(e) => {
            return DaemonResponse::error(format!("Failed to list patches: {}", e));
        }
    };

    match serde_json::to_string(&patches) {
        Ok(json) => DaemonResponse::success(format!("Found {} patches", patches.len())).with_data(json),
        Err(e) => DaemonResponse::error(format!("Failed to serialize patches: {}", e)),
    }
}
