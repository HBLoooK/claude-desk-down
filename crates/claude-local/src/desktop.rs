//! Find locally installed original Claude Desktop, read its version. Codex++'s localized copy (patched Desktop) also uses this.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

/// Max duration for a single probe (launch PowerShell / defaults etc.).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

/// Probe output goes to temp file to avoid child process blocking on full pipe; timeout reliably terminates and reaps child.
#[derive(Debug)]
pub struct ProbeOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
}

pub fn probe_output(command: &mut Command, deadline: Instant) -> Result<ProbeOutput> {
    ensure!(Instant::now() < deadline, "Claude detection timeout, please retry");
    let path = std::env::temp_dir().join(format!("codex-plus-claude-probe-{}.tmp", uuid::Uuid::new_v4()));
    struct Capture(PathBuf);
    impl Drop for Capture {
        fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
    }
    let mut capture = fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    // Handle must be closed before removal (Windows doesn't allow deleting open files).
    let cleanup = Capture(path);
    let result = (|| {
        command.stdin(Stdio::null()).stdout(capture.try_clone()?).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().context("Failed to launch Claude detection program")?;
        let deadline = deadline.min(Instant::now() + PROBE_TIMEOUT);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Err(error) = result { return Err(error.into()); }
                    bail!("Claude detection timeout, detection process ended, please retry");
                }
            }
        };
        capture.seek(SeekFrom::Start(0))?;
        let mut stdout = Vec::new();
        capture.by_ref().take(64 * 1024).read_to_end(&mut stdout)?;
        Ok(ProbeOutput { status, stdout })
    })();
    // Command itself also holds redirected handles; cleanup after they're released.
    command.stdout(Stdio::null());
    drop(capture);
    drop(cleanup);
    result
}


#[cfg(target_os = "macos")]
pub fn source_app(_deadline: Instant) -> Result<PathBuf> {
    let system = PathBuf::from("/Applications/Claude.app");
    if system.is_dir() {
        return Ok(system);
    }
    Ok(home_dir().join("Applications/Claude.app"))
}

#[cfg(target_os = "windows")]
pub fn source_app(deadline: Instant) -> Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(home_dir);
    let program = std::env::var_os("ProgramFiles").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let query = probe_output(Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; @(Get-AppxPackage -Name Claude; Get-AppxPackage -Name AnthropicPBC.Claude) | Select-Object Name,InstallLocation | ConvertTo-Json -Compress"]), deadline);
    if let Ok(output) = &query {
        if output.status.success() {
            for install in claude_package_locations(&output.stdout)? {
                let packaged = PathBuf::from(install).join(r"app\Claude.exe");
                if packaged.is_file() { return Ok(packaged); }
            }
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    // Squirrel installs (current Claude Desktop) live in %LOCALAPPDATA%\AnthropicClaude\app-<version>\claude.exe.
    let anthropic = local.join("AnthropicClaude");
    if let Ok(entries) = fs::read_dir(&anthropic) {
        let mut versioned: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("app-")))
            .map(|path| path.join("claude.exe"))
            .filter(|path| path.is_file())
            .collect();
        versioned.sort_by(|left, right| right.cmp(left));
        candidates.extend(versioned);
    }
    candidates.extend([
        anthropic.join("claude.exe"),
        local.join(r"Microsoft\WindowsApps\claude-desktop.exe"),
        local.join(r"Microsoft\WindowsApps\Claude_pzs8sxrjxfjjc\claude-desktop.exe"),
        local.join(r"Programs\Claude\Claude.exe"),
        local.join(r"Programs\claude-desktop\Claude.exe"),
        local.join(r"Claude\Claude.exe"),
        program.join(r"Claude\Claude.exe"),
    ]);
    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) { return Ok(path); }
    // Query failure != "not installed", return explicit error to avoid mistaking timeout for not installed.
    let output = query?;
    ensure!(output.status.success(), "Failed to detect Claude install location, check Windows app package service and retry");
    Ok(local.join(r"Programs\Claude\Claude.exe"))
}

