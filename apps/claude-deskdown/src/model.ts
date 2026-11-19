/**
 * ClaudeDeskDown UI pure logic: backend data types, status text, form validation.
 * No React / Tauri dependency — easy to unit test.
 */

export type CommandStatus = "ok" | "failed" | string;
export type CommandResult<T> = T & { status: CommandStatus; message: string };

export type UpstreamSource = "manual" | "ccswitch" | "magpie" | "codexpp";

/** One upstream profile (matches core `upstreams::UpstreamProfile` fields). */
export type UpstreamProfile = {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  smallModel: string;
  models: string[];
  contextWindow: number | null;
  /** Model display name (from upstream `/v1/models` display_name). */
  modelNames: Record<string, string>;
  source: UpstreamSource | string;
  sourceId: string;
};

export type UpstreamBook = { profiles: UpstreamProfile[]; activeId: string | null };

export type UpstreamModel = { id: string; displayName: string | null; contextWindow: number | null };

/** Display model name in UI: use display name if available, else id. */
export function modelLabel(profile: Pick<UpstreamProfile, "modelNames">, id: string): string {
  return profile.modelNames?.[id] || id;
}

/** Claude local service settings (matches core `ClaudeLocalSettings`, only UI-used fields listed). */
export type ClaudeConfig = {
  enabled: boolean;
  model: string;
  localization: boolean;
  voiceInput: boolean;
  computerUse: boolean;
  backgroundMode: boolean;
  /** Ignore Claude's macOS version ceiling for background mode. */
  forceBackgroundMode?: boolean;
  /** Master switch for Cowork (default on). */
  cowork?: boolean;
  /** Use Claude's merged homepage: Chat and Cowork combined, no 'Chat | Cowork' toggle (default off). */
  hubHome?: boolean;
  /** Which recorded version the recording mode uses (frontend build id); empty = latest. */
  template?: string;
  /** Select 'Local' interface: launch in Claude's own third-party mode (built-in new UI, inference via local service; default recorded). */
  threePMode?: boolean;
  displayName?: string;
  email?: string;
  plan?: string;
  sessionSync?: boolean;
  listAllModels?: boolean;
  hideUnroutedModels?: boolean;
  /** Whether Code tab shares ~/.claude with CLI. */
  codeShareHome?: boolean;
  /** Before launch, add CLI-created sessions to Desktop's session list. */
  importCliSessions?: boolean;
};

export type ClaudeStatus = {
  supported: boolean;
  sourceApp: string;
  version: string;
  claudeRunning: boolean;
  running: boolean;
  localProcess?: boolean;
  connected: boolean;
  port?: number | null;
  caReady: boolean;
  caTrusted: boolean;
  caMachineSupported: boolean;
  caMachineTrusted: boolean;
  /** Embedded recorded versions, newest first. `builtAt` is that version's frontend build time (seconds). */
  templates?: Array<{ build: string; builtAt: number | null }>;
  nodeAvailable: boolean;
  dictationReady: boolean;
  config: ClaudeConfig;
  models: string[];
  contextWindow?: number | null;
  warning?: string | null;
  needsRestart: boolean;
};

export const SOURCE_LABELS: Record<string, string> = {
  manual: "Manual",
  ccswitch: "cc-switch",
  magpie: "Magpie",
  codexpp: "Codex++",
};

export const IMPORT_SOURCES: Array<{ id: UpstreamSource; label: string; detail: string }> = [
  { id: "ccswitch", label: "cc-switch", detail: "Import cc-switch's Claude providers" },
  { id: "magpie", label: "Magpie", detail: "Use Magpie's local gateway as upstream" },
  { id: "codexpp", label: "Codex++", detail: "Use Codex++'s local gateway as upstream" },
];

export function emptyProfile(): UpstreamProfile {
  return { id: "", name: "", baseUrl: "", apiKey: "", model: "", smallModel: "", models: [], contextWindow: null, modelNames: {}, source: "manual", sourceId: "" };
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url.trim();
  }
}

/** Form validation: return first problem, or null if none. */
export function profileProblem(profile: UpstreamProfile): string | null {
  const base = profile.baseUrl.trim();
  if (!base) return "Enter a Base URL";
  let parsed: URL;
  try {
    parsed = new URL(base);
  } catch {
    return "Base URL is not a valid address";
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return "Base URL must start with http:// or https://";
  if (/\/v1\/messages\/?$/.test(parsed.pathname)) return "Base URL should be root only, don't include /v1/messages";
  if (!profile.model.trim()) return "Pick a default model";
  return null;
}

/** Deduplicate list, remove blanks, primary model first. */
export function normalizeModels(models: string[], primary = ""): string[] {
  const out: string[] = [];
  for (const raw of [primary, ...models]) {
    const model = raw.trim();
    if (model && !out.includes(model)) out.push(model);
  }
  return out;
}

export type Tone = "ok" | "warn" | "off" | "busy";

/** Top status bar: one line + color tone. */
export function claudeState(status: ClaudeStatus | null, busy: string | null): { text: string; tone: Tone } {
  if (busy === "launch") return { text: "Launching…", tone: "busy" };
  if (busy === "stop") return { text: "Stopping…", tone: "busy" };
  if (!status) return { text: "Detecting Claude…", tone: "busy" };
  if (!status.supported) return { text: "Claude Desktop not found", tone: "warn" };
  if (status.connected) return { text: "Connected", tone: "ok" };
  if (status.claudeRunning && status.localProcess) return { text: "Running", tone: "ok" };
  if (status.claudeRunning) return { text: "Claude is open but not launched via ClaudeDeskDown", tone: "warn" };
  if (status.running) return { text: "Local service ready", tone: "ok" };
  return { text: "Not started", tone: "off" };
}

/** What's missing before launch: listed in order, UI gives buttons for each. */
export function setupSteps(status: ClaudeStatus | null, active: UpstreamProfile | null): Array<{ id: "upstream" | "node" | "trust"; text: string }> {
  if (!status || !status.supported) return [];
  const steps: Array<{ id: "upstream" | "node" | "trust"; text: string }> = [];
  if (!active || profileProblem(active)) steps.push({ id: "upstream", text: "Add an upstream and pick a default model" });
  if (!status.nodeAvailable) steps.push({ id: "node", text: "Node.js required (version with fetch and WebSocket support)" });
  // Third-party mode: Claude doesn't go through proxy, no need to trust local cert.
  if (!status.caTrusted && !status.config.threePMode) steps.push({ id: "trust", text: "First run requires trusting local certificate" });
  return steps;
}

export const WINDOW_PRESETS: Array<{ value: number | null; label: string }> = [
  { value: null, label: "Unspecified (200k)" },
  { value: 200_000, label: "200k" },
  { value: 400_000, label: "400k" },
  { value: 1_000_000, label: "1M" },
];

export function formatWindow(value: number | null | undefined): string {
  if (!value) return "200k";
  if (value >= 1_000_000) return `${+(value / 1_000_000).toFixed(1)}M`;
  return `${Math.round(value / 1000)}k`;
}

export function isOk(result: { status: CommandStatus }): boolean {
  return result.status === "ok";
}

/** Recorded version display: frontend build date + build id, e.g. "2026-11-14 · a4ced7f85b". */
export function templateLabel(template: { build: string; builtAt: number | null }): string {
  if (!template.builtAt) return template.build;
  const date = new Date(template.builtAt * 1000);
  const day = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  return `${day} · ${template.build}`;
}
