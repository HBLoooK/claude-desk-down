//! ClaudeDeskDown upstreams: multiple Base URL + Key profiles, passed directly to Claude Desktop full-feature mode.
//!
//! Three import sources, all become plain Base URL + Key after import:
//! - **cc-switch**: reads providers with `app_type = 'claude'` from `~/.cc-switch/cc-switch.db` (Anthropic format only);
//! - **Magpie**: uses Magpie's local gateway as upstream, address from `~/.config/magpie/gateway.json`,
//!   token `magpie-claude-desktop` (Magpie only validates non-local requests, suffix for usage attribution);
//! - **Codex++**: uses Codex++'s local gateway as upstream, using the token it issued for Claude Desktop / Claude Code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use claude_local::Upstream;

pub const SOURCE_MANUAL: &str = "manual";
pub const SOURCE_CC_SWITCH: &str = "ccswitch";
pub const SOURCE_MAGPIE: &str = "magpie";
pub const SOURCE_CODEX_PLUS_PLUS: &str = "codexpp";

/// Magpie gateway default port (`settings.DefaultPort`).
const MAGPIE_DEFAULT_ADDR: &str = "127.0.0.1:3425";
/// Magpie attributes usage by `magpie-<agent>`; local requests not validated, value just gives client a token to carry.
const MAGPIE_TOKEN: &str = "magpie-claude-desktop";
const BOOK_FILE: &str = "upstreams.json";

/// One upstream profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpstreamProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub small_model: String,
    pub models: Vec<String>,
    pub context_window: Option<u64>,
    /// Model display name (from upstream `/v1/models` `display_name`). Magpie's routing group IDs not readable, so we rely on this.
    pub model_names: BTreeMap<String, String>,
    /// `manual` / `ccswitch` / `magpie` / `codexpp`.
    pub source: String,
    /// Source identifier (e.g., cc-switch provider ID); re-import updates by this instead of duplicating.
    pub source_id: String,
}

impl UpstreamProfile {
    /// Convert to upstream for local compatible service: default model first, deduped; all models share one window (omitted if not declared).
    pub fn to_upstream(&self) -> Upstream {
        let model = self.model.trim().to_string();
        let mut models: Vec<String> = Vec::new();
        for candidate in std::iter::once(&self.model).chain(self.models.iter()) {
            let candidate = candidate.trim();
            if !candidate.is_empty() && !models.iter().any(|known| known == candidate) {
                models.push(candidate.to_string());
            }
        }
        let small_model = Some(self.small_model.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| model.clone());
        let window = self.context_window.filter(|window| *window > 0);
        let model_windows = window
            .map(|window| models.iter().map(|id| (id.clone(), window)).collect())
            .unwrap_or_default();
        let model_names = models
            .iter()
            .map(|id| {
                let name = self
                    .model_names
                    .get(id)
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| claude_local::friendly_model_name(id.rsplit('/').next().unwrap_or(id)));
                (id.clone(), serde_json::json!({ "name": name, "description": self.name }))
            })
            .collect();
        Upstream {
            root: self.base_url.trim().trim_end_matches('/').to_string(),
            token: self.api_key.trim().to_string(),
            default_model: model,
            small_model,
            models,
            hidden_models: Vec::new(),
            model_names,
            model_windows,
            default_window: window,
            extra_env: Vec::new(),
        }
    }
}

/// All upstreams and the currently selected one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpstreamBook {
    pub profiles: Vec<UpstreamProfile>,
    pub active_id: Option<String>,
}

