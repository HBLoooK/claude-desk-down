//! File utilities: atomic write (same implementation as Codex++'s settings::atomic_write).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

pub fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    atomic_write_with(path, |file| file.write_all(bytes))
}

/// Streaming atomic write: content written directly to temp file by `write_contents`,
/// caller doesn't need to assemble full bytes in memory. Large files (e.g. history
/// session rollout JSONL) use this path.
pub fn atomic_write_with(
    path: &Path,
    write_contents: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> anyhow::Result<()> {
    // If target is a symlink (common when ~/.codex/config.toml linked to dotfiles repo),
    // atomic replace must land on the real file the link points to; direct rename would
    // turn the symlink itself into a regular file, silently breaking the link —
    // the original file in dotfiles stops receiving updates.
    let resolved = resolve_symlink_target(path);
    let path = resolved.as_deref().unwrap_or(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    // Preserve original file permissions: temp file default permissions may differ.
    let existing_permissions = match fs::metadata(path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read permissions for {}", path.display()));
        }
    };
    let temp_path = temp_path_for(path);
    let write_result = (|| -> std::io::Result<()> {
        let mut temp_file = File::create(&temp_path)?;
        write_contents(&mut temp_file)?;
        temp_file.flush()?;
        if let Some(permissions) = existing_permissions {
            temp_file.set_permissions(permissions)?;
        }
        // Must sync to disk before rename: flush alone may leave content in page cache,
        // power loss or crash after rename would leave a config.toml full of NULs (#2450).
        temp_file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to write temp file {}", temp_path.display()));
    }
    if let Err(error) = replace_file(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| {
            format!(
                "failed to replace {} with {}",
                path.display(),
                temp_path.display()
            )
        });
    }
    Ok(())
}

/// Resolve symlink chain (max 16 levels, cycle protection), return final target path;
/// returns None if not a symlink.
fn resolve_symlink_target(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut resolved = false;
    for _ in 0..16 {
        let Ok(metadata) = fs::symlink_metadata(&current) else { break };
        if !metadata.file_type().is_symlink() {
            break;
        }
        let Ok(target) = fs::read_link(&current) else { break };
        current = if target.is_absolute() {
            target
        } else {
            current.parent().map(|parent| parent.join(&target)).unwrap_or(target)
        };
        resolved = true;
    }
    resolved.then_some(current)
}

fn temp_path_for(path: &Path) -> PathBuf {
    let mut temp_path = path.to_path_buf();
    let extension = path.extension().and_then(|value| value.to_str());
    temp_path.set_extension(match extension {
        Some(extension) => format!("{extension}.tmp"),
        None => "tmp".to_string(),
    });
    temp_path
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> anyhow::Result<()> {
    fs::rename(source, target)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // Codex client / antivirus may briefly hold a read handle on target file,
    // MoveFileExW fails with ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION.
    // Single failure surfaces as "unable to switch and save provider" to user
    // (#2601 / #2500 / #2612). Short backoff retry a few times, rethrow on final failure.
    let mut last_error = None;
    for attempt in 0..6 {
        match unsafe {
            MoveFileExW(
                PCWSTR(source.as_ptr()),
                PCWSTR(target.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("Failed to replace file")))
}