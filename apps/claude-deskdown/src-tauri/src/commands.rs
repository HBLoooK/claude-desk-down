//! ClaudeDeskDown commands: Claude status / launch / settings (forwarded to claude-local), upstream management & import, dictation model.

use std::sync::Mutex;

use crate::upstreams::{self, UpstreamBook, UpstreamModel, UpstreamProfile};
use serde::Serialize;

use serde_json::{Value, json};

/// Commands return `{status, message, ...payload}` (payload flattened to top level, frontend reads this shape).
#[derive(Debug, Serialize)]
pub struct CommandResult<T: Serialize> {
    pub status: String,
    pub message: String,
    #[serde(flatten)]
    pub payload: T,
}

pub fn ok<T: Serialize>(message: &str, payload: T) -> CommandResult<T> {
    CommandResult { status: "ok".into(), message: message.into(), payload }
}

pub fn failed<T: Serialize>(message: &str, payload: T) -> CommandResult<T> {
    CommandResult { status: "failed".into(), message: message.into(), payload }
}

/// Serialize read-modify-write of upstream list to avoid concurrent saves overwriting each other.
static BOOK_LOCK: Mutex<()> = Mutex::new(());

/// Payload must be a struct: `CommandResult` flattens it to top level, arrays can't be flattened.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookPayload {
    profiles: Vec<UpstreamProfile>,
    active_id: Option<String>,
}

impl From<UpstreamBook> for BookPayload {
    fn from(book: UpstreamBook) -> Self {
        Self { profiles: book.profiles, active_id: book.active_id }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ModelsPayload {
    models: Vec<UpstreamModel>,
}

fn load() -> anyhow::Result<UpstreamBook> {
    upstreams::load_book(&upstreams::default_book_path())
}

/// Modify upstream list, save to disk, then pass selected upstream to Claude local service.
fn mutate<T>(change: impl FnOnce(&mut UpstreamBook) -> anyhow::Result<T>) -> anyhow::Result<(UpstreamBook, T)> {
    let _guard = BOOK_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut book = load()?;
    let value = change(&mut book)?;
    upstreams::save_book(&upstreams::default_book_path(), &book)?;
    Ok((book, value))
}

fn respond<T>(result: anyhow::Result<(UpstreamBook, T)>, message: impl FnOnce(T) -> String, failure: &str) -> CommandResult<BookPayload> {
    match result {
        Ok((book, value)) => ok(&message(value), book.into()),
        Err(error) => failed(&format!("{failure}: {error:#}"), load().unwrap_or_default().into()),
    }
}


#[tauri::command]
pub fn deskdown_load() -> CommandResult<BookPayload> {
    match load() {
        Ok(book) => ok("Upstreams loaded.", book.into()),
        Err(error) => failed(&format!("{error:#}"), BookPayload::default()),
    }
}

/// Add new (id empty) or update existing; if `activate` true, also set as current.
#[tauri::command]
pub fn deskdown_save_profile(profile: UpstreamProfile, activate: Option<bool>) -> CommandResult<BookPayload> {
    let result = mutate(|book| {
        let id = book.upsert(profile);
        if activate.unwrap_or(false) {
            book.active_id = Some(id.clone());
        }
        Ok(book.profiles.iter().find(|item| item.id == id).map(|item| item.name.clone()).unwrap_or_default())
    });
    respond(result, |name| format!("Saved \"{name}\"."), "Failed to save upstream")
}

#[tauri::command]
pub fn deskdown_delete_profile(id: String) -> CommandResult<BookPayload> {
    let result = mutate(|book| {
        let name = book.profiles.iter().find(|item| item.id == id).map(|item| item.name.clone()).unwrap_or_default();
        book.remove(&id);
        Ok(name)
    });
    respond(result, |name| format!("Deleted \"{name}\"."), "Failed to delete upstream")
}

#[tauri::command]
pub fn deskdown_set_active(id: String) -> CommandResult<BookPayload> {
    let result = mutate(|book| {
        let name = book
            .profiles
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.clone())
            .ok_or_else(|| anyhow::anyhow!("Upstream no longer exists"))?;
        book.active_id = Some(id.clone());
        Ok(name)
    });
    respond(result, |name| format!("Switched to \"{name}\", effective after restarting Claude."), "Failed to switch upstream")
}

/// Import from other tools: `ccswitch` / `magpie` / `codexpp`. Gateway sources (Magpie, Codex++) fetch models if missing.
#[tauri::command]
pub async fn deskdown_import(source: String) -> CommandResult<BookPayload> {
    let imported = match source.as_str() {
        upstreams::SOURCE_CC_SWITCH => upstreams::cc_switch_profiles(&upstreams::default_cc_switch_db()),
        upstreams::SOURCE_MAGPIE => Ok(vec![upstreams::magpie_profile(
            &upstreams::default_magpie_dir(),
            std::env::var("MAGPIE_ADDR").ok().as_deref(),
        )]),
        upstreams::SOURCE_CODEX_PLUS_PLUS => {
            upstreams::codex_plus_plus_profile(&upstreams::codex_plus_plus_state_dir()).map(|profile| vec![profile])
        }
        other => Err(anyhow::anyhow!("Unknown import source: {other}")),
    };
    let mut imported = match imported {
        Ok(imported) => imported,
        Err(error) => return failed(&format!("{error:#}"), load().unwrap_or_default().into()),
    };
    if imported.is_empty() {
        return failed("No importable Claude providers found (only Anthropic-compatible imported).", load().unwrap_or_default().into());
    }
    let mut unreachable = Vec::new();
    for profile in imported.iter_mut().filter(|profile| profile.models.is_empty()) {
        match upstreams::fetch_models(&profile.base_url, &profile.api_key).await {
            Ok(models) => {
                profile.models = models.iter().map(|model| model.id.clone()).collect();
                profile.model_names = models
                    .iter()
                    .filter_map(|model| Some((model.id.clone(), model.display_name.clone()?)))
                    .collect();
                if profile.model.is_empty() {
                    profile.model = profile.models.first().cloned().unwrap_or_default();
                }
                profile.context_window = models
                    .iter()
                    .find(|model| model.id == profile.model)
                    .and_then(|model| model.context_window);
            }
            Err(_) => unreachable.push(profile.name.clone()),
        }
    }
    let result = tauri::async_runtime::spawn_blocking(move || mutate(|book| Ok(book.merge_imported(imported))))
        .await
        .unwrap_or_else(|error| Err(anyhow::anyhow!("{error}")));
    respond(
        result,
        |(added, updated)| {
            let mut message = format!("Imported: {added} new, {updated} updated.");
            if !unreachable.is_empty() {
                message.push_str(&format!(" {} temporarily unreachable, model list fetched later.", unreachable.join(", ")));
            }
            message
        },
        "Import failed",
    )
}

#[tauri::command]
pub async fn deskdown_fetch_models(base_url: String, api_key: String) -> CommandResult<ModelsPayload> {
    match upstreams::fetch_models(&base_url, &api_key).await {
        Ok(models) => ok(&format!("Fetched {} models.", models.len()), ModelsPayload { models }),
        Err(error) => failed(&format!("{error:#}"), ModelsPayload::default()),
    }
}

/// Native dictation toggle: only changes Claude's "native dictation", transcription uses local SenseVoice. Returns latest Claude status.
#[tauri::command]
pub async fn deskdown_set_voice(enabled: bool) -> CommandResult<Value> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut config = claude_local::status()?.config;
        config.voice_input = enabled;
        claude_local::configure(config)
    })
    .await
    .unwrap_or_else(|error| Err(anyhow::anyhow!("{error}")));
    match result {
        Ok(status) => ok(
            if enabled { "Native dictation enabled. Restart Claude to see microphone in input box." } else { "Native dictation disabled." },
            serde_json::to_value(status).unwrap_or_default(),
        ),
        Err(error) => failed(&format!("{error:#}"), json!({})),
    }
}

