//! Claude Code session sync between 3p mode and full-feature mode:
//! Two deployment modes use different Application Support dirs, each stores session metadata at
//! `claude-code-sessions/<account>/<org>/local_<uuid>.json`; transcripts live in their respective
//! CLAUDE_CONFIG_DIR under `projects/<escaped cwd>/<cliSessionId>.jsonl`. Early 3p sessions also
//! have transcripts in `~/.claude`. `local-agent-mode-sessions` (Cowork) not synced.
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Full-feature mode mock account/org (must match tools/claude-local/proxy.mjs ACCOUNT.id / ORG.uuid).
pub const FULL_ACCOUNT: &str = "c46bd1eb-fbbf-4e5a-9bb3-5ebe8521eb54";
pub const FULL_ORG: &str = "e0e877d3-8475-44a1-a501-4fc431b085ab";

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub to_full: usize,
    pub to_three_p: usize,
    pub updated: usize,
    pub skipped_deleted: usize,
    /// Metadata exists but transcript missing on both sides.
        pub missing_transcripts: usize,
        /// Note when 3p side has no session dir yet; empty in normal cases.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

const LOCAL_PREFIX: &str = "local_";
const DELETED_PREFIX: &str = "deleted_";

fn session_id(name: &str, prefix: &str) -> Option<String> {
    name.strip_prefix(prefix)?
        .strip_suffix(".json")
        .map(str::to_string)
}

/// `claude-code-sessions/<acct>/<org>/` dir pair containing local_*.json with highest mtime.
fn newest_pair(root: &Path) -> Option<(String, String)> {
    let mut best: Option<(String, String, std::time::SystemTime)> = None;
    for acct in fs::read_dir(root).ok()?.flatten() {
        let acct_path = acct.path();
        if !acct_path.is_dir() {
            continue;
        }
        for org in fs::read_dir(&acct_path).ok()?.flatten() {
            let org_path = org.path();
            if !org_path.is_dir() {
                continue;
            }
            let newest = fs::read_dir(&org_path).ok()?.flatten().filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with(LOCAL_PREFIX)
                    .then(|| entry.metadata().ok()?.modified().ok())
                    .flatten()
            });
            for mtime in newest {
                if best.as_ref().is_none_or(|(_, _, t)| mtime > *t) {
                    best = Some((
                        acct.file_name().to_string_lossy().into_owned(),
                        org.file_name().to_string_lossy().into_owned(),
                        mtime,
                    ));
                }
            }
        }
    }
    best.map(|(a, o, _)| (a, o))
}

fn list_sessions(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut locals = Vec::new();
    let mut deleted = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = session_id(&name, LOCAL_PREFIX) {
            locals.push(id);
        } else if let Some(id) = session_id(&name, DELETED_PREFIX) {
            deleted.push(id);
        }
    }
    (locals, deleted)
}