#[cfg(any(target_os = "windows", test))]
fn claude_package_locations(bytes: &[u8]) -> Result<Vec<String>> {
    // PowerShell outputs empty for zero, object for one, array for many.
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("Claude app package query returned invalid JSON")?;
    let records = match &value {
        serde_json::Value::Array(records) => records.as_slice(),
        serde_json::Value::Null => &[],
        _ => std::slice::from_ref(&value),
    };
    Ok(records
        .iter()
        .filter_map(|record| {
            let name = record.get("Name")?.as_str()?;
            if !["Claude", "AnthropicPBC.Claude"]
                .iter()
                .any(|known| name.eq_ignore_ascii_case(known))
            {
                return None;
            }
            let path = record.get("InstallLocation")?.as_str()?.trim();
            (!path.is_empty()).then(|| path.to_string())
        })
        .collect())
}

/// Whether path is a Windows Store (MSIX) Claude entry point; other platforms always false (path won't have WindowsApps).
pub fn is_claude_msix_path(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('/', "\\");
    let parts = text
        .split('\\')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some(index) = parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("WindowsApps"))
    else {
        return false;
    };
    let tail = &parts[index + 1..];
    if tail.len() == 1 && tail[0].eq_ignore_ascii_case("claude-desktop.exe") {
        return true; // Registered App Execution Alias.
    }
    let Some(package) = tail.first() else {
        return false;
    };
    let known = ["Claude_", "AnthropicPBC.Claude_"].iter().any(|prefix| {
        package
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
    });
    known
        && ((tail.len() == 3
            && tail[1].eq_ignore_ascii_case("app")
            && tail[2].eq_ignore_ascii_case("Claude.exe"))
            || (tail.len() == 2 && tail[1].eq_ignore_ascii_case("claude-desktop.exe")))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn source_app(_deadline: Instant) -> Result<PathBuf> {
    Ok(PathBuf::from("Claude.exe"))
}

fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_default()
}


#[cfg(target_os = "macos")]
pub fn app_version(app: &Path, key: &str, deadline: Instant) -> Result<String> {
    if !app.is_dir() { return Ok(String::new()); }
    let output = probe_output(Command::new("/usr/bin/defaults")
        .args(["read"]).arg(app.join("Contents/Info")).arg(key), deadline)?;
    Ok(if output.status.success() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { String::new() })
}

#[cfg(target_os = "windows")]
pub fn app_version(app: &Path, _key: &str, deadline: Instant) -> Result<String> {
    if !app.is_file() { return Ok(String::new()); }
    let escaped = app.to_string_lossy().replace('\'', "''");
    let output = probe_output(Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!("(Get-Item -LiteralPath '{escaped}').VersionInfo.ProductVersion")), deadline)?;
    Ok(if output.status.success() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { String::new() })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn app_version(_app: &Path, _key: &str, _deadline: Instant) -> Result<String> { Ok(String::new()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_msix_namespace_discovery_and_classification_agree() {
        let packages = br#"[{"Name":"AnthropicPBC.Claude","InstallLocation":"C:\\Program Files\\WindowsApps\\AnthropicPBC.Claude_2.42304.0.0_x64__fnn82j28hfe8t"},{"Name":"ClaudeFork","InstallLocation":"C:\\Other"}]"#;
        let locations = claude_package_locations(packages).unwrap();
        assert_eq!(locations.len(), 1);
        assert!(is_claude_msix_path(Path::new(&format!(
            "{}\\app\\Claude.exe",
            locations[0]
        ))));
        assert_eq!(
            claude_package_locations(br#"{"Name":"Claude","InstallLocation":"C:\\legacy"}"#)
                .unwrap(),
            vec![r"C:\legacy"]
        );
        assert!(claude_package_locations(b"").unwrap().is_empty());
        assert!(claude_package_locations(b"not JSON").is_err());
    }
}