impl UpstreamBook {
    pub fn active(&self) -> Option<&UpstreamProfile> {
        let id = self.active_id.as_deref()?;
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// Add or update a profile: no id = new; first profile auto-selected. Returns its id.
    pub fn upsert(&mut self, mut profile: UpstreamProfile) -> String {
        profile.base_url = profile.base_url.trim().trim_end_matches('/').to_string();
        profile.name = profile.name.trim().to_string();
        if profile.name.is_empty() {
            profile.name = host_of(&profile.base_url).unwrap_or_else(|| "Unnamed".into());
        }
        if profile.source.is_empty() {
            profile.source = SOURCE_MANUAL.into();
        }
        if profile.id.is_empty() {
            profile.id = uuid::Uuid::new_v4().to_string();
        }
        let id = profile.id.clone();
        match self.profiles.iter_mut().find(|known| known.id == id) {
            Some(known) => *known = profile,
            None => self.profiles.push(profile),
        }
        if self.active().is_none() {
            self.active_id = Some(id.clone());
        }
        id
    }

    /// Import: existing entry with same source + source_id only updates address and key, preserves user's model choices and name; otherwise adds new.
    /// Returns `(added, updated)`.
    pub fn merge_imported(&mut self, imported: Vec<UpstreamProfile>) -> (usize, usize) {
        let (mut added, mut updated) = (0, 0);
        for profile in imported {
            let existing = self.profiles.iter_mut().find(|known| {
                known.source == profile.source && known.source_id == profile.source_id
            });
            match existing {
                Some(known) => {
                    known.base_url = profile.base_url;
                    known.api_key = profile.api_key;
                    if known.model.is_empty() {
                        known.model = profile.model;
                        known.small_model = profile.small_model;
                    }
                    for model in profile.models {
                        if !known.models.contains(&model) {
                            known.models.push(model);
                        }
                    }
                    known.model_names.extend(profile.model_names);
                    updated += 1;
                }
                None => {
                    self.upsert(UpstreamProfile { id: String::new(), ..profile });
                    added += 1;
                }
            }
        }
        (added, updated)
    }

    pub fn remove(&mut self, id: &str) {
        self.profiles.retain(|profile| profile.id != id);
        if self.active().is_none() {
            self.active_id = self.profiles.first().map(|profile| profile.id.clone());
        }
    }
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_string)
}

pub fn default_book_path() -> PathBuf {
    crate::host::state_dir().join(BOOK_FILE)
}

pub fn load_book(path: &Path) -> Result<UpstreamBook> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("Upstream config corrupted: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UpstreamBook::default()),
        Err(error) => Err(error).with_context(|| format!("Failed to read upstream config: {}", path.display())),
    }
}

/// Save to disk (includes keys, Unix permissions tightened to user-only).
pub fn save_book(path: &Path, book: &UpstreamBook) -> Result<()> {
    claude_local::fsutil::atomic_write(path, &serde_json::to_vec_pretty(book)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}


// ── cc-switch ──────────────────────────────────────────────────────────

/// cc-switch database location, follows its own rules: custom config dir from settings (`app_paths.json`) preferred,
/// then `~/.cc-switch`; on Windows, if default missing, also check old `HOME` location (from v3.10.3).
pub fn default_cc_switch_db() -> PathBuf {
    let Some(dirs) = directories::BaseDirs::new() else {
        return PathBuf::from(".cc-switch/cc-switch.db");
    };
    let home = dirs.home_dir();
    let store = dirs.data_dir().join("com.ccswitch.desktop").join("app_paths.json");
    if let Some(dir) = cc_switch_dir_override(&store, home) {
        return dir.join("cc-switch.db");
    }
    let default_db = home.join(".cc-switch").join("cc-switch.db");
    if cfg!(windows) && !default_db.exists()
        && let Ok(legacy_home) = std::env::var("HOME")
        && !legacy_home.trim().is_empty()
    {
        let legacy_db = PathBuf::from(legacy_home.trim()).join(".cc-switch").join("cc-switch.db");
        if legacy_db.exists() {
            return legacy_db;
        }
    }
    default_db
}

/// cc-switch custom config dir from settings (stored in its `app_paths.json`); dir non-existent = ignored by cc-switch too.
fn cc_switch_dir_override(store: &Path, home: &Path) -> Option<PathBuf> {
    let store: Value = serde_json::from_str(&std::fs::read_to_string(store).ok()?).ok()?;
    let raw = store.get("app_config_dir_override")?.as_str()?.trim();
    let path = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        home.join(rest)
    } else if raw.is_empty() {
        return None;
    } else {
        PathBuf::from(raw)
    };
    path.is_dir().then_some(path)
}