/// Metadata freshness: lastActivityAt from JSON (numeric), falls back to file mtime.
fn freshness(path: &Path) -> f64 {
    let parsed = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if let Some(value) = parsed {
        if let Some(activity) = value.get("lastActivityAt").and_then(Value::as_f64) {
            return activity;
        }
    }
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Claude Code projects dir escaping: each non-ASCII alphanumeric char in cwd becomes `-`.
fn escape_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn backup_stamp() -> String {
    format!(
        "{:.0}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    )
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Transcript ids referenced by this session metadata: cliSessionId + priorCliSessionIds.
fn transcript_ids(meta: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(id) = meta.get("cliSessionId").and_then(Value::as_str) {
        ids.push(id.to_string());
    }
    if let Some(list) = meta.get("priorCliSessionIds").and_then(Value::as_array) {
        for id in list.iter().filter_map(Value::as_str) {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

fn find_transcript(configs: &[PathBuf], escaped: &str, id: &str) -> Option<PathBuf> {
    configs
        .iter()
        .map(|config| config.join("projects").join(escaped).join(format!("{id}.jsonl")))
        .find(|path| path.is_file())
}

/// Copy transcripts for a session from source candidates to first target config.
/// Conflict: keep larger byte count (jsonl only appends), overwritten backed up first.
fn sync_transcripts(
    meta: &Value,
    source_configs: &[PathBuf],
    target_configs: &[PathBuf],
    backup: &Path,
    report: &mut SyncReport,
) -> Result<()> {
    let Some(target) = target_configs.first() else {
        return Ok(());
    };
    let cwd = meta.get("cwd").and_then(Value::as_str).unwrap_or_default();
    if cwd.is_empty() {
        return Ok(());
    }
    let escaped = escape_cwd(cwd);
    for id in transcript_ids(meta) {
        let dst = target.join("projects").join(&escaped).join(format!("{id}.jsonl"));
        let src = find_transcript(source_configs, &escaped, &id)
            .or_else(|| find_transcript(target_configs, &escaped, &id));
        match (src, dst.is_file()) {
            (Some(src), false) => {
                fs::create_dir_all(dst.parent().unwrap())?;
                fs::copy(&src, &dst)?;
            }
            (Some(src), true) => {
                let src_bytes = fs::read(&src)?;
                let dst_bytes = fs::read(&dst)?;
                if src_bytes != dst_bytes && src_bytes.len() > dst_bytes.len() {
                    let backup_dir = backup.join(backup_stamp()).join("transcripts");
                    fs::create_dir_all(&backup_dir)?;
                    fs::copy(&dst, backup_dir.join(format!("{id}.jsonl")))?;
                    crate::fsutil::atomic_write(&dst, &src_bytes)?;
                }
            }
            (None, exists) if !exists => {
                report.missing_transcripts += 1;
                let _ = crate::host::diagnostic(
                    "claude_session_sync.missing_transcript",
                    serde_json::json!({"cliSessionId": id, "cwd": cwd}),
                );
            }
            _ => {}
        }
        // Same-name subdirs (sub-agent outputs etc.) follow along.
        for config in source_configs {
            let dir = config.join("projects").join(&escaped).join(&id);
            if dir.is_dir() {
                let out = target.join("projects").join(&escaped).join(&id);
                if !out.exists() {
                    copy_dir(&dir, &out)?;
                }
            }
        }
    }
    Ok(())
}

/// One-way merge mine -> theirs: copy missing sessions, overwrite by freshness, copy transcripts.
fn merge_dir(
    mine: &Path,
    mine_configs: &[PathBuf],
    theirs: &Path,
    their_configs: &[PathBuf],
    tombstones: &HashSet<String>,
    backup: &Path,
    report: &mut SyncReport,
    direction: &str,
) -> Result<()> {
    let (mine_locals, _) = list_sessions(mine);
    let (their_locals, _) = list_sessions(theirs);
    for id in &mine_locals {
        if tombstones.contains(id) {
            continue;
        }
        let src = mine.join(format!("{LOCAL_PREFIX}{id}.json"));
        let dst = theirs.join(format!("{LOCAL_PREFIX}{id}.json"));
        if !their_locals.contains(id) {
            fs::create_dir_all(theirs)?;
            fs::copy(&src, &dst)?;
            if direction == "full" {
                report.to_full += 1;
            } else {
                report.to_three_p += 1;
            }
        } else {
            let src_bytes = fs::read(&src)?;
            let dst_bytes = fs::read(&dst)?;
            if src_bytes != dst_bytes {
                if serde_json::from_slice::<Value>(&src_bytes).is_err()
                    || serde_json::from_slice::<Value>(&dst_bytes).is_err()
                {
                    let _ = crate::host::diagnostic(
                        "claude_session_sync.bad_json",
                        serde_json::json!({"file": src.file_name().unwrap_or_default().to_string_lossy()}),
                    );
                    continue;
                }
                if freshness(&src) > freshness(&dst) {
                    let backup_dir = backup.join(backup_stamp()).join(direction);
                    fs::create_dir_all(&backup_dir)?;
                    fs::copy(&dst, backup_dir.join(format!("{LOCAL_PREFIX}{id}.json")))?;
                    crate::fsutil::atomic_write(&dst, &src_bytes)?;
                    report.updated += 1;
                }
            }
        }
        // Fill transcript after metadata ready (includes previously synced ones that weren't copied at the time).
        if let Ok(meta) = serde_json::from_slice::<Value>(&fs::read(&dst).unwrap_or_default()) {
            sync_transcripts(&meta, mine_configs, their_configs, backup, report)?;
        }
    }
    Ok(())
}

/// sessions root = `.../claude-code-sessions`; config candidates by priority, write target = first of each list.
pub fn sync_sessions(
    three_p_root: &Path,
    three_p_configs: &[PathBuf],
    full_root: &Path,
    full_configs: &[PathBuf],
    backup_root: &Path,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let Some((acct, org)) = newest_pair(three_p_root) else {
        report.note = "3p mode has no sessions yet".into();
        return Ok(report);
    };
    let three_p = three_p_root.join(&acct).join(&org);
    let full = full_root.join(FULL_ACCOUNT).join(FULL_ORG);
    fs::create_dir_all(&full)?;
    // Tombstones from either side block copy; same session counted once.
    let (_, tp_deleted) = list_sessions(&three_p);
    let (_, f_deleted) = list_sessions(&full);
    let tombstones: HashSet<String> = tp_deleted.into_iter().chain(f_deleted).collect();
    let (tp_locals, _) = list_sessions(&three_p);
    let (f_locals, _) = list_sessions(&full);
    report.skipped_deleted = tp_locals
        .iter()
        .chain(f_locals.iter())
        .filter(|id| tombstones.contains(*id))
        .collect::<HashSet<_>>()
        .len();
    merge_dir(
        &three_p,
        three_p_configs,
        &full,
        full_configs,
        &tombstones,
        backup_root,
        &mut report,
        "full",
    )?;
    merge_dir(
        &full,
        full_configs,
        &three_p,
        three_p_configs,
        &tombstones,
        backup_root,
        &mut report,
        "three-p",
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(
        root: &Path,
        acct: &str,
        org: &str,
        id: &str,
        activity: f64,
        extra: &str,
    ) {
        let dir = root.join(acct).join(org);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("local_{id}.json")),
            format!(
                r#"{{"sessionId":"{id}","cliSessionId":"cli-{id}","cwd":"/Users/mac/Desktop/reclaude2api","lastActivityAt":{activity}{extra}}}"#
            ),
        )
        .unwrap();
    }

    fn transcript(config: &Path, id: &str, lines: usize) -> PathBuf {
        let file = config
            .join("projects/-Users-mac-Desktop-reclaude2api")
            .join(format!("cli-{id}.jsonl"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "x\n".repeat(lines)).unwrap();
        file
    }

    fn transcript_exists(config: &Path, id: &str) -> bool {
        config
            .join("projects/-Users-mac-Desktop-reclaude2api")
            .join(format!("cli-{id}.jsonl"))
            .is_file()
    }

    fn copy_dir_prep(config: &Path, id: &str) {
        let dir = config
            .join("projects/-Users-mac-Desktop-reclaude2api")
            .join(format!("cli-{id}/sub"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("out.txt"), "o").unwrap();
    }

    #[test]
    fn escape_matches_claude_code_project_dirs() {
        assert_eq!(escape_cwd("/Users/mac/Desktop/reclaude2api"), "-Users-mac-Desktop-reclaude2api");
        assert_eq!(escape_cwd("/Users/mac/.tmp_cc"), "-Users-mac--tmp-cc");
    }

    #[test]
    fn sessions_and_transcripts_flow_both_ways() {
        let temp = tempfile::tempdir().unwrap();
        let t3 = temp.path().join("3p/sessions");
        let tf = temp.path().join("full/sessions");
        let c3 = vec![temp.path().join("3p/claude-code-3p"), temp.path().join("claude")];
        let cf = vec![temp.path().join("full/claude-code"), temp.path().join("claude")];
        let backup = temp.path().join("backup");
        seed(&t3, "a3p", "o3p", "s1", 10.0, "");
        /// Transcript only in ~/.claude candidate.
        transcript(&c3[1], "s1", 3);
        seed(&tf, FULL_ACCOUNT, FULL_ORG, "s2", 20.0, "");
        transcript(&cf[0], "s2", 2);
        let report = sync_sessions(&t3, &c3, &tf, &cf, &backup).unwrap();
        assert_eq!((report.to_full, report.to_three_p), (1, 1));
        assert!(transcript_exists(&cf[0], "s1"));
        assert!(transcript_exists(&c3[0], "s2"));
        assert_eq!(report.missing_transcripts, 0);
    }

    #[test]
    fn prior_ids_subdirs_conflicts_and_backfill() {
        let temp = tempfile::tempdir().unwrap();
        let t3 = temp.path().join("3p/sessions");
        let tf = temp.path().join("full/sessions");
        let c3 = vec![temp.path().join("3p/claude-code-3p")];
        let cf = vec![temp.path().join("full/claude-code")];
        let backup = temp.path().join("backup");
        // priorCliSessionIds transcripts also copied.
        seed(&t3, "a3p", "o3p", "s1", 30.0, r#","priorCliSessionIds":["cli-p1"]"#);
        transcript(&c3[0], "s1", 5);
        transcript(&c3[0], "p1", 1);
        // Subdirs copied wholesale.
        copy_dir_prep(&c3[0], "s1");
        // Conflict keeps larger + backup.
        transcript(&cf[0], "s1", 2);
        let dst = cf[0].join("projects/-Users-mac-Desktop-reclaude2api/cli-s1.jsonl");
        // Session with metadata synced but missing transcript gets backfilled.
        seed(&t3, "a3p", "o3p", "s9", 1.0, "");
        seed(&tf, FULL_ACCOUNT, FULL_ORG, "s9", 1.0, "");
        transcript(&c3[0], "s9", 1);
        let _report = sync_sessions(&t3, &c3, &tf, &cf, &backup).unwrap();
        assert!(transcript_exists(&cf[0], "p1"));
        assert!(cf[0].join("projects/-Users-mac-Desktop-reclaude2api/cli-s1/sub").is_dir());
        assert!(transcript_exists(&cf[0], "s9"), "Existing metadata missing transcript must be backfilled");
                assert!(fs::read_to_string(&dst).unwrap().lines().count() == 5, "Larger file wins");
                let stamped: Vec<_> = fs::read_dir(&backup).unwrap().flatten().collect();
                assert!(stamped.iter().any(|e| e.path().join("transcripts/cli-s1.jsonl").is_file()), "Overwritten goes to backup");
                // Missing source counted as missing_transcripts.
        seed(&t3, "a3p", "o3p", "gone", 1.0, "");
        let report = sync_sessions(&t3, &c3, &tf, &cf, &backup).unwrap();
        assert!(report.missing_transcripts >= 1);
    }

    #[test]
    fn tombstones_dedupe_and_other_files_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let t3 = temp.path().join("3p/sessions");
        let tf = temp.path().join("full/sessions");
        let c3 = vec![temp.path().join("3p/claude-code-3p")];
        let cf = vec![temp.path().join("full/claude-code")];
        let dir3 = t3.join("a").join("o");
        fs::create_dir_all(&dir3).unwrap();
        fs::write(dir3.join("deleted_s9.json"), "{}").unwrap();
        fs::write(dir3.join("local_s9.json"), "{}").unwrap();
        fs::write(dir3.join("scheduled-tasks.json"), "[]").unwrap();
        let report = sync_sessions(&t3, &c3, &tf, &cf, &temp.path().join("backup")).unwrap();
        assert_eq!(report.skipped_deleted, 1, "Tombstones deduped by id");
                assert!(!tf.join(FULL_ACCOUNT).join(FULL_ORG).join("local_s9.json").exists());
                assert!(dir3.join("scheduled-tasks.json").exists());
            }

            #[test]
            fn empty_three_p_reports_note_instead_of_error() {
                let temp = tempfile::tempdir().unwrap();
                let report = sync_sessions(
                    &temp.path().join("nope"),
                    &[],
                    &temp.path().join("full"),
                    &[],
                    &temp.path().join("backup"),
                )
                .unwrap();
                assert_eq!(report.note, "3p mode has no sessions yet");
            }

            /// Hardcoded mock account/org must match literal in proxy.mjs.
            #[test]
            fn full_mode_ids_match_proxy_mock() {
                let proxy = include_str!("../runtime/proxy.mjs");
                assert!(proxy.contains(FULL_ACCOUNT), "proxy.mjs ACCOUNT.id changed");
                assert!(proxy.contains(FULL_ORG), "proxy.mjs ORG.uuid changed");
            }
}
