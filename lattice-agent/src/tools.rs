//! Default tool executor — executes tool calls against the local filesystem.
//!
//! Tool definitions (the schemas that tell the LLM what tools are available)
//! live in [`crate::tool_definitions`]. This module provides the execution
//! layer that runs those tools when the model requests them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use async_trait::async_trait;

use crate::sandbox::SandboxConfig;
use crate::tool_error::ToolError;
use crate::ToolExecutor;

/// Trait abstracting tool registry access. lattice-harness::tools::ToolRegistry implements this.
/// This abstraction lets DefaultToolExecutor (in lattice-agent) use ToolRegistry (in lattice-harness)
/// without a direct crate dependency.
pub trait RegistryToolAccess: Send + Sync {
    fn get_handler(
        &self,
        tool_name: &str,
    ) -> Option<std::sync::Arc<dyn Fn(serde_json::Value) -> String + Send + Sync>>;
    fn get_definitions(&self) -> Vec<lattice_core::types::ToolDefinition>;
}

/// Executes tools using the local filesystem and shell.
///
/// Supports: read_file, grep, write_file, list_directory, bash, patch,
/// web_search, plus bus:fetch when the `blob-store` feature is enabled.
/// The `project_root` is used by `write_file` and `patch`
/// to restrict writes to project source directories.
///
/// All tool operations are gated by the `sandbox` configuration
/// (path validation, sensitive-file blocking, command allowlisting,
/// URL scheme restrictions, and size/timeout limits).
///
/// When a `registry` is set, custom tool handlers from the registry are
/// checked first before falling back to the default 7 tools.
pub struct DefaultToolExecutor {
    pub project_root: String,
    pub sandbox: SandboxConfig,
    pub http_client: reqwest::Client,
    pub registry: Option<std::sync::Arc<dyn RegistryToolAccess>>,
    #[cfg(feature = "blob-store")]
    pub blob_store: Option<std::sync::Arc<crate::blob::BlobStore>>,
}

impl DefaultToolExecutor {
    fn build_http_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("failed to build reqwest client: {e}"))
    }

    pub fn new(project_root: &str) -> Result<Self, String> {
        Ok(Self {
            project_root: project_root.to_string(),
            sandbox: SandboxConfig::default(),
            http_client: Self::build_http_client()?,
            registry: None,
            #[cfg(feature = "blob-store")]
            blob_store: None,
        })
    }

    pub fn new_with_registry(
        project_root: &str,
        registry: Option<std::sync::Arc<dyn RegistryToolAccess>>,
    ) -> Result<Self, String> {
        Ok(Self {
            project_root: project_root.to_string(),
            sandbox: SandboxConfig::default(),
            http_client: Self::build_http_client()?,
            registry,
            #[cfg(feature = "blob-store")]
            blob_store: None,
        })
    }

    /// Override the sandbox config (replaces the default).
    pub fn with_sandbox(mut self, config: SandboxConfig) -> Self {
        self.sandbox = config;
        self
    }

    /// Create a executor with an optional BlobStore for bus:fetch support.
    #[cfg(feature = "blob-store")]
    pub fn new_with_blob_store(
        project_root: &str,
        blob_store: Option<std::sync::Arc<crate::blob::BlobStore>>,
    ) -> Result<Self, String> {
        Ok(Self {
            project_root: project_root.to_string(),
            sandbox: SandboxConfig::default(),
            http_client: Self::build_http_client()?,
            registry: None,
            blob_store,
        })
    }
}