/// cc-switch providers for Claude Code (`claude`) and Claude Desktop (`claude-desktop`).
/// OpenAI / Gemini formats go through its own proxy, not imported here; same address+key on both sides imported once.
pub fn cc_switch_profiles(db: &Path) -> Result<Vec<UpstreamProfile>> {
    if !db.is_file() {
        bail!("cc-switch database not found: {}", db.display());
    }
    let connection = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("Failed to open cc-switch database: {}", db.display()))?;
    let mut statement = connection.prepare(
        "SELECT id, app_type, name, settings_config, COALESCE(meta, '{}') FROM providers \
         WHERE app_type IN ('claude', 'claude-desktop') \
         ORDER BY app_type = 'claude-desktop', COALESCE(sort_index, 0), created_at",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut profiles: Vec<UpstreamProfile> = Vec::new();
    let mut other_formats = 0;
    for row in rows {
        let (id, app_type, name, settings, meta) = row?;
        if api_format(&meta) != "anthropic" {
            other_formats += 1;
            continue;
        }
        // Desktop provider ID may clash with Claude Code's (cc-switch distinguishes by id + category), add prefix to avoid collision.
        let source_id = if app_type == "claude" { id } else { format!("{app_type}:{id}") };
        let Some(profile) = cc_switch_profile(&source_id, &name, &settings) else { continue };
        if !profiles
            .iter()
            .any(|known| known.base_url == profile.base_url && known.api_key == profile.api_key)
        {
            profiles.push(profile);
        }
    }
    if profiles.is_empty() && other_formats > 0 {
        bail!(
            "cc-switch's {other_formats} Claude providers are all non-Anthropic format (e.g. OpenAI Chat), \
             they require cc-switch's own proxy to convert, ClaudeDeskDown can't connect directly. Add Anthropic-compatible address and key manually in ClaudeDeskDown."
        );
    }
    Ok(profiles)
}

fn api_format(meta: &str) -> String {
    serde_json::from_str::<Value>(meta)
        .ok()
        .and_then(|meta| meta.get("apiFormat").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "anthropic".into())
}

fn cc_switch_profile(id: &str, name: &str, settings: &str) -> Option<UpstreamProfile> {
    let settings: Value = serde_json::from_str(settings).ok()?;
    let env = settings.get("env")?;
    let text = |key: &str| {
        env.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let base_url = text("ANTHROPIC_BASE_URL")?;
    let api_key = text("ANTHROPIC_AUTH_TOKEN")
        .or_else(|| text("ANTHROPIC_API_KEY"))
        .unwrap_or_default();
    let model = text("ANTHROPIC_MODEL")
        .or_else(|| text("ANTHROPIC_DEFAULT_OPUS_MODEL"))
        .or_else(|| text("ANTHROPIC_DEFAULT_SONNET_MODEL"))
        .unwrap_or_default();
    let small_model = text("ANTHROPIC_DEFAULT_HAIKU_MODEL")
        .or_else(|| text("ANTHROPIC_SMALL_FAST_MODEL"))
        .unwrap_or_default();
    let mut models = Vec::new();
    for key in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
    ] {
        if let Some(model) = text(key)
            && !models.contains(&model)
        {
            models.push(model);
        }
    }
    Some(UpstreamProfile {
        name: name.trim().to_string(),
        base_url,
        api_key,
        model,
        small_model,
        models,
        source: SOURCE_CC_SWITCH.into(),
        source_id: id.to_string(),
        ..UpstreamProfile::default()
    })
}

// ── Magpie ─────────────────────────────────────────────────────────────

pub fn default_magpie_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".config").join("magpie"))
        .unwrap_or_else(|| PathBuf::from(".config/magpie"))
}

