//! ClaudeDeskDown host implementation for claude-local: state directory, settings file, upstream (selected Base URL + Key), dictation service.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use claude_local::dictation::EngineError;
use claude_local::{ClaudeLocalSettings, Host, Upstream};
use serde_json::{Map, Value};

/// State directory: `CLAUDE_DESKDOWN_STATE_DIR` overrides, else `~/.claude-deskdown`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_DESKDOWN_STATE_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        return dir;
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".claude-deskdown"))
        .unwrap_or_else(|| PathBuf::from(".claude-deskdown"))
}

fn settings_path() -> PathBuf {
    state_dir().join("settings.json")
}

/// Settings file stores Claude section under `claudeDesktopLocal` (same name and shape as Codex++); other keys preserved as-is.
const SETTINGS_KEY: &str = "claudeDesktopLocal";

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

fn read_settings_map() -> Result<Map<String, Value>> {
    match std::fs::read(settings_path()) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes)
            .with_context(|| format!("Settings file corrupted: {}", settings_path().display()))?
        {
            Value::Object(map) => Ok(map),
            _ => bail!("Settings file format invalid: {}", settings_path().display()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(error).with_context(|| format!("Failed to read settings: {}", settings_path().display())),
    }
}

pub struct DeskDownHost;

impl DeskDownHost {
    pub fn install() {
        claude_local::install_host(Arc::new(DeskDownHost));
    }

    fn active_upstream(&self) -> Result<Upstream> {
        let book = crate::upstreams::load_book(&crate::upstreams::default_book_path())?;
        let profile = book
            .active()
            .context("No upstream yet: add a Base URL and Key, or import from cc-switch / Magpie / Codex++")?;
        let upstream = profile.to_upstream();
        if upstream.root.is_empty() {
            bail!("Current upstream \"{}\" has no Base URL", profile.name);
        }
        if upstream.default_model.is_empty() {
            bail!("Current upstream \"{}\" has no default model selected", profile.name);
        }
        Ok(upstream)
    }
}

impl Host for DeskDownHost {
    fn state_dir(&self) -> PathBuf {
        state_dir()
    }

    fn ca_name(&self) -> &str {
        "ClaudeDeskDown Local CA"
    }

    fn load_config(&self) -> Result<ClaudeLocalSettings> {
        let map = read_settings_map()?;
        let stored = map.get(SETTINGS_KEY).cloned();
        let mut config: ClaudeLocalSettings = stored
            .clone()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        // ClaudeDeskDown only has full-feature mode.
        config.enabled = true;
        // Session sync default on: sessions created in regular Claude / 3p mode live in ~/.claude, full-feature mode uses separate dir,
        // without sync the sidebar shows "session not found on disk". Respect user if they've disabled it.
        if stored.as_ref().and_then(|value| value.get("sessionSync")).is_none() {
            config.session_sync = true;
        }
        // Code tab defaults to sharing ~/.claude with CLI (fill actual effective value so UI displays correctly).
        if config.code_share_home.is_none() {
            config.code_share_home = Some(self.default_code_share_home());
        }
        Ok(config)
    }

    fn save_config(&self, config: &ClaudeLocalSettings) -> Result<()> {
        let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut map = read_settings_map()?;
        let mut config = config.clone();
        config.enabled = true;
        map.insert(SETTINGS_KEY.into(), serde_json::to_value(config)?);
        claude_local::fsutil::atomic_write(&settings_path(), &serde_json::to_vec_pretty(&Value::Object(map))?)
    }

    fn preview_upstream(&self, _config: &ClaudeLocalSettings) -> Result<Upstream> {
        self.active_upstream()
    }

    fn prepare_upstream(&self, _config: &ClaudeLocalSettings) -> Result<Upstream> {
        self.active_upstream()
    }

    fn default_code_share_home(&self) -> bool {
        true
    }

    fn dictation_ready(&self, config: &ClaudeLocalSettings) -> bool {
        config.voice_input && claude_local::dictation::model_ready()
    }

    fn prepare_dictation_service(&self) -> Result<()> {
        let availability: claude_local::dictation::server::Availability = Arc::new(|| {
            let config = DeskDownHost.load_config().map_err(|error| EngineError::new(500, format!("{error:#}")))?;
            if !config.voice_input {
                return Err(EngineError::new(403, "Native dictation not enabled"));
            }
            if !claude_local::dictation::model_ready() {
                return Err(EngineError::new(503, "Local SenseVoice model not ready"));
            }
            Ok(())
        });
        claude_local::dictation::server::ensure_running(&state_dir(), availability).map(|_| ())
    }

    fn diagnostic(&self, event: &str, detail: Value) {
        crate::log::append(event, detail);
    }
}