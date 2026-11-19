//! Register CLI-created (terminal, BaoCode, etc.) Claude Code sessions in Desktop's session list.
//!
//! Desktop sidebar only lists sessions it knows about (`claude-code-sessions/<account>/<org>/local_*.json`),
//! while CLI session transcripts live in `~/.claude/projects` but don't appear in the list.
//! This runs before Claude launch to backfill: record format matches Desktop's own "Import CLI Sessions"
//! (`local_<cliSessionId>`, `fromCliTranscript`).
//!
//! Not imported: sessions Desktop already knows (session id appears in any record), sessions deleted
//! in Desktop, empty shell sessions with no real conversation. Only called when Claude isn't running
//! (Desktop rewrites session records while running).

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde_json::{Value, json};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped_empty: usize,
}

/// `code_dir` is the Code tab's actual config dir (shared = `~/.claude`), `sessions_dir` is the
/// full-feature mode session list directory (`<profile>/claude-code-sessions/<account>/<org>`).
pub fn import_cli_sessions(code_dir: &Path, sessions_dir: &Path) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let projects = code_dir.join("projects");
    if !projects.is_dir() {
        return Ok(report);
    }
    let known = known_session_ids(sessions_dir);
    let mut pending = Vec::new();
    for project in fs::read_dir(&projects)?.flatten() {
        if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let names: Vec<String> = fs::read_dir(project.path())?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        for name in &names {
            let Some(id) = name.strip_suffix(".jsonl") else { continue };
            if !is_uuid(id) || known.contains(id) || deleted_in_desktop(id, &names) {
                continue;
            }
            pending.push((id.to_string(), project.path().join(name)));
        }
    }
    if pending.is_empty() {
        return Ok(report);
    }
    fs::create_dir_all(sessions_dir)?;
    for (id, path) in pending {
        let Some(summary) = summarize(&path)? else {
            report.skipped_empty += 1;
            continue;
        };
        let record = record_for(&id, &summary, now_ms());
        crate::fsutil::atomic_write(
            &sessions_dir.join(format!("local_{id}.json")),
            &serde_json::to_vec_pretty(&record)?,
        )?;
        report.imported += 1;
    }
    Ok(report)
}

/// All session ids already known to Desktop (including continuation/fork leftovers), avoids duplicate imports.
fn known_session_ids(sessions_dir: &Path) -> HashSet<String> {
    let mut known = HashSet::new();
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return known;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("local_") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(entry.path()) {
            known.extend(uuids_in(&text));
        }
    }
    known
}

/// When Desktop deletes a session, it renames the record to `<id>.jsonl.desktop-deleted-*`
/// or leaves a `<id>.desktop-released.json`.
fn deleted_in_desktop(id: &str, names: &[String]) -> bool {
    names.iter().any(|name| {
        name == &format!("{id}.desktop-released.json")
            || name.starts_with(&format!("{id}.jsonl.desktop-deleted-"))
    })
}

#[derive(Debug, Default, PartialEq)]
struct Summary {
    cwd: String,
    title: String,
    model: Option<String>,
    permission_mode: Option<String>,
    created_ms: Option<i64>,
    modified_ms: i64,
}

/// Read transcript once: extract working dir, title (manual rename > auto-title > first prompt),
/// model, permission mode, start/end times. Returns None if no real Q&A found.
/// Only parses JSON for lines containing relevant fields, fast even for large files.
fn summarize(path: &Path) -> Result<Option<Summary>> {
    let modified_ms = fs::metadata(path)?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_else(now_ms);
    let mut summary = Summary { modified_ms, ..Summary::default() };
    let (mut custom_title, mut ai_title, mut first_prompt, mut fallback_prompt) = (None, None, None, None);
    let mut has_assistant = false;
    for line in BufReader::new(fs::File::open(path)?).lines() {
        let Ok(line) = line else { break };
        let kind = if line.contains("\"type\":\"user\"") {
            "user"
        } else if line.contains("\"type\":\"assistant\"") {
            "assistant"
        } else if line.contains("\"type\":\"custom-title\"") {
            "custom-title"
        } else if line.contains("\"type\":\"ai-title\"") {
            "ai-title"
        } else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        match kind {
            "user" => {
                if value["isSidechain"].as_bool() == Some(true) || value["isMeta"].as_bool() == Some(true) {
                    continue;
                }
                if summary.cwd.is_empty()
                    && let Some(cwd) = value["cwd"].as_str()
                {
                    summary.cwd = cwd.to_string();
                }
                if summary.created_ms.is_none() {
                    summary.created_ms = value["timestamp"].as_str().and_then(parse_timestamp_ms);
                }
                if let Some(mode) = value["permissionMode"].as_str() {
                    summary.permission_mode = Some(mode.to_string());
                }
                if first_prompt.is_none() {
                    first_prompt = prompt_text(&value["message"]["content"]);
                }
                if first_prompt.is_none() && fallback_prompt.is_none() {
                    fallback_prompt = placeholder_text(&value["message"]["content"]);
                }
            }
            "assistant" => {
                has_assistant = true;
                if let Some(model) = value["message"]["model"].as_str().filter(|model| !model.starts_with('<')) {
                    summary.model = Some(model.to_string());
                }
            }
            "custom-title" => custom_title = value["customTitle"].as_str().map(str::to_string),
            "ai-title" => ai_title = value["aiTitle"].as_str().map(str::to_string),
            _ => {}
        }
    }
    let first_prompt = first_prompt.or(fallback_prompt);
    if !has_assistant || summary.cwd.is_empty() || first_prompt.is_none() {
        return Ok(None);
    }
    summary.title = [custom_title, ai_title, first_prompt]
        .into_iter()
        .flatten()
        .map(|title| title.trim().to_string())
        .find(|title| !title.is_empty())
        .unwrap_or_else(|| "CLI Session".into());
    Ok(Some(summary))
}

fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter(|part| part["type"] == "text")
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

/// Line is purely placeholder: `[Image #1]`, `[Request interrupted by user]` style full-line
/// brackets, or `<command-name>` style wrappers.
fn is_placeholder(line: &str) -> bool {
    line.starts_with('<') || (line.starts_with('[') && line.ends_with(']'))
}

/// User's message this turn: first non-placeholder line, leading `[Image #1]` etc. stripped (truncated to 80 chars).
fn prompt_text(content: &Value) -> Option<String> {
    let text = content_text(content)?;
    let line = text
        .lines()
        .map(|line| strip_leading_placeholders(line.trim()))
        .find(|line| !line.is_empty() && !is_placeholder(line))?;
    Some(line.chars().take(80).collect())
}

/// Strip consecutive `[…]` placeholders from line start (`[Image #1] [Image #2] look at this` → `look at this`).
fn strip_leading_placeholders(mut line: &str) -> &str {
    while let Some(rest) = line.strip_prefix('[') {
        let Some(end) = rest.find(']') else { break };
        line = rest[end + 1..].trim_start();
    }
    line
}

/// Entire content is placeholders (single pasted image) -> fallback to placeholder itself as title,
/// but wrapper tags (`<…>`) don't count.
fn placeholder_text(content: &Value) -> Option<String> {
    let text = content_text(content)?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    (!line.starts_with('<')).then(|| line.chars().take(80).collect())
}