/// Use Magpie's local gateway as upstream. Discovery order matches Magpie's own: `MAGPIE_ADDR`, `gateway.json`, default port.
pub fn magpie_profile(dir: &Path, env_addr: Option<&str>) -> UpstreamProfile {
    let recorded = std::fs::read(dir.join("gateway.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("addr").and_then(Value::as_str).map(str::to_string));
    let addr = env_addr
        .map(str::trim)
        .filter(|addr| !addr.is_empty())
        .map(str::to_string)
        .or(recorded)
        .unwrap_or_else(|| MAGPIE_DEFAULT_ADDR.into());
    UpstreamProfile {
        name: "Magpie".into(),
        base_url: format!("http://{}", loopback_addr(&addr)),
        api_key: MAGPIE_TOKEN.into(),
        source: SOURCE_MAGPIE.into(),
        source_id: "gateway".into(),
        ..UpstreamProfile::default()
    }
}

/// If listening on all interfaces (`0.0.0.0` / `[::]` / port only), local connects to 127.0.0.1.
fn loopback_addr(addr: &str) -> String {
    let addr = addr.trim().trim_start_matches("http://");
    let (host, port) = match addr.rsplit_once(':') {
        Some((host, port)) => (host, port),
        None => return MAGPIE_DEFAULT_ADDR.into(),
    };
    match host {
        "" | "0.0.0.0" | "[::]" | "::" => format!("127.0.0.1:{port}"),
        _ => format!("{host}:{port}"),
    }
}

// ── Codex++ ────────────────────────────────────────────────────────────

/// Use Codex++'s local gateway as upstream: prefers its Claude Desktop binding, then Claude Code.
/// Errors if neither bound, prompting user to bind in Codex++ first.
pub fn codex_plus_plus_profile(state_dir: &Path) -> Result<UpstreamProfile> {
    let path = state_dir.join("gateway.json");
    let gateway: Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("Codex++ gateway config corrupted: {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error).with_context(|| format!("Failed to read Codex++ gateway config: {}", path.display())),
    };
    // Local gateway port: Codex++ records in protocol-proxy-port when migrated, else default 57321.
    let port = std::fs::read_to_string(state_dir.join("protocol-proxy-port"))
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(57321);
    for agent in ["claude-desktop", "claude"] {
        let binding = &gateway["bindings"][agent];
        let enabled = binding["enabled"].as_bool().unwrap_or(false);
        let token = binding["token"].as_str().unwrap_or_default().trim();
        if enabled && !token.is_empty() {
            let model = binding["model"].as_str().unwrap_or_default().trim().to_string();
            return Ok(UpstreamProfile {
                name: "Codex++".into(),
                base_url: format!("http://127.0.0.1:{port}/gw/{agent}"),
                api_key: token.to_string(),
                models: if model.is_empty() { Vec::new() } else { vec![model.clone()] },
                model,
                source: SOURCE_CODEX_PLUS_PLUS.into(),
                source_id: "gateway".into(),
                ..UpstreamProfile::default()
            });
        }
    }
    bail!("Codex++ has no Claude bound yet: bind Claude Code or Claude Desktop in Codex++'s 'Agent Connect' first")
}

/// Codex++ state directory (`~/.codex-session-delete`).
pub fn codex_plus_plus_state_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".codex-session-delete"))
        .unwrap_or_else(|| PathBuf::from(".codex-session-delete"))
}

// ── Model list ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u64>,
}

/// Model-list URL for an Anthropic-compatible base URL. Accepts either the root
/// or a base that already ends in `/v1` (e.g. NVIDIA NIM), without doubling the segment.
fn models_url(base_url: &str) -> Result<reqwest::Url> {
    let base = base_url.trim().trim_end_matches('/');
    let root = base.strip_suffix("/v1").unwrap_or(base);
    reqwest::Url::parse(&format!("{root}/v1/models"))
        .with_context(|| format!("Base URL is not a valid address: {base_url}"))
}

/// `GET {base}/v1/models` (Anthropic format, Magpie / Codex++ gateways both support).
///
/// Some OpenAI-compatible gateways serve a *different, empty* model list when `anthropic-version`
/// is present: NVIDIA NIM switches into Anthropic-compat mode on that header and then answers
/// `/v1/models` with `{"data":[]}`. Ask with the header first (real Anthropic-compatible upstreams
/// expect it), and retry without it before believing an empty list.
pub async fn fetch_models(base_url: &str, api_key: &str) -> Result<Vec<UpstreamModel>> {
    let url = models_url(base_url)?;
    let with_version = request_models(&url, api_key, true).await?;
    if !with_version.is_empty() {
        return Ok(with_version);
    }
    request_models(&url, api_key, false).await
}

