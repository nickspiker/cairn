//! Cairn daemon - long-running process for VSCode extension IPC

use anyhow::{Context, Result};
use shared_memory::{Shmem, ShmemConf};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::sync::Arc;

// Shared memory layout
const SHM_SIZE: usize = 2060;
const TX_BUFFER_OFFSET: usize = 12;
const TX_BUFFER_SIZE: usize = 1024;
const RX_BUFFER_OFFSET: usize = 12 + TX_BUFFER_SIZE;
const RX_BUFFER_SIZE: usize = 1024;

// Event flags
const EVENT_PATCHES_CHANGED: u32 = 0x01;
const EVENT_BUILD_STARTED: u32 = 0x02;
const EVENT_BUILD_COMPLETED: u32 = 0x04;

/// Shared memory wrapper for daemon IPC
struct DaemonSharedMem {
    shmem: Shmem,
}

impl DaemonSharedMem {
    /// Create and initialize shared memory
    fn create() -> Result<Self> {
        let shmem = ShmemConf::new()
            .size(SHM_SIZE)
            .os_id("cairn-daemon-shm")
            .create()
            .context("Failed to create shared memory")?;

        // Initialize control block to zeros
        unsafe {
            let ptr = shmem.as_ptr();
            std::ptr::write_bytes(ptr, 0, 12);
        }

        Ok(Self { shmem })
    }

    /// Get tx_ready flag
    fn tx_ready(&self) -> &AtomicU32 {
        unsafe {
            &*(self.shmem.as_ptr() as *const AtomicU32)
        }
    }

    /// Get rx_ready flag
    fn rx_ready(&self) -> &AtomicU32 {
        unsafe {
            &*(self.shmem.as_ptr().add(4) as *const AtomicU32)
        }
    }

    /// Get events flag
    fn events(&self) -> &AtomicU32 {
        unsafe {
            &*(self.shmem.as_ptr().add(8) as *const AtomicU32)
        }
    }

    /// Get tx buffer (read-only)
    fn tx_buffer(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self.shmem.as_ptr().add(TX_BUFFER_OFFSET),
                TX_BUFFER_SIZE
            )
        }
    }

    /// Get rx buffer (mutable)
    fn rx_buffer_mut(&self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.shmem.as_ptr().add(RX_BUFFER_OFFSET) as *mut u8,
                RX_BUFFER_SIZE
            )
        }
    }

    /// Set an event flag
    fn set_event(&self, event: u32) {
        self.events().fetch_or(event, Ordering::Release);
        // Wake extension
        self.futex_wake(self.events());
    }

    /// Futex wake operation
    fn futex_wake(&self, atomic: &AtomicU32) {
        unsafe {
            let ptr = atomic as *const AtomicU32 as *const i32;
            libc::syscall(
                libc::SYS_futex,
                ptr,
                libc::FUTEX_WAKE,
                1, // Wake 1 waiter
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<i32>(),
                0
            );
        }
    }

    /// Futex wait operation
    fn futex_wait(&self, atomic: &AtomicU32, expected: u32) {
        unsafe {
            let ptr = atomic as *const AtomicU32 as *const i32;
            libc::syscall(
                libc::SYS_futex,
                ptr,
                libc::FUTEX_WAIT,
                expected as i32,
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<i32>(),
                0
            );
        }
    }
}

/// Request from client (parsed from VSF)
struct DaemonRequest {
    command: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
}

/// Response to client
enum ResponseStatus {
    Success = 0,
    Error = 1,
}

struct DaemonResponse {
    status: ResponseStatus,
    message: String,
    data: Option<String>,
}

impl DaemonResponse {
    fn success(message: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::Success,
            message: message.into(),
            data: None,
        }
    }

    fn success_with_data(message: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::Success,
            message: message.into(),
            data: Some(data.into()),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::Error,
            message: message.into(),
            data: None,
        }
    }
}

