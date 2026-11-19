import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { claudeState, emptyProfile, formatWindow, hostOf, normalizeModels, profileProblem, setupSteps, templateLabel, type ClaudeStatus } from "./model.ts";

const status = (patch: Partial<ClaudeStatus> = {}): ClaudeStatus => ({
  supported: true, sourceApp: "", version: "1.0", claudeRunning: false, running: false, connected: false,
  caReady: true, caTrusted: true, caMachineSupported: true, caMachineTrusted: false, nodeAvailable: true, dictationReady: true, models: [], needsRestart: false,
  config: { enabled: true, model: "", localization: true, voiceInput: false, computerUse: false, backgroundMode: false },
  ...patch,
});

test("Upstream form: valid Base URL and default model required, no /v1/messages", () => {
  const profile = { ...emptyProfile(), baseUrl: "https://api.example.com", model: "glm-5" };
  assert.equal(profileProblem(profile), null);
  assert.equal(profileProblem({ ...profile, baseUrl: "" }), "Enter a Base URL");
  assert.equal(profileProblem({ ...profile, baseUrl: "api.example.com" }), "Base URL is not a valid address");
  assert.equal(profileProblem({ ...profile, baseUrl: "ftp://x" }), "Base URL must start with http:// or https://");
  assert.match(profileProblem({ ...profile, baseUrl: "https://x/v1/messages" }) ?? "", /don't include/);
  assert.equal(profileProblem({ ...profile, model: " " }), "Pick a default model");
  assert.equal(hostOf("http://127.0.0.1:3425"), "127.0.0.1:3425");
});

test("Model list deduplication, default model first", () => {
  assert.deepEqual(normalizeModels(["b", " a ", "b", ""], "a"), ["a", "b"]);
});

test("Status text: connected, running, externally opened, not started", () => {
  assert.deepEqual(claudeState(null, null), { text: "Detecting Claude…", tone: "busy" });
  assert.equal(claudeState(status({ connected: true }), null).tone, "ok");
  assert.equal(claudeState(status({ claudeRunning: true }), null).tone, "warn");
  assert.equal(claudeState(status(), null).text, "Not started");
  assert.equal(claudeState(status(), "launch").text, "Launching…");
  assert.equal(claudeState(status({ supported: false }), null).text, "Claude Desktop not found");
});

test("Pre-launch checklist lists missing steps in order", () => {
  const ready = { ...emptyProfile(), baseUrl: "https://a.example", model: "m" };
  assert.deepEqual(setupSteps(status(), ready), []);
  assert.deepEqual(setupSteps(status({ caTrusted: false, nodeAvailable: false }), null).map((step) => step.id), ["upstream", "node", "trust"]);
  // Third-party mode doesn't proxy: no cert trust required.
  assert.deepEqual(setupSteps(status({ caTrusted: false, config: { ...status().config, threePMode: true } }), null).map((step) => step.id), ["upstream"]);
});

test("Window display", () => {
  assert.equal(formatWindow(null), "200k");
  assert.equal(formatWindow(1_000_000), "1M");
  assert.equal(formatWindow(1_100_000), "1.1M");
  assert.equal(formatWindow(400_000), "400k");
});

test("All commands called from frontend are registered in backend", () => {
  const app = readFileSync(new URL("./ClaudeDeskDownApp.tsx", import.meta.url), "utf8");
  const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
  // Some commands passed directly to invoke, some via bookCall: collect by literal.
  const called = new Set([...app.matchAll(/"(deskdown_[a-z_]+|claude_local_[a-z_]+)"/g)].map((match) => match[1]));
  assert.ok(called.size >= 9, [...called].join(","));
  for (const command of called) assert.ok(new RegExp(`::${command},`).test(lib), `${command} not registered`);
});

test("Recorded template versions are labelled by build date and id", () => {
  const label = templateLabel({ build: "a4ced7f85b", builtAt: 1794603257 });
  assert.match(label, /^2026-11-1[34] · a4ced7f85b$/);
  assert.equal(templateLabel({ build: "x1", builtAt: null }), "x1");
});