async fn request_models(
    url: &reqwest::Url,
    api_key: &str,
    anthropic_version: bool,
) -> Result<Vec<UpstreamModel>> {
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1" | "[::1]"));
    let mut client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20));
    if loopback {
        // Local gateway shouldn't go through system proxy, or request gets forwarded out.
        client = client.no_proxy();
    }
    let mut request = client
        .build()?
        .get(url.clone())
        .header("x-api-key", api_key.trim())
        .bearer_auth(api_key.trim());
    if anthropic_version {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = request.send().await.context("Failed to fetch model list")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail: String = body.chars().take(200).collect();
        bail!("Upstream returned {status}: {detail}");
    }
    parse_models(&body)
}

fn parse_models(body: &str) -> Result<Vec<UpstreamModel>> {
    let value: Value = serde_json::from_str(body).context("Model list is not JSON")?;
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .context("Model list missing data field")?;
    let mut seen = BTreeMap::new();
    let mut models = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("id").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if id.is_empty() || seen.insert(id.to_string(), ()).is_some() {
            continue;
        }
        models.push(UpstreamModel {
            id: id.to_string(),
            display_name: entry
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty() && *name != id)
                .map(str::to_string),
            context_window: entry.get("context_window").and_then(Value::as_u64).filter(|v| *v > 0),
        });
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal HTTP server: answers `/v1/models` with an empty list when `anthropic-version` is
    /// present and a populated list otherwise, reproducing NVIDIA NIM's behaviour.
    fn spawn_gateway() -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut has_anthropic_version = false;
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if trimmed.to_ascii_lowercase().starts_with("anthropic-version:") {
                        has_anthropic_version = true;
                    }
                    line.clear();
                }
                let body = if has_anthropic_version {
                    r#"{"data":[],"has_more":false}"#
                } else {
                    r#"{"object":"list","data":[{"id":"meta/muse-glimmer-30b"},{"id":"google/gemma-4-31b-it"}]}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn empty_list_from_anthropic_header_falls_back_to_openai_probe() {
        let base = spawn_gateway();
        let models = fetch_models(&base, "k").await.unwrap();
        assert_eq!(
            models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(),
            ["meta/muse-glimmer-30b", "google/gemma-4-31b-it"]
        );
    }

    #[test]
    fn model_list_url_never_doubles_the_v1_segment() {
        assert_eq!(models_url(" https://integrate.api.nvidia.com/v1/ ").unwrap().as_str(), "https://integrate.api.nvidia.com/v1/models");
        assert_eq!(models_url("https://integrate.api.nvidia.com").unwrap().as_str(), "https://integrate.api.nvidia.com/v1/models");
        assert_eq!(models_url("https://api.example.com/").unwrap().as_str(), "https://api.example.com/v1/models");
        assert!(models_url("not a url").is_err());
    }

    #[test]
    fn upsert_assigns_ids_names_and_selects_the_first_profile() {
        let mut book = UpstreamBook::default();
        let id = book.upsert(UpstreamProfile {
            base_url: "https://api.example.com/".into(),
            ..UpstreamProfile::default()
        });
        assert_eq!(book.active_id.as_deref(), Some(id.as_str()));
        let first = book.active().unwrap();
        assert_eq!(first.base_url, "https://api.example.com");
        assert_eq!(first.name, "api.example.com");
        assert_eq!(first.source, SOURCE_MANUAL);
        // Second doesn't steal selection; rename updates by id.
        let second = book.upsert(UpstreamProfile { name: "B".into(), ..UpstreamProfile::default() });
        assert_eq!(book.active_id.as_deref(), Some(id.as_str()));
        book.upsert(UpstreamProfile { id: second.clone(), name: "B2".into(), ..UpstreamProfile::default() });
        assert_eq!(book.profiles.len(), 2);
        assert_eq!(book.profiles[1].name, "B2");
        // Delete selected, selection falls to remaining first.
        book.remove(&id);
        assert_eq!(book.active_id.as_deref(), Some(second.as_str()));
    }

    #[test]
    fn reimport_updates_address_and_key_but_keeps_user_choices() {
        let mut book = UpstreamBook::default();
        let imported = |key: &str| UpstreamProfile {
            name: "Kimi".into(),
            base_url: "https://kimi.example".into(),
            api_key: key.into(),
            model: "kimi-k3".into(),
            models: vec!["kimi-k3".into()],
            source: SOURCE_CC_SWITCH.into(),
            source_id: "p1".into(),
            ..UpstreamProfile::default()
        };
        assert_eq!(book.merge_imported(vec![imported("old")]), (1, 0));
        book.profiles[0].name = "My Kimi".into();
        book.profiles[0].model = "kimi-k3-turbo".into();
        assert_eq!(book.merge_imported(vec![imported("new")]), (0, 1));
        let profile = &book.profiles[0];
        assert_eq!(profile.api_key, "new");
        assert_eq!(profile.name, "My Kimi");
        assert_eq!(profile.model, "kimi-k3-turbo");
    }

    #[test]
    fn book_round_trips_through_disk_and_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude-deskdown").join(BOOK_FILE);
        assert_eq!(load_book(&path).unwrap(), UpstreamBook::default());
        let mut book = UpstreamBook::default();
        book.upsert(UpstreamProfile { base_url: "https://a.example".into(), api_key: "k".into(), ..UpstreamProfile::default() });
        save_book(&path, &book).unwrap();
        assert_eq!(load_book(&path).unwrap(), book);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn cc_switch_claude_providers_are_read_and_non_anthropic_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cc-switch.db");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE providers (id TEXT, app_type TEXT, name TEXT, settings_config TEXT, \
                 meta TEXT DEFAULT '{}', sort_index INTEGER, created_at INTEGER, is_current BOOLEAN);",
            )
            .unwrap();
        let insert = |id: &str, app: &str, name: &str, settings: Value, meta: Value| {
            connection
                .execute(
                    "INSERT INTO providers (id, app_type, name, settings_config, meta, sort_index, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
                    rusqlite::params![id, app, name, settings.to_string(), meta.to_string()],
                )
                .unwrap();
        };
        insert(
            "a",
            "claude",
            "GLM",
            json!({"env": {
                "ANTHROPIC_BASE_URL": "https://open.bigmodel.example/api/anthropic",
                "ANTHROPIC_AUTH_TOKEN": "tok",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-5-air"
            }}),
            json!({}),
        );
        insert(
            "b",
            "claude",
            "OpenRouter",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://openrouter.example", "ANTHROPIC_API_KEY": "x"}}),
            json!({"apiFormat": "openai_chat"}),
        );
        insert("", "codex", "Codex", json!({"env": {"ANTHROPIC_BASE_URL": "https://x"}}), json!({}));
        insert("d", "claude", "No URL", json!({"env": {}}), json!({}));
        // cc-switch 4.x desktop providers; id may clash with Claude Code's, prefix avoids collision.
        insert(
            "a",
            "claude-desktop",
            "Kimi",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://kimi.example/anthropic", "ANTHROPIC_AUTH_TOKEN": "k"}}),
            json!({"claudeDesktopMode": "direct", "apiFormat": "anthropic"}),
        );
        // Same address+key on both sides: imported once.
        insert(
            "e",
            "claude-desktop",
            "GLM (Desktop)",
            json!({"env": {"ANTHROPIC_BASE_URL": "https://open.bigmodel.example/api/anthropic", "ANTHROPIC_AUTH_TOKEN": "tok"}}),
            json!({"claudeDesktopMode": "direct"}),
        );
        drop(connection);

        let profiles = cc_switch_profiles(&db).unwrap();
        assert_eq!(profiles.len(), 2);
        let kimi = &profiles[1];
        assert_eq!((kimi.name.as_str(), kimi.base_url.as_str()), ("Kimi", "https://kimi.example/anthropic"));
        assert_eq!(kimi.source_id, "claude-desktop:a");
        let glm = &profiles[0];
        assert_eq!(glm.name, "GLM");
        assert_eq!(glm.api_key, "tok");
        assert_eq!(glm.model, "glm-5");
        assert_eq!(glm.small_model, "glm-5-air");
        assert_eq!(glm.models, ["glm-5", "glm-5-air"]);
        assert_eq!((glm.source.as_str(), glm.source_id.as_str()), (SOURCE_CC_SWITCH, "a"));
        assert!(cc_switch_profiles(&dir.path().join("missing.db")).is_err());
    }

    #[test]
    fn cc_switch_with_only_openai_format_providers_explains_why() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cc-switch.db");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE providers (id TEXT, app_type TEXT, name TEXT, settings_config TEXT, \
                 meta TEXT DEFAULT '{}', sort_index INTEGER, created_at INTEGER);\n\
                 INSERT INTO providers VALUES ('x', 'claude-desktop', 'DeepSeek', \
                 '{\"env\":{\"ANTHROPIC_BASE_URL\":\"https://api.deepseek.example\"}}', \
                 '{\"apiFormat\":\"openai_chat\"}', 0, 0);",
            )
            .unwrap();
        drop(connection);
        let error = format!("{:#}", cc_switch_profiles(&db).unwrap_err());
        assert!(error.contains("1 ") && error.contains("Anthropic format"), "{error}");
    }

    #[test]
    fn cc_switch_custom_config_dir_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("my-ccs");
        std::fs::create_dir_all(&custom).unwrap();
        let store = dir.path().join("app_paths.json");
        std::fs::write(&store, json!({"app_config_dir_override": custom.to_string_lossy()}).to_string()).unwrap();
        assert_eq!(cc_switch_dir_override(&store, dir.path()), Some(custom.clone()));
        std::fs::write(&store, json!({"app_config_dir_override": "~/my-ccs"}).to_string()).unwrap();
        assert_eq!(cc_switch_dir_override(&store, dir.path()), Some(custom));
        // Dir missing / not set = fall back to default.
        std::fs::write(&store, json!({"app_config_dir_override": "/nope/missing"}).to_string()).unwrap();
        assert_eq!(cc_switch_dir_override(&store, dir.path()), None);
        assert_eq!(cc_switch_dir_override(&dir.path().join("none.json"), dir.path()), None);
    }

    #[test]
    fn magpie_gateway_is_discovered_like_magpie_does() {
        let dir = tempfile::tempdir().unwrap();
        // No gateway.json: default port.
        assert_eq!(magpie_profile(dir.path(), None).base_url, "http://127.0.0.1:3425");
        std::fs::write(dir.path().join("gateway.json"), r#"{"addr":"0.0.0.0:4111","pid":1}"#).unwrap();
        let profile = magpie_profile(dir.path(), None);
        assert_eq!(profile.base_url, "http://127.0.0.1:4111");
        assert_eq!(profile.api_key, "magpie-claude-desktop");
        assert_eq!(profile.source, SOURCE_MAGPIE);
        // MAGPIE_ADDR takes precedence.
        assert_eq!(magpie_profile(dir.path(), Some("127.0.0.1:5000")).base_url, "http://127.0.0.1:5000");
    }

    #[test]
    fn codex_plus_plus_gateway_needs_a_claude_binding() {
        let dir = tempfile::tempdir().unwrap();
        assert!(codex_plus_plus_profile(dir.path()).is_err(), "No Claude bound = error");
        std::fs::write(dir.path().join("protocol-proxy-port"), "58001\n").unwrap();
        std::fs::write(
            dir.path().join("gateway.json"),
            json!({"bindings": {"claude": {"enabled": true, "token": "cpp-token", "model": "deepseek/v4"}}})
                .to_string(),
        )
        .unwrap();
        let profile = codex_plus_plus_profile(dir.path()).unwrap();
        assert_eq!(profile.base_url, "http://127.0.0.1:58001/gw/claude");
        assert_eq!(profile.api_key, "cpp-token");
        assert_eq!(profile.model, "deepseek/v4");
        assert_eq!(profile.source, SOURCE_CODEX_PLUS_PLUS);
    }

    #[test]
    fn model_lists_parse_anthropic_shape_with_optional_windows() {
        let models = parse_models(
            &json!({"data": [
                {"id": "a/claude-opus", "context_window": 1_000_000, "display_name": "Claude Opus · A"},
                {"id": "b/glm-5"},
                {"id": "b/glm-5"},
                {"id": ""}
            ]})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            models,
            [
                UpstreamModel { id: "a/claude-opus".into(), display_name: Some("Claude Opus · A".into()), context_window: Some(1_000_000) },
                UpstreamModel { id: "b/glm-5".into(), display_name: None, context_window: None },
            ]
        );
        assert!(parse_models("{}").is_err());
    }
}