#[async_trait]
impl ToolExecutor for DefaultToolExecutor {
    async fn execute(&self, call: &lattice_core::types::ToolCall) -> String {
        let args: serde_json::Value = match serde_json::from_str(&call.function.arguments) {
            Ok(v) => v,
            Err(e) => {
                return format!(
                    "Error: invalid JSON arguments for tool '{}': {} — raw args: {:?}",
                    call.function.name,
                    e,
                    &call.function.arguments[..call.function.arguments.len().min(200)]
                );
            }
        };

        // Check registry for custom tools first
        if let Some(reg) = &self.registry {
            if let Some(handler) = reg.get_handler(&call.function.name) {
                return handler(args);
            }
        }

        match call.function.name.as_str() {
            "read_file" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                // Canonicalize path to resolve symlinks (TOCTOU prevention)
                let canonical = match tokio::fs::canonicalize(path).await {
                    Ok(p) => p,
                    Err(e) => {
                        return ToolError::IoError {
                            path: path.to_string(),
                            error: e,
                        }
                        .to_string()
                    }
                };
                let path_str = canonical.to_string_lossy();
                if let Err(e) = self.sandbox.check_read(&path_str) {
                    return e;
                }
                match tokio::fs::metadata(&canonical).await {
                    Ok(meta) if meta.len() > self.sandbox.max_read_size as u64 => {
                        return format!(
                            "Sandbox: file size {} exceeds max_read_size {}",
                            meta.len(),
                            self.sandbox.max_read_size
                        );
                    }
                    Err(e) => {
                        return ToolError::IoError {
                            path: path.to_string(),
                            error: e,
                        }
                        .to_string()
                    }
                    _ => {}
                }
                tokio::fs::read_to_string(&canonical)
                    .await
                    .unwrap_or_else(|e| {
                        ToolError::IoError {
                            path: path.to_string(),
                            error: e,
                        }
                        .to_string()
                    })
            }
            "grep" => {
                let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                if let Err(e) = self.sandbox.check_read(path) {
                    return e;
                }

                let re = match regex::Regex::new(pattern) {
                    Ok(r) => r,
                    Err(e) => return ToolError::RegexError(e.to_string()).to_string(),
                };

                let mut results = Vec::new();
                let mut visited = HashSet::new();
                grep_recursive(
                    &re,
                    Path::new(path),
                    &mut results,
                    &self.sandbox,
                    0,
                    &mut visited,
                )
                .await;

                if results.is_empty() {
                    "(no matches)".to_string()
                } else {
                    results.join("\n")
                }
            }
            "write_file" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let abs = std::path::PathBuf::from(format!(
                    "{}/{}",
                    self.project_root,
                    path.trim_start_matches('/')
                ));
                // Canonicalize parent directory (TOCTOU prevention)
                let write_path = if let Some(parent) = abs.parent() {
                    match tokio::fs::canonicalize(parent).await {
                        Ok(canonical_parent) => {
                            let canonical = canonical_parent.join(abs.file_name().unwrap());
                            let path_str = canonical.to_string_lossy();
                            if let Err(e) = self.sandbox.check_write(&path_str) {
                                return e;
                            }
                            canonical
                        }
                        Err(_) => {
                            // Parent doesn't exist — reject write rather than
                            // bypass canonicalize (TOCTOU: symlink could redirect
                            // a non-canonical path after the check)
                            return format!(
                                "Sandbox: cannot write to '{}' — parent directory does not exist or cannot be resolved",
                                path
                            );
                        }
                    }
                } else {
                    if let Err(e) = self.sandbox.check_write(path) {
                        return e;
                    }
                    abs.clone()
                };
                if content.len() > self.sandbox.max_write_size {
                    return ToolError::SizeLimit {
                        limit: self.sandbox.max_write_size,
                        actual: content.len(),
                    }
                    .to_string();
                }
                let path_str = write_path.to_string_lossy().to_string();
                match tokio::fs::write(&write_path, content).await {
                    Ok(_) => format!("Wrote {} bytes to {}", content.len(), path),
                    Err(e) => ToolError::IoError {
                        path: path_str,
                        error: e,
                    }
                    .to_string(),
                }
            }
            "list_directory" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                // Canonicalize path to resolve symlinks (TOCTOU prevention)
                let canonical = match tokio::fs::canonicalize(path).await {
                    Ok(p) => p,
                    Err(e) => {
                        return ToolError::IoError {
                            path: path.to_string(),
                            error: e,
                        }
                        .to_string()
                    }
                };
                let path_str = canonical.to_string_lossy();
                if let Err(e) = self.sandbox.check_read(&path_str) {
                    return e;
                }
                let mut entries = match tokio::fs::read_dir(&canonical).await {
                    Ok(dir) => dir,
                    Err(e) => {
                        return ToolError::IoError {
                            path: path.to_string(),
                            error: e,
                        }
                        .to_string()
                    }
                };
                let mut files = Vec::new();
                loop {
                    match entries.next_entry().await {
                        Ok(Some(entry)) => {
                            let ty = if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
                            {
                                "DIR"
                            } else {
                                "FILE"
                            };
                            files.push(format!("{}  {}", ty, entry.file_name().to_string_lossy()));
                        }
                        Ok(None) => break,
                        Err(e) => {
                            // Log but continue on individual entry errors
                            files.push(format!("Error reading entry: {}", e));
                        }
                    }
                }
                files.sort();
                files.join("\n")
            }
            "bash" => {
                let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                if let Err(e) = self.sandbox.check_command(cmd) {
                    return e;
                }
                let result = execute_bash_command(cmd, &self.sandbox, &self.project_root).await;
                match result {
                    Ok(o) => {
                        let mut cmd_result = String::from_utf8_lossy(&o.stdout).to_string();
                        if !o.stderr.is_empty() {
                            cmd_result
                                .push_str(&format!("\nERR:{}", String::from_utf8_lossy(&o.stderr)));
                        }
                        cmd_result
                    }
                    Err(e) => ToolError::CommandError(e).to_string(),
                }
            }
            "patch" => {
                let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let search = args.get("search").and_then(|v| v.as_str()).unwrap_or("");
                let insert = args.get("insert").and_then(|v| v.as_str()).unwrap_or("");
                let abs = std::path::PathBuf::from(format!(
                    "{}/{}",
                    self.project_root,
                    path.trim_start_matches('/')
                ));
                // Canonicalize path for TOCTOU prevention
                let abs = match tokio::fs::canonicalize(&abs).await {
                    Ok(canonical) => {
                        let path_str = canonical.to_string_lossy();
                        if let Err(e) = self.sandbox.check_write(&path_str) {
                            return e;
                        }
                        canonical
                    }
                    Err(_) => {
                        // File doesn't exist yet — check parent directory instead
                        // to avoid bypassing canonicalize on a non-existent path
                        if let Some(parent) = abs.parent() {
                            match tokio::fs::canonicalize(parent).await {
                                Ok(canonical_parent) => {
                                    let canonical = canonical_parent.join(abs.file_name().unwrap());
                                    let path_str = canonical.to_string_lossy();
                                    if let Err(e) = self.sandbox.check_write(&path_str) {
                                        return e;
                                    }
                                    canonical
                                }
                                Err(_) => {
                                    return format!(
                                        "Sandbox: cannot patch '{}' — path cannot be resolved",
                                        path
                                    );
                                }
                            }
                        } else {
                            return format!("Sandbox: cannot patch '{}' — invalid path", path);
                        }
                    }
                };
                match tokio::fs::read_to_string(&abs).await {
                    Ok(content) => {
                        let count = content.matches(search).count();
                        if count == 0 {
                            format!("Error: search text not found in {}", path)
                        } else if count > 1 {
                            format!(
                                "Error: search text found {} times in {}. Use a more specific search.",
                                count, path
                            )
                        } else {
                            let new_content = content.replace(search, insert);
                            match tokio::fs::write(&abs, &new_content).await {
                                Ok(_) => {
                                    let diff_lines: Vec<String> = new_content
                                        .lines()
                                        .zip(content.lines())
                                        .enumerate()
                                        .filter(|(_, (a, b))| a != b)
                                        .map(|(i, _)| {
                                            let old_line = content.lines().nth(i).unwrap_or("");
                                            let new_line = new_content.lines().nth(i).unwrap_or("");
                                            format!("- {}\n+ {}", old_line, new_line)
                                        })
                                        .collect();
                                    format!("Patched {}. Changes:\n{}", path, diff_lines.join("\n"))
                                }
                                Err(e) => ToolError::IoError {
                                    path: abs.to_string_lossy().to_string(),
                                    error: e,
                                }
                                .to_string(),
                            }
                        }
                    }
                    Err(e) => ToolError::IoError {
                        path: abs.to_string_lossy().to_string(),
                        error: e,
                    }
                    .to_string(),
                }
            }
            "web_search" => {
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if let Err(e) = self.sandbox.check_url(url) {
                    return e;
                }
                match self.http_client.get(url).send().await {
                    Ok(response) => {
                        let limit = self.sandbox.max_http_response_size;
                        if let Some(content_length) = response.content_length() {
                            if content_length > limit as u64 {
                                return ToolError::HttpError(format!(
                                    "Response too large: {} bytes (max {})",
                                    content_length, limit
                                ))
                                .to_string();
                            }
                        }
                        // Stream the response body chunk-by-chunk, aborting at
                        // the size limit. This prevents buffering an unbounded
                        // response into memory before truncation.
                        use futures::StreamExt;
                        let mut buf = Vec::new();
                        let mut exceeded = false;
                        let mut stream = response.bytes_stream();
                        while let Some(chunk) = stream.next().await {
                            let chunk = match chunk {
                                Ok(c) => c,
                                Err(e) => {
                                    return ToolError::HttpError(e.to_string()).to_string();
                                }
                            };
                            if buf.len() + chunk.len() > limit {
                                let remaining = limit - buf.len();
                                buf.extend_from_slice(&chunk[..remaining]);
                                exceeded = true;
                                break;
                            }
                            buf.extend_from_slice(&chunk);
                        }
                        if exceeded {
                            let truncated = String::from_utf8_lossy(&buf);
                            format!(
                                "{}[TRUNCATED: response exceeded {} bytes]",
                                truncated.trim_end(),
                                limit
                            )
                        } else {
                            String::from_utf8_lossy(&buf).to_string()
                        }
                    }
                    Err(e) => ToolError::HttpError(e.to_string()).to_string(),
                }
            }
            #[cfg(feature = "blob-store")]
            "bus:fetch" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if !key.starts_with("blob://") {
                    format!(
                        "Error: invalid blob key '{}', expected format blob://source/topic/hash",
                        key
                    )
                } else {
                    match &self.blob_store {
                        Some(store) => match store.get(key).await {
                            Ok(blob) => blob.payload,
                            Err(_) => format!("Error: blob '{}' not found", key),
                        },
                        None => "Error: blob storage not configured for this agent".to_string(),
                    }
                }
            }
            _ => format!("Unknown tool: {}", call.function.name),
        }
    }
}