// ── Claude itself: status, settings, actions (forwarded to claude-local) ─────────────────────────

#[tauri::command]
pub async fn claude_local_status() -> CommandResult<Value> {
    match tauri::async_runtime::spawn_blocking(claude_local::status).await {
        Ok(Ok(status)) => ok("Claude status loaded.", serde_json::to_value(status).unwrap_or_default()),
        Ok(Err(error)) => failed(&format!("{error:#}"), json!({})),
        Err(error) => failed(&format!("Failed to read Claude status: {error}"), json!({})),
    }
}

#[tauri::command]
pub async fn claude_local_configure(config: claude_local::ClaudeLocalSettings) -> CommandResult<Value> {
    match tauri::async_runtime::spawn_blocking(move || claude_local::configure(config)).await {
        Ok(Ok(status)) => ok("Settings saved.", serde_json::to_value(status).unwrap_or_default()),
        Ok(Err(error)) => failed(&format!("{error:#}"), json!({})),
        Err(error) => failed(&format!("Failed to save settings: {error}"), json!({})),
    }
}

#[tauri::command]
pub async fn claude_local_action(action: String) -> CommandResult<Value> {
    match tauri::async_runtime::spawn_blocking(move || claude_local::perform(&action)).await {
        Ok(Ok(result)) => ok(&result.message, serde_json::to_value(result.status).unwrap_or_default()),
        Ok(Err(error)) => {
            let status = claude_local::status().ok().and_then(|status| serde_json::to_value(status).ok());
            failed(&format!("{error:#}"), status.unwrap_or_else(|| json!({})))
        }
        Err(error) => failed(&format!("Action failed: {error}"), json!({})),
    }
}

// ── Local dictation model ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn dictation_local_model_status() -> Value {
    claude_local::dictation::model_status()
}

#[tauri::command]
pub async fn download_dictation_local_model() -> CommandResult<Value> {
    match claude_local::dictation::prepare_model().await {
        Ok(state) => ok("Local SenseVoice model downloaded and verified.", state),
        Err(error) => failed(&error.message, json!({})),
    }
}