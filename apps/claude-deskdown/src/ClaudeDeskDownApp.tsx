import { invoke } from "@tauri-apps/api/core";
import { Check, ChevronDown, Download, Eye, EyeOff, Loader2, Play, Plus, RefreshCw, ShieldCheck, Square, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { IMPORT_SOURCES, SOURCE_LABELS, WINDOW_PRESETS, claudeState, emptyProfile, formatWindow, hostOf, isOk, modelLabel, normalizeModels, profileProblem, setupSteps, type ClaudeConfig, type ClaudeStatus, type CommandResult, type UpstreamBook, type UpstreamModel, type UpstreamProfile, templateLabel } from "./model";
import { checkForUpdate, currentVersion, installUpdate, type UpdateState } from "./updates";

type Tab = "upstream" | "claude";
type Notice = { text: string; tone: "ok" | "error" } | null;
type ClaudeAction = "launch" | "stop" | "trust" | "untrust" | "trust_machine" | "prepare" | "sync_sessions";

const errorText = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));

export function ClaudeDeskDownApp() {
  const [book, setBook] = useState<UpstreamBook>({ profiles: [], activeId: null });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<UpstreamProfile | null>(null);
  const [status, setStatus] = useState<ClaudeStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const [tab, setTab] = useState<Tab>("upstream");
  const pending = useRef(false);

  const say = (text: string, tone: "ok" | "error" = "ok") => setNotice(text ? { text, tone } : null);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), notice.tone === "error" ? 12000 : 5000);
    return () => clearTimeout(timer);
  }, [notice]);
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<UpdateState>({ phase: "idle" });

  useEffect(() => {
    void currentVersion().then(setVersion);
    const timer = setTimeout(() => {
      void checkForUpdate().then((result) => {
        if (result.phase === "available") setUpdate(result);
      });
    }, 5000);
    return () => clearTimeout(timer);
  }, []);

  const runUpdateCheck = async () => {
    setUpdate({ phase: "checking" });
    const result = await checkForUpdate();
    setUpdate(result);
    if (result.phase === "failed") say(`Update check failed: ${result.message}`, "error");
  };

  const runInstall = async () => {
    if (update.phase !== "available") return;
    const { update: pending, version: target } = update;
    setUpdate({ phase: "downloading", version: target, percent: null });
    try {
      await installUpdate(pending, (percent) => setUpdate({ phase: "downloading", version: target, percent }));
    } catch (cause) {
      setUpdate({ phase: "failed", message: errorText(cause) });
      say(`Update failed: ${errorText(cause)}`, "error");
    }
  };

  const acceptBook = useCallback((result: CommandResult<UpstreamBook>, keepSelection?: string | null) => {
    const next = { profiles: result.profiles ?? [], activeId: result.activeId ?? null };
    setBook(next);
    const wanted = keepSelection ?? null;
    const target = next.profiles.find((profile) => profile.id === wanted) ?? next.profiles.find((profile) => profile.id === next.activeId) ?? next.profiles[0] ?? null;
    setSelectedId(target?.id ?? null);
    setDraft(target ? { ...target } : null);
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const result = await invoke<CommandResult<ClaudeStatus>>("claude_local_status");
      if (isOk(result)) setStatus(result);
      else say(result.message, "error");
    } catch (cause) {
      say(errorText(cause), "error");
    }
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const result = await invoke<CommandResult<UpstreamBook>>("deskdown_load");
        acceptBook(result);
        if (!isOk(result)) say(result.message, "error");
      } catch (cause) {
        say(errorText(cause), "error");
      }
      await refreshStatus();
    })();
  }, [acceptBook, refreshStatus]);

  useEffect(() => {
    if ((!status?.running && !status?.claudeRunning) || busy) return;
    const timer = setInterval(() => void refreshStatus(), 3000);
    return () => clearInterval(timer);
  }, [status?.running, status?.claudeRunning, busy, refreshStatus]);

  const run = async <T,>(label: string, task: () => Promise<T>): Promise<T | undefined> => {
    if (pending.current) return undefined;
    pending.current = true;
    setBusy(label);
    try {
      return await task();
    } catch (cause) {
      say(errorText(cause), "error");
      return undefined;
    } finally {
      pending.current = false;
      setBusy(null);
    }
  };

  const claudeAction = (action: ClaudeAction) =>
    run(action, async () => {
      const result = await invoke<CommandResult<ClaudeStatus>>("claude_local_action", { action });
      if (isOk(result)) setStatus(result);
      say(result.message, isOk(result) ? "ok" : "error");
    });

  const configure = (patch: Partial<ClaudeConfig>) =>
    run("configure", async () => {
      if (!status) return;
      const config = { ...status.config, ...patch, enabled: true };
      const result = await invoke<CommandResult<ClaudeStatus>>("claude_local_configure", { config });
      if (isOk(result)) setStatus(result);
      else say(result.message, "error");
    });

  const setVoice = (enabled: boolean) =>
    run("voice", async () => {
      const result = await invoke<CommandResult<ClaudeStatus>>("deskdown_set_voice", { enabled });
      if (isOk(result)) setStatus(result);
      say(result.message, isOk(result) ? "ok" : "error");
    });

  const bookCall = (label: string, command: string, args: Record<string, unknown>, keep?: string | null) =>
    run(label, async () => {
      const result = await invoke<CommandResult<UpstreamBook>>(command, args);
      acceptBook(result, keep);
      say(result.message, isOk(result) ? "ok" : "error");
      await refreshStatus();
      return result;
    });

  const active = book.profiles.find((profile) => profile.id === book.activeId) ?? null;
  const dirty = useMemo(() => {
    if (!draft) return false;
    const saved = book.profiles.find((profile) => profile.id === draft.id);
    return !saved || JSON.stringify(saved) !== JSON.stringify(draft);
  }, [book, draft]);

  const select = (id: string) => {
    const profile = book.profiles.find((item) => item.id === id);
    if (!profile) return;
    setSelectedId(id);
    setDraft({ ...profile });
    setTab("upstream");
  };

  const addProfile = () => {
    setSelectedId(null);
    setDraft(emptyProfile());
    setTab("upstream");
  };

  const saveDraft = async (activate: boolean) => {
    if (!draft) return;
    const problem = profileProblem(draft);
    if (problem) return say(problem, "error");
    const profile = { ...draft, models: normalizeModels(draft.models, draft.model) };
    const result = await bookCall("save", "deskdown_save_profile", { profile, activate }, draft.id || undefined);
    if (result && isOk(result) && !draft.id) {
      const created = [...(result.profiles ?? [])].reverse().find((item) => item.baseUrl === profile.baseUrl.trim().replace(/\/+$/, ""));
      if (created) {
        setSelectedId(created.id);
        setDraft({ ...created });
      }
    }
  };

  const fetchModels = () =>
    run("models", async () => {
      if (!draft) return;
      const result = await invoke<CommandResult<{ models: UpstreamModel[] }>>("deskdown_fetch_models", { baseUrl: draft.baseUrl, apiKey: draft.apiKey });
      if (!isOk(result)) return say(result.message, "error");
      const ids = result.models.map((model) => model.id);
      setDraft((current) => {
        if (!current) return current;
        const model = current.model || ids[0] || "";
        const window = result.models.find((item) => item.id === model)?.contextWindow ?? current.contextWindow;
        const names = Object.fromEntries(result.models.filter((item) => item.displayName).map((item) => [item.id, item.displayName as string]));
        return { ...current, models: normalizeModels(ids, model), model, contextWindow: window ?? null, modelNames: { ...current.modelNames, ...names } };
      });
      say(result.message);
    });

  const state = claudeState(status, busy === "launch" || busy === "stop" ? busy : null);
  const steps = setupSteps(status, active);
  const canLaunch = Boolean(status?.supported && status.nodeAvailable && (status.caTrusted || status.config.threePMode) && active && !profileProblem(active));
  const showStop = Boolean(status?.running || status?.localProcess);

  return (
    <div className="cm-shell">
      <aside className="cm-side">
        <div className="cm-side-head" data-tauri-drag-region>
          <span className="cm-brand">ClaudeDeskDown</span>
        </div>
        <div className="cm-side-label">Upstreams</div>
        <nav className="cm-list" aria-label="Upstreams">
          {book.profiles.map((profile) => (
            <button
              aria-current={profile.id === selectedId ? "true" : undefined}
              className="cm-item"
              key={profile.id}
              onClick={() => select(profile.id)}
              onDoubleClick={() => void bookCall("activate", "deskdown_set_active", { id: profile.id }, profile.id)}
              title="Double-click to set as active"
              type="button"
            >
              <span className="cm-radio" data-on={profile.id === book.activeId || undefined} />
              <span className="cm-item-text">
                <span className="cm-item-name">{profile.name || hostOf(profile.baseUrl)}</span>
                <span className="cm-item-sub">{profile.model ? modelLabel(profile, profile.model) : hostOf(profile.baseUrl)}</span>
              </span>
              {profile.source !== "manual" ? <span className="cm-tag">{SOURCE_LABELS[profile.source] ?? profile.source}</span> : null}
            </button>
          ))}
          {draft && !draft.id ? (
            <div className="cm-item" aria-current="true">
              <span className="cm-radio" />
              <span className="cm-item-text"><span className="cm-item-name">New Upstream</span><span className="cm-item-sub">Unsaved</span></span>
            </div>
          ) : null}
          {!book.profiles.length && !draft ? <p className="cm-empty">No upstreams yet. Add manually, or import from cc-switch / Magpie / Codex++.</p> : null}
        </nav>
        <div className="cm-side-foot">
          <button className="cm-btn cm-btn-quiet" onClick={addProfile} type="button"><Plus />Add</button>
          <ImportMenu disabled={busy !== null} onPick={(source) => void bookCall("import", "deskdown_import", { source }, selectedId)} />
        </div>
      </aside>

      <main className="cm-main">
        <header className="cm-toolbar" data-tauri-drag-region>
          <span className="cm-dot" data-tone={state.tone} />
          <div className="cm-toolbar-text" data-tauri-drag-region>
            <strong>{state.text}</strong>
            <span>
              {!status?.supported && status ? "Official Claude Desktop required first" : active ? `${active.name} · ${active.model ? modelLabel(active, active.model) : "No model selected"}` : "No upstream"}
            </span>
          </div>
          <div className="cm-toolbar-actions">
            <button aria-label="Re-detect" className="cm-icon" disabled={busy !== null} onClick={() => void run("detect", refreshStatus)} title="Re-detect" type="button">
              <RefreshCw className={busy === "detect" ? "cm-spin" : undefined} />
            </button>
            {showStop ? (
              <button className="cm-btn" disabled={busy !== null} onClick={() => void claudeAction("stop")} type="button"><Square />Stop</button>
            ) : null}
            <button className="cm-btn cm-btn-primary" disabled={busy !== null || !canLaunch || Boolean(status?.claudeRunning)} onClick={() => void claudeAction("launch")} title={status?.claudeRunning ? "Fully quit Claude first" : undefined} type="button">
              {busy === "launch" ? <Loader2 className="cm-spin" /> : <Play />}Launch Claude
            </button>
          </div>
        </header>

        {steps.length || status?.needsRestart || status?.warning ? (
          <div className="cm-banners">
            {steps.map((step) => (
              <div className="cm-banner" key={step.id}>
                <span>{step.text}</span>
                {step.id === "trust" ? (
                  <button className="cm-btn cm-btn-small" disabled={busy !== null} onClick={() => void claudeAction("trust")} type="button"><ShieldCheck />Trust Certificate</button>
                ) : step.id === "upstream" && !draft ? (
                  <button className="cm-btn cm-btn-small" onClick={addProfile} type="button"><Plus />Add Upstream</button>
                ) : null}
              </div>
            ))}
            {status?.needsRestart ? <div className="cm-banner" data-tone="info"><span>Settings changed: fully quit Claude then relaunch to apply.</span></div> : null}
            {status?.warning && !steps.length ? <div className="cm-banner"><span>{status.warning}</span></div> : null}
          </div>
        ) : null}

        <div className="cm-tabs" role="tablist">
          <button aria-selected={tab === "upstream"} onClick={() => setTab("upstream")} role="tab" type="button">Upstreams</button>
          <button aria-selected={tab === "claude"} onClick={() => setTab("claude")} role="tab" type="button">Claude Settings</button>
        </div>

        <section className="cm-body">
          {tab === "upstream" ? (
            draft ? (
              <UpstreamEditor
                active={draft.id !== "" && draft.id === book.activeId}
                busy={busy}
                dirty={dirty}
                draft={draft}
                onActivate={() => void bookCall("activate", "deskdown_set_active", { id: draft.id }, draft.id)}
                onChange={setDraft}
                onDelete={() => void bookCall("delete", "deskdown_delete_profile", { id: draft.id })}
                onDiscard={() => {
                  const saved = book.profiles.find((profile) => profile.id === draft.id);
                  if (saved) setDraft({ ...saved });
                  else acceptBook({ ...book, status: "ok", message: "" });
                }}
                onFetchModels={() => void fetchModels()}
                onSave={(activate) => void saveDraft(activate)}
              />
            ) : (
              <div className="cm-placeholder">
                <p>Select an upstream, or create a new one.</p>
                <button className="cm-btn" onClick={addProfile} type="button"><Plus />Add Upstream</button>
              </div>
            )
          ) : (
            <ClaudeSettings
              busy={busy}
              onAction={(action) => void claudeAction(action)}
              onConfigure={(patch) => void configure(patch)}
              onVoice={(enabled) => void setVoice(enabled)}
              status={status}
              update={update}
              version={version}
              onCheckUpdate={() => void runUpdateCheck()}
              onInstallUpdate={() => void runInstall()}
            />
          )}
        </section>

        <footer className="cm-status">
          <span>{status?.port ? `Local service · Port ${status.port}` : "Local service not running"}</span>
          {update.phase === "available" ? (
            <button className="cm-update-chip" onClick={() => void runInstall()} type="button">New version {update.version}, click to update</button>
          ) : update.phase === "downloading" ? (
            <span className="cm-update-chip">Updating to {update.version}{update.percent != null ? ` · ${update.percent}%` : "…"}</span>
          ) : null}
          {notice ? (
            <span className="cm-notice" data-tone={notice.tone} role={notice.tone === "error" ? "alert" : "status"}>
              {notice.text}
              <button aria-label="Dismiss" onClick={() => setNotice(null)} type="button"><X /></button>
            </span>
          ) : null}
        </footer>
      </main>
    </div>
  );
}

