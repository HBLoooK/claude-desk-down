//! Claude Desktop full-feature mode settings (Codex++ stores under `claudeDesktopLocal` in settings.json, ClaudeDeskDown same shape).

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeLocalSettings {
    pub enabled: bool,
    pub model: String,
    pub localization: bool,
    pub voice_input: bool,
    pub computer_use: bool,
    pub background_mode: bool,
    /// Ignore Claude's macOS version ceiling for background mode (newer macOS prompts "not supported", default off).
    /// Enabling also enables background mode; not verified by Claude on this OS, may be unstable.
    pub force_background_mode: bool,
    /// Master switch for Cowork (GrowthBook `yukon_silver`, default on). Recording template has it off,
    /// newer Claude versions therefore only show Chat / Code. Platform limits (MSIX, HCS, etc.) still checked by Claude.
    pub cowork: bool,
    /// Use Claude's merged homepage (GrowthBook `claude_ai_hub_web_delivery`, default off): Chat and Cowork in one input, no "Chat | Cowork" toggle.
    /// Recording template's account is in this experiment group; off = classic homepage with toggle.
    pub hub_home: bool,
    /// Which recorded version the recording mode uses (frontend build id). Empty or deprecated version falls back to latest.
    pub template: String,
    /// Select "Local" interface (default recorded): launch in Claude's own third-party mode (3P), built-in new UI,
    /// Chat / Cowork / Code all call inference gateway directly (local service), no claude.ai account simulation, no local cert trust needed.
    /// No account-related features; Claude installer's UI lacks English.
    /// Old `localShell` setting (recorded mode data with local UI, mismatch) removed; ignored if read.
    pub three_p_mode: bool,
    /// Username shown in Claude; empty = "Local User".
    pub display_name: String,
    /// Email shown in Claude; empty = "local@claude.local".
    pub email: String,
    /// Plan shown in Claude: free / pro / max / ultra.
    pub plan: String,
    /// Sync 3p mode and full-feature mode Code sessions on launch (default off).
    pub session_sync: bool,
    /// Whether models passed to Claude are filtered by context window: default lists only models with window >= default model's
    /// (Claude's window is a single global value at startup, switching to smaller model doesn't shrink it).
    /// Enable to list all models in pool; trade-off is smaller-window models may exceed limit when selected.
    pub list_all_models: bool,
    /// Hide Claude built-in models that have no matching route in provider pool (Opus / Sonnet / Haiku native entries, default on).
    /// These entries would be swapped to default model if selected; off shows them as-is.
    pub hide_unrouted_models: bool,
    /// Whether Code tab (built-in Claude Code) shares ~/.claude with CLI: sessions, MCP, plugins are same.
    /// Trade-off: `~/.claude/settings.json` `env` (e.g. `ANTHROPIC_BASE_URL`) overrides upstream from this app.
    /// When absent, falls back to host's default (ClaudeDeskDown shares, Codex++ separate).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_share_home: Option<bool>,
    /// Before launch, register CLI-created (terminal, BaoCode, etc.) Claude Code sessions in Desktop's session list (default on).
    pub import_cli_sessions: bool,
}

impl Default for ClaudeLocalSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            model: String::new(),
            localization: false,
            voice_input: true,
            computer_use: false,
            background_mode: false,
            force_background_mode: false,
            cowork: true,
            hub_home: false,
            template: String::new(),
            three_p_mode: false,
            display_name: String::new(),
            email: String::new(),
            plan: "ultra".into(),
            session_sync: false,
            list_all_models: false,
            hide_unrouted_models: true,
            code_share_home: None,
            import_cli_sessions: true,
        }
    }
}

impl ClaudeLocalSettings {
    /// Plan only recognizes free / pro / max / ultra (case-insensitive), others default to ultra.
    pub fn normalized_plan(&self) -> &str {
        match self.plan.trim().to_ascii_lowercase().as_str() {
            "free" => "free",
            "pro" => "pro",
            "max" => "max",
            _ => "ultra",
        }
    }

    /// Display name: trim, remove control chars, max 64 chars; empty defaults to "Local User".
    pub fn effective_display_name(&self) -> String {
        let name: String = self
            .display_name
            .trim()
            .chars()
            .filter(|ch| !ch.is_control())
            .take(64)
            .collect();
        let name = name.trim();
        if name.is_empty() {
            "Local User".to_string()
        } else {
            name.to_string()
        }
    }

    /// Whether user-provided email is usable: trim -> empty, or form `a@b`
    /// (max 254 chars, no whitespace/control chars, `@` both sides non-empty).
    pub fn email_is_valid(&self) -> bool {
        normalized_email(&self.email).is_some()
    }

    /// Display email: empty or invalid format defaults to "local@claude.local".
    pub fn effective_email(&self) -> String {
        normalized_email(&self.email)
            .filter(|email| !email.is_empty())
            .unwrap_or_else(|| "local@claude.local".to_string())
    }
}

/// Email normalization: `Some("")` = not provided, `None` = invalid format.
fn normalized_email(raw: &str) -> Option<String> {
    let email = raw.trim();
    if email.is_empty() {
        return Some(String::new());
    }
    if email.chars().count() > 254 || email.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return None;
    }
    let (local, domain) = email.rsplit_once('@')?;
    (!local.is_empty() && !domain.is_empty()).then(|| email.to_string())
}