const GREP_MAX_DEPTH: u32 = 32;

/// Recursively search files under `path` for lines matching `pattern`.
/// Respects sandbox limits: max_depth, max_read_size, check_read.
/// Skips hidden dirs, binary files, and follows symlinks with cycle detection.
async fn grep_recursive(
    pattern: &regex::Regex,
    path: &Path,
    results: &mut Vec<String>,
    sandbox: &crate::sandbox::SandboxConfig,
    depth: u32,
    visited: &mut HashSet<std::path::PathBuf>,
) {
    if depth > GREP_MAX_DEPTH {
        return;
    }

    // Resolve symlinks to detect cycles AND use canonical path for sandbox checks
    let resolved = match tokio::fs::canonicalize(path).await {
        Ok(p) => p,
        Err(_) => return,
    };
    if !visited.insert(resolved.clone()) {
        return; // symlink cycle
    }

    let path_str = resolved.to_string_lossy();
    if sandbox.check_read(&path_str).is_err() {
        return;
    }

    if resolved.is_file() {
        // Skip files too large
        if let Ok(meta) = tokio::fs::metadata(&resolved).await {
            if meta.len() > sandbox.max_read_size as u64 {
                return;
            }
        }

        if let Ok(content) = tokio::fs::read_to_string(&resolved).await {
            // Skip binary files
            if content.contains('\0') {
                return;
            }
            for (line_num, line) in content.lines().enumerate() {
                if pattern.is_match(line) {
                    results.push(format!("{}:{}:{}", path_str, line_num + 1, line));
                }
            }
        }
    } else if resolved.is_dir() {
        let mut entries = match tokio::fs::read_dir(&resolved).await {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut children = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Skip hidden directories (but not . and ..)
            if name_str.starts_with('.') && name_str != "." && name_str != ".." {
                continue;
            }
            children.push(entry.path());
        }
        for child_path in children {
            Box::pin(grep_recursive(
                pattern,
                &child_path,
                results,
                sandbox,
                depth + 1,
                visited,
            ))
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Platform-specific bash execution helpers (Landlock + setrlimit)
// ---------------------------------------------------------------------------

/// Execute a bash command with platform-specific sandboxing (Unix).
///
/// On Linux with `LandlockConfig` set, applies Landlock file isolation and
/// resource limits via `pre_exec`. On other Unix (macOS, BSD), applies
/// resource limits only.
#[cfg(unix)]
async fn execute_bash_command(
    command: &str,
    sandbox: &SandboxConfig,
    project_root: &str,
) -> Result<std::process::Output, String> {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Linux: Landlock + setrlimit
    #[cfg(target_os = "linux")]
    {
        let read_paths: Option<Vec<PathBuf>> = sandbox
            .landlock
            .as_ref()
            .map(|lc| build_landlock_read_paths(project_root, &lc.extra_read_paths));
        let write_paths: Option<Vec<PathBuf>> = sandbox
            .landlock
            .as_ref()
            .map(|lc| build_landlock_write_paths(project_root, &lc.extra_write_paths));

        unsafe {
            cmd.pre_exec(move || {
                // Apply resource limits first (best-effort, warn on failure)
                apply_resource_limits_linux();

                // Apply Landlock (best-effort; silently skipped on old kernels)
                if let (Some(read), Some(write)) = (&read_paths, &write_paths) {
                    let _ = apply_landlock_sandbox(read, write);
                }

                Ok(())
            });
        }
    }

    // macOS / other Unix: setrlimit only (no Landlock available)
    #[cfg(all(unix, not(target_os = "linux")))]
    unsafe {
        cmd.pre_exec(move || {
            apply_resource_limits_unix();
            Ok(())
        });
    }

    let mut tokio_cmd: tokio::process::Command = cmd.into();
    let timeout_secs = std::cmp::max(sandbox.max_command_timeout, 1) as u64;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio_cmd.output(),
    )
    .await;

    match result {
        Ok(Ok(o)) => Ok(o),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "bash command timed out after {}s",
            sandbox.max_command_timeout
        )),
    }
}