/// Record shape matches Desktop's own "Import CLI Sessions" output (Desktop fills remaining defaults on load).
fn record_for(id: &str, summary: &Summary, now: i64) -> Value {
    let mut record = json!({
        "sessionId": format!("local_{id}"),
        "cliSessionId": id,
        "cwd": summary.cwd,
        "originCwd": summary.cwd,
        "createdAt": summary.created_ms.unwrap_or(summary.modified_ms),
        "lastActivityAt": summary.modified_ms,
        "indexedAt": now,
        "isArchived": false,
        "title": summary.title,
        "titleSource": "auto",
        "permissionMode": summary.permission_mode.as_deref().unwrap_or("default"),
        "adoptedFromOtherSurface": true,
        "fromCliTranscript": true,
        "sessionPermissionUpdates": [],
        "alwaysAllowedReasons": [],
    });
    if let Some(model) = &summary.model {
        record["model"] = json!(model);
    }
    record
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// `2026-11-09T12:34:56.789Z` → millisecond timestamp (only accepts UTC `Z` suffix, records all use this).
fn parse_timestamp_ms(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.split('-').map(|part| part.parse::<i32>().ok());
    let (year, month, day) = (date_parts.next()??, date_parts.next()??, date_parts.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock_parts = clock.split(':').map(|part| part.parse::<u8>().ok());
    let (hour, minute, second) = (clock_parts.next()??, clock_parts.next()??, clock_parts.next()??);
    let millis: i64 = format!("{:0<3}", &fraction[..fraction.len().min(3)]).parse().ok()?;
    let date = time::Date::from_calendar_date(year, time::Month::try_from(month as u8).ok()?, day as u8).ok()?;
    let clock = time::Time::from_hms(hour, minute, second).ok()?;
    let seconds = time::PrimitiveDateTime::new(date, clock).assume_utc().unix_timestamp();
    Some(seconds * 1000 + millis)
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(index, ch)| {
            if [8, 13, 18, 23].contains(&index) { ch == '-' } else { ch.is_ascii_hexdigit() }
        })
}

fn uuids_in(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index + 36 <= bytes.len() {
        if let Some(candidate) = text.get(index..index + 36)
            && is_uuid(candidate)
        {
            found.push(candidate.to_ascii_lowercase());
            index += 36;
        } else {
            index += 1;
        }
    }
    found
}

/// Full-feature mode session list directory (main profile's `claude-code-sessions/<mock account>/<mock org>`).
pub fn full_mode_sessions_dir(profile_data_dir: &Path) -> PathBuf {
    profile_data_dir
        .join("claude-code-sessions")
        .join(crate::session_sync::FULL_ACCOUNT)
        .join(crate::session_sync::FULL_ORG)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLI: &str = "11111111-2222-4333-8444-555555555555";
    const KNOWN: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const EMPTY: &str = "99999999-2222-4333-8444-555555555555";
    const DELETED: &str = "77777777-2222-4333-8444-555555555555";

    fn transcript(title_line: &str) -> String {
        [
            r#"{"type":"last-prompt","lastPrompt":"x"}"#,
            r#"{"type":"user","isMeta":true,"cwd":"/meta","message":{"role":"user","content":"<local-command-caveat>…</local-command-caveat>"}}"#,
            r#"{"type":"user","cwd":"/work/app","permissionMode":"acceptEdits","timestamp":"2026-11-09T12:34:56.789Z","message":{"role":"user","content":[{"type":"text","text":"Help me fix the login page\nCheck the styles too"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[]}}"#,
            title_line,
        ]
        .join("\n")
    }

    #[test]
    fn imports_only_cli_sessions_desktop_does_not_know() {
        let temp = tempfile::tempdir().unwrap();
        let code = temp.path().join(".claude");
        let project = code.join("projects/-work-app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(format!("{CLI}.jsonl")), transcript(r#"{"type":"ai-title","aiTitle":"Login page fix"}"#)).unwrap();
        fs::write(project.join(format!("{KNOWN}.jsonl")), transcript("")).unwrap();
        fs::write(project.join(format!("{EMPTY}.jsonl")), r#"{"type":"custom-title","customTitle":"hi"}"#).unwrap();
        fs::write(project.join(format!("{DELETED}.jsonl")), transcript("")).unwrap();
        fs::write(project.join(format!("{DELETED}.desktop-released.json")), r#"{"reason":"delete"}"#).unwrap();
        let sessions = temp.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        // Desktop already has a record mentioning KNOWN (e.g. as previous id in a continuation).
        fs::write(sessions.join("local_x.json"), format!(r#"{{\"cliSessionId\":\"other\",\"previous\":[\"{KNOWN}\"]}}"#)).unwrap();

        let report = import_cli_sessions(&code, &sessions).unwrap();
        assert_eq!(report, ImportReport { imported: 1, skipped_empty: 1 });
        let record: Value =
            serde_json::from_slice(&fs::read(sessions.join(format!("local_{CLI}.json"))).unwrap()).unwrap();
        assert_eq!(record["sessionId"], format!("local_{CLI}"));
        assert_eq!(record["cliSessionId"], CLI);
        assert_eq!(record["cwd"], "/work/app", "skips isMeta line, picks real working dir");
        assert_eq!(record["title"], "Login page fix");
        assert_eq!(record["model"], "claude-opus-5-5");
        assert_eq!(record["permissionMode"], "acceptEdits");
        assert_eq!(record["createdAt"], 1_794_227_696_789_i64);
        assert_eq!(record["fromCliTranscript"], true);
        assert!(!sessions.join(format!("local_{KNOWN}.json")).exists());
        assert!(!sessions.join(format!("local_{DELETED}.json")).exists(), "Desktop-deleted sessions not re-imported");

        // Run again: already imported sessions not duplicated.
        assert_eq!(import_cli_sessions(&code, &sessions).unwrap(), ImportReport { imported: 0, skipped_empty: 1 });
    }

    #[test]
    fn title_prefers_custom_then_ai_then_first_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        fs::write(&path, transcript(r#"{"type":"custom-title","customTitle":"My custom name"}"#)).unwrap();
        assert_eq!(summarize(&path).unwrap().unwrap().title, "My custom name");
        fs::write(&path, transcript("")).unwrap();
        assert_eq!(summarize(&path).unwrap().unwrap().title, "Help me fix the login page");
        // Leading placeholders (image, interruption) skipped; only placeholders -> use placeholder itself.
        assert_eq!(prompt_text(&json!("[Image #1] \nWhy is this button crooked")).as_deref(), Some("Why is this button crooked"));
        assert_eq!(prompt_text(&json!("[Image #1]")), None);
        assert_eq!(prompt_text(&json!("[Image #1] [Image #2] Where do these go")).as_deref(), Some("Where do these go"));
        assert_eq!(placeholder_text(&json!("[Image #1]")).as_deref(), Some("[Image #1]"));
        assert_eq!(placeholder_text(&json!("<command-name>/model</command-name>")), None);
    }

    #[test]
    fn timestamps_and_ids_parse_strictly() {
        assert_eq!(parse_timestamp_ms("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(parse_timestamp_ms("2026-11-09T12:34:56Z"), Some(1_794_227_696_000));
        assert_eq!(parse_timestamp_ms("not a time"), None);
        assert!(is_uuid(CLI));
        assert!(!is_uuid("agent-1234"));
        assert_eq!(uuids_in(&format!("x{CLI}y{KNOWN}")), vec![CLI.to_string(), KNOWN.to_string()]);
    }
}