function ImportMenu({ disabled, onPick }: { disabled: boolean; onPick: (source: string) => void }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  return (
    <div className="cm-menu-anchor" ref={ref}>
      <button aria-expanded={open} className="cm-btn cm-btn-quiet" disabled={disabled} onClick={() => setOpen((value) => !value)} type="button">
        <Download />Import<ChevronDown className="cm-chevron" />
      </button>
      {open ? (
        <div className="cm-menu" role="menu">
          {IMPORT_SOURCES.map((source) => (
            <button key={source.id} onClick={() => { setOpen(false); onPick(source.id); }} role="menuitem" type="button">
              <strong>{source.label}</strong>
              <span>{source.detail}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="cm-field">
      <span className="cm-field-label">{label}</span>
      <span className="cm-field-control">{children}</span>
      {hint ? <span className="cm-field-hint">{hint}</span> : null}
    </label>
  );
}

function UpstreamEditor({ draft, active, dirty, busy, onChange, onSave, onActivate, onDelete, onDiscard, onFetchModels }: {
  draft: UpstreamProfile;
  active: boolean;
  dirty: boolean;
  busy: string | null;
  onChange: (next: UpstreamProfile) => void;
  onSave: (activate: boolean) => void;
  onActivate: () => void;
  onDelete: () => void;
  onDiscard: () => void;
  onFetchModels: () => void;
}) {
  const [showKey, setShowKey] = useState(false);
  const [newModel, setNewModel] = useState("");
  const [editConnection, setEditConnection] = useState(!draft.id || !draft.baseUrl);
  useEffect(() => setEditConnection(!draft.id || !draft.baseUrl), [draft.id]);
  const set = (patch: Partial<UpstreamProfile>) => onChange({ ...draft, ...patch });
  const preset = WINDOW_PRESETS.find((item) => item.value === draft.contextWindow);
  const options = draft.model && !draft.models.includes(draft.model) ? [draft.model, ...draft.models] : draft.models;
  const addModel = () => {
    const model = newModel.trim();
    if (!model) return;
    set({ models: normalizeModels([...draft.models, model]), model: draft.model || model });
    setNewModel("");
  };
  return (
    <div className="cm-editor">
      <div className="cm-editor-head">
        <input aria-label="Name" className="cm-title-input" onChange={(event) => set({ name: event.currentTarget.value })} placeholder={hostOf(draft.baseUrl) || "New Upstream"} value={draft.name} />
        {active ? <span className="cm-tag" data-tone="ok"><Check />Active</span> : null}
      </div>

      <section className="cm-card">
        <div className="cm-card-label">Connection</div>
        {editConnection ? (
          <div className="cm-form">
            <Field label="Base URL">
              <input autoComplete="off" onChange={(event) => set({ baseUrl: event.currentTarget.value })} placeholder="https://api.example.com" spellCheck={false} value={draft.baseUrl} />
            </Field>
            <Field label="Key">
              <input autoComplete="off" onChange={(event) => set({ apiKey: event.currentTarget.value })} placeholder="sk-…" spellCheck={false} type={showKey ? "text" : "password"} value={draft.apiKey} />
              <button aria-label={showKey ? "Hide Key" : "Show Key"} className="cm-icon" onClick={() => setShowKey((value) => !value)} type="button">{showKey ? <EyeOff /> : <Eye />}</button>
            </Field>
          </div>
        ) : (
          <div className="cm-card-line">
            <span className="cm-mono cm-ellipsis" title={draft.baseUrl}>{draft.baseUrl}</span>
            {draft.source !== "manual" ? <span className="cm-tag">From {SOURCE_LABELS[draft.source] ?? draft.source}</span> : null}
            <button className="cm-link" onClick={() => setEditConnection(true)} type="button">Edit</button>
          </div>
        )}
      </section>

      <section className="cm-card">
        <div className="cm-card-label">Model</div>
        <div className="cm-card-line">
          {options.length ? (
            <select aria-label="Default Model" className="cm-grow" onChange={(event) => set({ model: event.currentTarget.value })} value={draft.model}>
              {!draft.model ? <option value="">Select a default model</option> : null}
              {options.map((model) => <option key={model} value={model}>{modelLabel(draft, model)}</option>)}
            </select>
          ) : (
            <input aria-label="Default Model" className="cm-grow" onChange={(event) => set({ model: event.currentTarget.value })} placeholder="Fetch models first, or type model name" spellCheck={false} value={draft.model} />
          )}
          <button aria-label="Fetch Models" className="cm-icon" disabled={busy !== null || !draft.baseUrl.trim()} onClick={onFetchModels} title="Fetch model list from upstream" type="button">
            {busy === "models" ? <Loader2 className="cm-spin" /> : <RefreshCw />}
          </button>
        </div>
        {draft.models.length > 1 ? <p className="cm-field-hint">In Claude you can also switch among {draft.models.length - 1} other models.</p> : null}
      </section>

      <details className="cm-more">
        <summary>Advanced</summary>
        <div className="cm-form">
          <Field hint="Small tasks like title generation; empty = same as default model" label="Fast Model">
            <select onChange={(event) => set({ smallModel: event.currentTarget.value })} value={draft.smallModel}>
              <option value="">Same as default model</option>
              {options.map((model) => <option key={model} value={model}>{modelLabel(draft, model)}</option>)}
            </select>
          </Field>
          <Field label="Context Window">
            <select onChange={(event) => set({ contextWindow: event.currentTarget.value === "" ? null : Number(event.currentTarget.value) })} value={draft.contextWindow == null ? "" : String(draft.contextWindow)}>
              {WINDOW_PRESETS.map((item) => <option key={item.label} value={item.value == null ? "" : String(item.value)}>{item.label}</option>)}
              {!preset && draft.contextWindow ? <option value={String(draft.contextWindow)}>{formatWindow(draft.contextWindow)}</option> : null}
            </select>
          </Field>
          <div className="cm-field">
            <span className="cm-field-label">Available Models</span>
            <div className="cm-chips">
              {draft.models.map((model) => (
                <span className="cm-chip" data-primary={model === draft.model || undefined} key={model}>
                  <button onClick={() => set({ model })} title={`${model} (click to set as default)`} type="button">{modelLabel(draft, model)}</button>
                  <button aria-label={`Remove ${model}`} onClick={() => set({ models: draft.models.filter((item) => item !== model), model: draft.model === model ? "" : draft.model })} type="button"><X /></button>
                </span>
              ))}
              <input
                aria-label="Add Model"
                className="cm-chip-input"
                onChange={(event) => setNewModel(event.currentTarget.value)}
                onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); addModel(); } }}
                placeholder="Add model, Enter"
                spellCheck={false}
                value={newModel}
              />
            </div>
          </div>
          {draft.id ? (
            <div className="cm-row-actions">
              <button className="cm-btn cm-btn-small cm-btn-danger" disabled={busy !== null} onClick={onDelete} type="button"><Trash2 />Delete this upstream</button>
            </div>
          ) : null}
        </div>
      </details>

      {dirty || (draft.id && !active) ? (
        <div className="cm-editor-foot">
          <span className="cm-spacer" />
          {dirty ? <button className="cm-btn cm-btn-quiet" disabled={busy !== null} onClick={onDiscard} type="button">Discard changes</button> : null}
          {!dirty && draft.id && !active ? <button className="cm-btn cm-btn-primary" disabled={busy !== null} onClick={onActivate} type="button">Use this one</button> : null}
          {dirty ? <button className="cm-btn" disabled={busy !== null} onClick={() => onSave(false)} type="button">Save</button> : null}
          {dirty ? <button className="cm-btn cm-btn-primary" disabled={busy !== null} onClick={() => onSave(true)} type="button">Save and use</button> : null}
        </div>
      ) : null}
    </div>
  );
}

function Toggle({ label, detail, hint, checked, disabled, onChange }: { label: string; detail?: string; hint?: string; checked: boolean; disabled?: boolean; onChange: (next: boolean) => void }) {
  return (
    <label className="cm-toggle-row" title={detail}>
      <span className="cm-toggle-text">
        <span>{label}</span>
        {hint ? <span className="cm-field-hint">{hint}</span> : null}
      </span>
      <input checked={checked} disabled={disabled} onChange={(event) => onChange(event.currentTarget.checked)} type="checkbox" />
      <span aria-hidden="true" className="cm-switch" />
    </label>
  );
}

type ModelState = { modelReady: boolean; phase?: string; completedBytes?: number; totalBytes?: number; message?: string };

function VoiceModel({ onReady }: { onReady: () => void }) {
  const [state, setState] = useState<ModelState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try { setState(await invoke<ModelState>("dictation_local_model_status")); } catch (cause) { setError(errorText(cause)); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  const downloading = state?.phase === "downloading";
  useEffect(() => {
    if (!downloading) return;
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [downloading, refresh]);
  const download = async () => {
    setError(null);
    setState((current) => ({ ...(current ?? { modelReady: false }), phase: "downloading", completedBytes: 0 }));
    try {
      const result = await invoke<CommandResult<ModelState>>("download_dictation_local_model");
      if (!isOk(result)) setError(result.message);
      else onReady();
    } catch (cause) {
      setError(errorText(cause));
    }
    await refresh();
  };
  if (!state) return null;
  const percent = state.totalBytes ? Math.min(100, Math.round(((state.completedBytes ?? 0) / state.totalBytes) * 100)) : 0;
  return (
    <div className="cm-row-actions cm-voice-model">
      <span className="cm-field-hint">
        {state.modelReady ? "Local voice model (SenseVoice) ready." : downloading ? `Downloading local voice model… ${percent}%` : "Local voice model (SenseVoice, ~230MB) needs to be downloaded first."}
        {error ? ` ${error}` : ""}
      </span>
      {!state.modelReady && !downloading ? <button className="cm-btn cm-btn-small" onClick={() => void download()} type="button"><Download />Download Model</button> : null}
      {downloading ? <Loader2 className="cm-spin" /> : null}
    </div>
  );
}

function ClaudeSettings({ status, busy, onConfigure, onAction, onVoice, update, version, onCheckUpdate, onInstallUpdate }: {
  status: ClaudeStatus | null;
  busy: string | null;
  onConfigure: (patch: Partial<ClaudeConfig>) => void;
  onAction: (action: ClaudeAction) => void;
  onVoice: (enabled: boolean) => void;
  update: UpdateState;
  version: string;
  onCheckUpdate: () => void;
  onInstallUpdate: () => void;
}) {
  if (!status) return <div className="cm-placeholder"><Loader2 className="cm-spin" /></div>;
  const config = status.config;
  const disabled = busy !== null || !status.supported;
  const text = (key: "displayName" | "email", placeholder: string, label: string, type = "text") => (
    <Field label={label}>
      <input
        defaultValue={config[key] ?? ""}
        disabled={disabled}
        key={config[key] ?? ""}
        onBlur={(event) => { if (event.currentTarget.value !== (config[key] ?? "")) onConfigure({ [key]: event.currentTarget.value }); }}
        onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }}
        placeholder={placeholder}
        type={type}
      />
    </Field>
  );
  return (
    <div className="cm-settings">
      <section className="cm-card">
        <Field
          hint={config.threePMode
            ? "Claude's built-in new UI (Claude's third-party mode): updates with Claude, Chat/Cowork/Code call local service → upstream. No account features, no cert trust needed, English only for now."
            : "Recorded UI from claude.ai, simulates a claude.ai account: fullest features, includes English"}
          label="Interface"
        >
          <select disabled={disabled} onChange={(event) => onConfigure({ threePMode: event.currentTarget.value === "local" })} value={config.threePMode ? "local" : "recorded"}>
            <option value="recorded">Recorded (default)</option>
            <option value="local">Local (third-party mode)</option>
          </select>
        </Field>
        {!config.threePMode && (status.templates?.length ?? 0) > 1 ? (
          <Field hint="Claude.ai version at recording time (API data + page shell). Newer versions = newer features; switch back if new version has issues. Frontend code downloads from Anthropic per selected version" label="Version">
            <select
              disabled={disabled}
              onChange={(event) => onConfigure({ template: event.currentTarget.value })}
              value={status.templates?.some((template) => template.build === config.template) ? config.template : ""}
            >
              <option value="">Latest ({templateLabel(status.templates![0])})</option>
              {status.templates!.map((template) => <option key={template.build} value={template.build}>{templateLabel(template)}</option>)}
            </select>
          </Field>
        ) : null}
        {config.threePMode ? null : (
          <Toggle checked={config.hubHome ?? false} detail="Claude is experimentally testing this homepage layout: Chat and Cowork merged into one input, no 'Chat | Cowork' toggle. Off = classic homepage with toggle." disabled={disabled} label="Merged Homepage (experimental)" onChange={(next) => onConfigure({ hubHome: next })} />
        )}
        <Toggle
          checked={config.localization}
          detail={config.threePMode
            ? "Local mode unsupported: this UI only reads languages bundled in Claude installer, which lacks English (menus etc. follow system language)"
            : undefined}
          disabled={disabled || config.threePMode}
          label="English UI"
          onChange={(next) => onConfigure({ localization: next })}
        />
        <Toggle
          checked={config.voiceInput}
          detail={config.threePMode
            ? "Local mode unsupported for now: third-party mode dictation only hooks into Claude's gateway SSO; local service not yet implemented"
            : "Microphone in Claude input box uses local SenseVoice transcription; audio never leaves your machine. Restart Claude after change to apply."}
          disabled={disabled || config.threePMode}
          label="Voice Input"
          onChange={onVoice}
        />
        {config.voiceInput && !config.threePMode && !status.dictationReady ? <VoiceModel onReady={() => onAction("prepare")} /> : null}
        <Toggle
          checked={config.importCliSessions ?? true}
          detail="Before launching Claude, add sessions created in terminal, BaoCode, and other CLIs to the sidebar (already listed or previously deleted won't be re-added)."
          disabled={disabled}
          label="Show CLI Sessions"
          onChange={(next) => onConfigure({ importCliSessions: next })}
        />
      </section>

      <details className="cm-more">
        <summary>Sessions</summary>
        <Toggle
          checked={config.codeShareHome ?? true}
          detail="On: Code tab and CLI share ~/.claude (sessions, MCP, plugins are the same; ~/.claude/settings.json API address takes priority). Off: separate directory, requests use the upstream selected here."
          disabled={disabled}
          label="Share ~/.claude with CLI"
          onChange={(next) => onConfigure({ codeShareHome: next })}
        />
        <Toggle checked={config.sessionSync === true} detail="Sync Code sessions between 3p mode and full-feature mode on launch" disabled={disabled} label="Sync 3p mode sessions" onChange={(next) => onConfigure({ sessionSync: next })} />
        <div className="cm-row-actions">
          <button className="cm-btn cm-btn-small" disabled={disabled || status.claudeRunning} onClick={() => onAction("sync_sessions")} type="button">Sync Now</button>
        </div>
      </details>

      <details className="cm-more">
        <summary>Account Display</summary>
        <p className="cm-field-hint">Only changes what shows in Claude; takes effect on next launch.</p>
        {text("displayName", "Local User", "Username")}
        {text("email", "local@claude.local", "Email", "email")}
        <Field label="Plan">
          <select disabled={disabled} onChange={(event) => onConfigure({ plan: event.currentTarget.value })} value={config.plan || "ultra"}>
            {["free", "pro", "max", "ultra"].map((plan) => <option key={plan} value={plan}>{plan[0].toUpperCase() + plan.slice(1)}</option>)}
          </select>
        </Field>
      </details>

      <details className="cm-more">
        <summary>Advanced</summary>
        <Toggle checked={config.cowork ?? true} detail="Off falls back to Claude's own switch; newer Claude versions often only keep Chat/Code. System requirements (Windows Store version, virtualization) still checked by Claude." disabled={disabled} label="Cowork" onChange={(next) => onConfigure({ cowork: next })} />
        {(config.cowork ?? true) && status.caMachineSupported && !status.caMachineTrusted ? (
          <div className="cm-row-actions" title="Cowork VM runs as LocalSystem, only reads machine root cert.">
            <span className="cm-field-hint cm-grow">Cowork VM needs machine-level root cert (requires admin).</span>
            <button className="cm-btn cm-btn-small" disabled={busy !== null} onClick={() => onAction("trust_machine")} type="button"><ShieldCheck />Install System Cert</button>
          </div>
        ) : null}
        <Toggle checked={config.hideUnroutedModels ?? true} detail="Claude's built-in Opus/Sonnet etc. entries replaced with default model" disabled={disabled} label="Only Show Upstream Models" onChange={(next) => onConfigure({ hideUnroutedModels: next })} />
        <Toggle checked={config.computerUse} detail="Requires system Spotlight working; otherwise Claude may hang" disabled={disabled} label="Computer Control" onChange={(next) => onConfigure({ computerUse: next })} />
        <Toggle checked={config.backgroundMode} disabled={disabled} label="Background Mode" onChange={(next) => onConfigure(next ? { backgroundMode: true } : { backgroundMode: false, forceBackgroundMode: false })} />
        <Toggle checked={config.forceBackgroundMode ?? false} detail="Claude on newer macOS prompts 'Background mode not supported'; enabling bypasses system version check, not verified by Claude, may be unstable" disabled={disabled} label="Enable Background Mode on New macOS" onChange={(next) => onConfigure({ forceBackgroundMode: next })} />
        <div className="cm-row-actions" title="Local service uses it to intercept Claude's requests; only valid on this machine.">
          <span className="cm-field-hint cm-grow">Local Certificate: {status.caTrusted ? "Trusted" : status.caReady ? "Not Trusted" : "Not Ready"}</span>
          {!status.caReady ? <button className="cm-btn cm-btn-small" disabled={busy !== null} onClick={() => onAction("prepare")} type="button">Prepare</button> : null}
          {status.caTrusted
            ? <button className="cm-btn cm-btn-small" disabled={busy !== null} onClick={() => onAction("untrust")} type="button">Untrust</button>
            : <button className="cm-btn cm-btn-small" disabled={busy !== null} onClick={() => onAction("trust")} type="button"><ShieldCheck />Trust</button>}
        </div>
      </details>

      <div className="cm-about">
        <span>ClaudeDeskDown {version || ""}{status.version ? ` · Claude ${status.version}` : ""}</span>
        {update.phase === "available" ? (
          <button className="cm-link" onClick={onInstallUpdate} type="button">Update to {update.version}</button>
        ) : (
          <button className="cm-link" disabled={update.phase === "checking" || update.phase === "downloading"} onClick={onCheckUpdate} type="button">
            {update.phase === "checking" ? "Checking…" : update.phase === "latest" ? "Up to date" : "Check for Updates"}
          </button>
        )}
      </div>
    </div>
  );
}