/// Execute a bash command on non-Unix platforms (no sandboxing available).
#[cfg(not(unix))]
async fn execute_bash_command(
    command: &str,
    sandbox: &SandboxConfig,
    _project_root: &str,
) -> Result<std::process::Output, String> {
    let timeout_secs = std::cmp::max(sandbox.max_command_timeout, 1) as u64;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new("sh")
            .args(["-c", command])
            .output(),
    )
    .await;

    match result {
        Ok(Ok(o)) => Ok(o),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "bash command timed out after {}s",
            sandbox.max_command_timeout
        )),
    }
}

// --- Linux Landlock helpers ------------------------------------------------

#[cfg(target_os = "linux")]
fn build_landlock_read_paths(project_root: &str, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(project_root), PathBuf::from("/")];
    paths.extend(extra.iter().cloned());
    paths
}

#[cfg(target_os = "linux")]
fn build_landlock_write_paths(project_root: &str, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(project_root), PathBuf::from("/tmp")];
    paths.extend(extra.iter().cloned());
    paths
}

/// Apply Landlock file isolation in the child process.
///
/// Denies all filesystem access by default, then adds permissive
/// PathBeneath rules: read-only + execute for system paths, full
/// access for write paths (project root + /tmp).
///
/// Returns `Ok(())` on success or if Landlock is not supported
/// (kernel < 5.13). Only returns `Err` for unexpected OS errors.
#[cfg(target_os = "linux")]
fn apply_landlock_sandbox(read_paths: &[PathBuf], write_paths: &[PathBuf]) -> std::io::Result<()> {
    use landlock::{Access, AccessFs, PathBeneath, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI};

    // Progressive ABI: try highest first, fall back step by step.
    // V4 (kernel 6.7+) adds network + io_uring isolation.
    // V3 (kernel 6.0+) adds file truncation control.
    // V2 (kernel 5.19+) adds file reparenting control.
    // V1 (kernel 5.13+) provides filesystem isolation only.
    let ruleset = [ABI::V4, ABI::V3, ABI::V2, ABI::V1]
        .iter()
        .find_map(|&abi| {
            Ruleset::default()
                .handle_access(AccessFs::from_all(abi))
                .ok()
                .and_then(|r| r.create().ok())
        });

    let ruleset = match ruleset {
        Some(r) => r,
        None => return Ok(()), // Landlock not supported on this kernel
    };

    // Use the ABI that was actually accepted for PathBeneath rules
    let abi = ABI::V1; // V1-compatible read/write flags work on all ABIs

    // Collect PathBeneath rules (needs file descriptors, not just paths)
    let mut rules: Vec<PathBeneath<std::fs::File>> = Vec::new();

    // Read-only + execute for read paths (only existing paths)
    for path in read_paths {
        if let Ok(file) = std::fs::File::open(path) {
            rules.push(PathBeneath::new(
                file,
                AccessFs::from_read(abi) | AccessFs::Execute,
            ));
        }
    }

    // Full access for write paths (only existing paths)
    for path in write_paths {
        if let Ok(file) = std::fs::File::open(path) {
            rules.push(PathBeneath::new(file, AccessFs::from_all(abi)));
        }
    }

    // add_rules takes an iterator of Result<T, E> where T: Rule<U>
    let rules_iter = rules
        .into_iter()
        .map(|r| -> Result<_, landlock::RulesetError> { Ok(r) });

    let ruleset = match ruleset.add_rules(rules_iter) {
        Ok(r) => r,
        Err(_) => return Ok(()), // Failed to add rules, skip Landlock
    };

    // Enforce the ruleset (returns RestrictionStatus; ignore on failure)
    let _ = ruleset.restrict_self();

    Ok(())
}

