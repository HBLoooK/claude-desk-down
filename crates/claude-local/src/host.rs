//! Host interface: embedding programs (ClaudeDeskDown, Codex++) each implement it; this crate uses it to get state dir, settings, and upstream.
//!
//! This crate only handles "connecting original Claude Desktop to local compat service": resource installation, certificates, proxy process,
//! real profile switching, launch/stop. Where upstream comes from (ClaudeDeskDown direct Base URL + Key, Codex++ via its own gateway),
//! where settings live, how dictation service starts — all decided by host.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::settings::ClaudeLocalSettings;

/// Upstream given to local compat service and built-in Claude Code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Upstream {
    /// Anthropic-compatible root URL: requests sent to `{root}/v1/messages` (same semantics as `ANTHROPIC_BASE_URL`).
    pub root: String,
    pub token: String,
    pub default_model: String,
    /// Haiku tier (title generation etc.) model.
    pub small_model: String,
    /// Models listed to Claude model selector.
    pub models: Vec<String>,
    /// Models with smaller window than default, not listed to Claude (for status display only).
    pub hidden_models: Vec<String>,
    /// Model display names: `{"name": "...", "description": "..."}`.
    pub model_names: BTreeMap<String, Value>,
    /// Per-model context windows.
    pub model_windows: BTreeMap<String, u64>,
    /// Default model's context window; `None` means undeclared, Claude treats as 200k.
    pub default_window: Option<u64>,
    /// Extra env vars passed when launching Claude (e.g., Codex++'s `CLAUDE_CODE_MODEL_CAPABILITIES`).
    pub extra_env: Vec<(String, String)>,
}

impl Upstream {
    pub fn ready(&self) -> bool {
        !self.root.trim().is_empty() && !self.default_model.trim().is_empty()
    }
}

pub trait Host: Send + Sync {
    /// Host's state directory: local service files in `<state>/claude-local`, dictation creds etc. also here.
    fn state_dir(&self) -> PathBuf;

    /// Cert name in system keychain/cert store, each host uses its own, no overwrite.
    fn ca_name(&self) -> &str;

    fn load_config(&self) -> Result<ClaudeLocalSettings>;
    fn save_config(&self, config: &ClaudeLocalSettings) -> Result<()>;

    /// Upstream for status display: read-only, cannot start any service. Returns error when not configured yet, error shown in UI.
    fn preview_upstream(&self, config: &ClaudeLocalSettings) -> Result<Upstream>;

    /// Pre-launch prepare and return actual upstream to use (Codex++ starts gateway, writes bindings here).
    fn prepare_upstream(&self, config: &ClaudeLocalSettings) -> Result<Upstream>;

    /// Upstream state participating in config fingerprint: change triggers local service restart. Defaults to preview upstream (empty on failure).
    fn upstream_fingerprint(&self, config: &ClaudeLocalSettings) -> Value {
        self.preview_upstream(config)
            .map(|upstream| {
                serde_json::json!({
                    "root": upstream.root,
                    "token": sha_hex(&upstream.token),
                    "model": upstream.default_model,
                    "small": upstream.small_model,
                    "models": upstream.models,
                    "windows": upstream.model_windows,
                    "window": upstream.default_window,
                })
            })
            .unwrap_or(Value::Null)
    }

    /// When settings doesn't specify, whether Code tab shares `~/.claude` with command line (see [`ClaudeLocalSettings::code_share_home`]).
    fn default_code_share_home(&self) -> bool {
        false
    }

    /// Local dictation (SenseVoice) ready: model present, toggle on.
    fn dictation_ready(&self, config: &ClaudeLocalSettings) -> bool;

    /// With native dictation enabled, ensure transcription service online before launch (it writes creds to `<state>/dictation-helper.json`,
    /// local compat service uses creds to transcribe microphone audio).
    fn prepare_dictation_service(&self) -> Result<()>;

    /// Launch 3p when stopping full-featured mode. Only Codex++ supports (via its gateway); default errors.
    fn launch_three_p(&self, _source: &Path, _root: &Path, _config: &ClaudeLocalSettings) -> Result<()> {
        bail!("Current program only supports full-featured mode")
    }

    /// Diagnostic log; default discarded.
    fn diagnostic(&self, _event: &str, _detail: Value) {}
}

fn sha_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

static HOST: OnceLock<Arc<dyn Host>> = OnceLock::new();

/// Install host once at process start; repeated calls ignored (returns false).
pub fn install_host(host: Arc<dyn Host>) -> bool {
    HOST.set(host).is_ok()
}

pub(crate) fn host() -> &'static Arc<dyn Host> {
    HOST.get().expect("claude_local::install_host must be called before use")
}

pub(crate) fn diagnostic(event: &str, detail: Value) {
    if let Some(host) = HOST.get() {
        host.diagnostic(event, detail);
    }
}