/// Start the daemon server
pub fn start_daemon() -> Result<()> {
    let shm = DaemonSharedMem::create()?;

    println!("✓ Cairn daemon started");
    println!("  Shared memory: /dev/shm/cairn-daemon-shm");
    println!("  Waiting for commands...");

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Handle Ctrl+C gracefully
    ctrlc::set_handler(move || {
        println!("\n✓ Shutting down daemon...");
        r.store(false, Ordering::SeqCst);
    }).ok();

    // Main loop: poll for commands
    while running.load(Ordering::SeqCst) {
        // Poll tx_ready flag
        while shm.tx_ready().load(Ordering::Acquire) == 0 {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            // Sleep briefly to avoid busy-waiting
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        // Read and decode VSF request from tx_buffer
        let tx_data = shm.tx_buffer();

        match decode_daemon_request(tx_data) {
            Ok(request) => {
                println!("[DAEMON] Command: {} {:?}", request.command, request.args);

                // Execute command
                let response = match request.command.as_str() {
                    "ping" => DaemonResponse::success("pong"),
                    "jump" => handle_jump(&request),
                    "build" => handle_build(&request, &shm),
                    "test" => handle_test(&request, &shm),
                    "check" => handle_check(&request),
                    "run" => handle_run(&request),
                    "clear" => handle_clear(&request, &shm),
                    "list" => handle_list(&request),
                    _ => DaemonResponse::error(format!("Unknown command: {}", request.command)),
                };

                // Encode VSF response to rx_buffer
                match encode_daemon_response(&response) {
                    Ok(response_bytes) => {
                        let rx_buf = shm.rx_buffer_mut();
                        let len = response_bytes.len().min(RX_BUFFER_SIZE);
                        rx_buf[..len].copy_from_slice(&response_bytes[..len]);

                        // Signal response ready
                        shm.rx_ready().store(1, Ordering::Release);
                        shm.futex_wake(shm.rx_ready());
                    }
                    Err(e) => {
                        eprintln!("[DAEMON] Failed to encode response: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("[DAEMON] Failed to decode request: {}", e);
            }
        }

        // Clear tx_ready
        shm.tx_ready().store(0, Ordering::Release);
    }

    println!("✓ Daemon stopped");
    Ok(())
}

/// Decode VSF request from buffer
fn decode_daemon_request(buf: &[u8]) -> Result<DaemonRequest> {
    // TODO: Implement VSF decoding
    // For now, parse a simple ASCII format:
    // Format: "command arg1 arg2\n/path/to/cwd"

    let s = std::str::from_utf8(buf)
        .context("Invalid UTF-8 in request")?
        .trim_end_matches('\0');

    let lines: Vec<&str> = s.split('\n').collect();
    if lines.is_empty() {
        anyhow::bail!("Empty request");
    }

    let parts: Vec<String> = lines[0].split_whitespace().map(|s| s.to_string()).collect();
    if parts.is_empty() {
        anyhow::bail!("No command specified");
    }

    let command = parts[0].clone();
    let args = parts[1..].to_vec();
    let cwd = if lines.len() > 1 && !lines[1].is_empty() {
        Some(PathBuf::from(lines[1]))
    } else {
        None
    };

    Ok(DaemonRequest { command, args, cwd })
}

/// Encode VSF response to buffer
fn encode_daemon_response(resp: &DaemonResponse) -> Result<Vec<u8>> {
    // TODO: Implement VSF encoding
    // For now, use a simple ASCII format:
    // Format: "status\nmessage\ndata"

    let status = match resp.status {
        ResponseStatus::Success => "Success",
        ResponseStatus::Error => "Error",
    };

    let mut result = format!("{}\n{}", status, resp.message);
    if let Some(data) = &resp.data {
        result.push('\n');
        result.push_str(data);
    }

    Ok(result.into_bytes())
}

/// Handle jump command
fn handle_jump(req: &DaemonRequest) -> DaemonResponse {
    if req.args.is_empty() {
        return DaemonResponse::error("Missing patch hash argument");
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

/// Handle build command
fn handle_build(req: &DaemonRequest, shm: &DaemonSharedMem) -> DaemonResponse {
    let is_release = req.args.contains(&"--release".to_string());
    let mode = if is_release { "release" } else { "debug" };

    shm.set_event(EVENT_BUILD_STARTED);

    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("build")
        .args(&req.args)
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    let response = match output {
        Ok(output) if output.status.success() => {
            shm.set_event(EVENT_BUILD_COMPLETED | EVENT_PATCHES_CHANGED);
            DaemonResponse::success(format!("Build ({}) succeeded", mode))
        }
        Ok(output) => {
            shm.set_event(EVENT_BUILD_COMPLETED);
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Build failed: {}", stderr))
        }
        Err(e) => {
            shm.set_event(EVENT_BUILD_COMPLETED);
            DaemonResponse::error(format!("Failed to run build: {}", e))
        }
    };

    response
}

/// Handle test command
fn handle_test(req: &DaemonRequest, shm: &DaemonSharedMem) -> DaemonResponse {
    shm.set_event(EVENT_BUILD_STARTED);

    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("test")
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    let response = match output {
        Ok(output) if output.status.success() => {
            shm.set_event(EVENT_BUILD_COMPLETED | EVENT_PATCHES_CHANGED);
            DaemonResponse::success("Tests passed")
        }
        Ok(output) => {
            shm.set_event(EVENT_BUILD_COMPLETED);
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Tests failed: {}", stderr))
        }
        Err(e) => {
            shm.set_event(EVENT_BUILD_COMPLETED);
            DaemonResponse::error(format!("Failed to run tests: {}", e))
        }
    };

    response
}

/// Handle check command
fn handle_check(req: &DaemonRequest) -> DaemonResponse {
    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("check")
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    match output {
        Ok(output) if output.status.success() => {
            DaemonResponse::success("Check succeeded")
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Check failed: {}", stderr))
        }
        Err(e) => DaemonResponse::error(format!("Failed to run check: {}", e)),
    }
}

/// Handle run command
fn handle_run(req: &DaemonRequest) -> DaemonResponse {
    let is_release = req.args.contains(&"--release".to_string());
    let mode = if is_release { "release" } else { "debug" };

    let output = std::process::Command::new("cargo")
        .arg("cairn")
        .arg("run")
        .args(&req.args)
        .current_dir(req.cwd.as_ref().unwrap_or(&PathBuf::from(".")))
        .output();

    match output {
        Ok(output) if output.status.success() => {
            DaemonResponse::success(format!("Run ({}) succeeded", mode))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DaemonResponse::error(format!("Run failed: {}", stderr))
        }
        Err(e) => DaemonResponse::error(format!("Failed to run: {}", e)),
    }
}

/// Handle clear command
fn handle_clear(req: &DaemonRequest, shm: &DaemonSharedMem) -> DaemonResponse {
    let cairn_dir = req.cwd.as_ref()
        .map(|p| p.join(".cairn"))
        .unwrap_or_else(|| PathBuf::from(".cairn"));

    if !cairn_dir.exists() {
        return DaemonResponse::error("No .cairn directory found");
    }

    match std::fs::remove_dir_all(&cairn_dir) {
        Ok(()) => {
            shm.set_event(EVENT_PATCHES_CHANGED);
            DaemonResponse::success("Cleared cairn directory")
        }
        Err(e) => DaemonResponse::error(format!("Failed to clear: {}", e)),
    }
}

/// Handle list command
fn handle_list(req: &DaemonRequest) -> DaemonResponse {
    let cairn_dir = req.cwd.as_ref()
        .map(|p| p.join(".cairn"))
        .unwrap_or_else(|| PathBuf::from(".cairn"));

    let patches_dir = cairn_dir.join("patches");
    if !patches_dir.exists() {
        return DaemonResponse::success_with_data("No patches yet", "[]");
    }

    match std::fs::read_dir(&patches_dir) {
        Ok(entries) => {
            let mut patches: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s == "vsf")
                        .unwrap_or(false)
                })
                .collect();

            patches.sort_by(|a, b| {
                let a_time = a.metadata().and_then(|m| m.modified()).ok();
                let b_time = b.metadata().and_then(|m| m.modified()).ok();
                b_time.cmp(&a_time) // Newest first
            });

            let patch_list: Vec<String> = patches
                .iter()
                .map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .trim_end_matches(".vsf")
                        .to_string()
                })
                .collect();

            // Simple format for now (not JSON, will use VSF later)
            let data = patch_list.join(",");
            DaemonResponse::success_with_data(
                format!("{} patches", patch_list.len()),
                data
            )
        }
        Err(e) => DaemonResponse::error(format!("Failed to read patches: {}", e)),
    }
}

/// Check if daemon is running
pub fn is_daemon_running() -> bool {
    // Try to open existing shared memory
    ShmemConf::new()
        .os_id("cairn-daemon-shm")
        .open()
        .is_ok()
}