// --- Resource limit helpers ------------------------------------------------

/// Apply resource limits in the child process (Linux).
///
/// - RLIMIT_AS: 512 MB virtual memory
/// - RLIMIT_NPROC: 1 process
/// - RLIMIT_CPU: 30 seconds
///
/// Failures are silently ignored (logged at trace level).
#[cfg(target_os = "linux")]
fn apply_resource_limits_linux() {
    let rlim_as = libc::rlimit {
        rlim_cur: 512 * 1024 * 1024,
        rlim_max: 512 * 1024 * 1024,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim_as) } != 0 {
        tracing::warn!("setrlimit(RLIMIT_AS) failed in child process");
    }

    let rlim_nproc = libc::rlimit {
        rlim_cur: 1,
        rlim_max: 1,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlim_nproc) } != 0 {
        tracing::warn!("setrlimit(RLIMIT_NPROC) failed in child process");
    }

    let rlim_cpu = libc::rlimit {
        rlim_cur: 30,
        rlim_max: 30,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &rlim_cpu) } != 0 {
        tracing::warn!("setrlimit(RLIMIT_CPU) failed in child process");
    }
}

/// Apply resource limits in the child process (macOS / other Unix).
///
/// - RLIMIT_AS: 512 MB virtual memory
/// - RLIMIT_NPROC: 32 (macOS limits entire process tree, not individual)
/// - RLIMIT_CPU: 30 seconds
///
/// Failures are silently ignored (logged at trace level).
#[cfg(all(unix, not(target_os = "linux")))]
fn apply_resource_limits_unix() {
    let rlim_as = libc::rlimit {
        rlim_cur: 512 * 1024 * 1024,
        rlim_max: 512 * 1024 * 1024,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim_as) } != 0 {
        tracing::warn!("setrlimit(RLIMIT_AS) failed in child process");
    }

    let rlim_nproc = libc::rlimit {
        rlim_cur: 32,
        rlim_max: 32,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlim_nproc) } != 0 {
        tracing::warn!("setrlimit(RLIMIT_NPROC) failed in child process");
    }

    let rlim_cpu = libc::rlimit {
        rlim_cur: 30,
        rlim_max: 30,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &rlim_cpu) } != 0 {
        tracing::warn!("setrlimit(RLIMIT_CPU) failed in child process");
    }
}
