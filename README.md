# ClaudeDeskDown

**Use Claude Desktop in full login mode with your own API key. No Claude account required.**

ClaudeDeskDown is a small local service that makes Claude Desktop believe it is signed in. You get the complete client — Chat, Code, Cowork, Computer Control, Voice Input, English UI — while every model request goes to a base URL and key you control.

[Download the latest release](https://github.com/HBLoooK/claude-desk-down/releases/latest) · macOS (universal dmg) · Windows (exe)

## Why

Claude Desktop ships with two sign-in modes: an official account (full features) or an API key in "3p mode" (no account, but most feature entry points are hidden). ClaudeDeskDown adds a third path — no account, full features. It runs locally and turns your API key into "login mode": Claude Desktop sees a logged-in account and exposes the full UI, but requests route through your own upstream.

The Claude Desktop app itself is never patched or repackaged. ClaudeDeskDown does adjust some of Claude's own configuration while it runs and puts it back when you stop — see [What it changes on your machine](#what-it-changes-on-your-machine).

### Three access modes compared

| Feature | Official account | API key (3p) | ClaudeDeskDown |
|---------|------------------|--------------|----------------|
| Claude account required | Yes | No | **No** |
| Model source | Official | Your key | **Your key** |
| Chat / Code / Cowork | Yes | Partial | **Yes** |
| Computer Control / Voice Input | Yes | No | **Yes** |
| English UI | Yes | No | **Yes** |

## How it works

A local compatible service intercepts Claude Desktop's account and feature requests. Account info and feature flags report "logged in"; chat requests are forwarded to your configured upstream. The certificate is local-only — revoke trust at any time.

Base URLs work with or without a trailing `/v1`. If the upstream has no Anthropic `/v1/messages` route (NVIDIA NIM and other OpenAI-only gateways), the service falls back to `/v1/chat/completions` and translates requests and responses in both directions, including streaming and tool calls. Anthropic-compatible upstreams are forwarded unchanged.

## What it changes on your machine

ClaudeDeskDown never patches, repackages, or modifies the Claude Desktop application. It does change three things, and you should know what they are before installing:

| Change | Why | How it is undone |
|--------|-----|------------------|
| A local root CA trusted in your certificate store | Lets the local service terminate TLS for Claude's requests | "Revoke trust" removes it; the private key never leaves `<state>/claude-local` |
| `deploymentMode` set to `1p` in `claude_desktop_config.json` | Runs Claude in full-feature mode instead of third-party mode | Restored from `real-profile-switch.json` on stop |
| `Claude-3p/configLibrary` moved aside | Claude's third-party inference config would otherwise take precedence | Moved back on stop; only libraries this app wrote are deleted |

The config switch is recorded before it is applied and rolled back on stop; an interrupted switch is recovered on the next launch. If the app is force-killed or the state file is lost, your Claude config can be left in `1p` with the original library displaced — reinstall or restore from the backup file in that case. Official account login state is untouched, and Claude Desktop updates normally.

## Features

- **Login-mode experience** — Chat, Code, and Cowork all work. It is the original client, not a web wrapper.
- **Multiple upstreams** — Each with its own base URL and key (Anthropic-compatible, same semantics as `ANTHROPIC_BASE_URL`). Switch at any time and fetch model lists.
- **One-click import** — Import providers from cc-switch, or use Magpie's local gateway as an upstream.
- **English UI** — Full English interface.
- **Voice Input** — Local offline dictation with SenseVoice; audio never leaves your machine.
- **Computer Control / Background Mode** — On newer macOS, Claude reports "Background mode not supported"; an advanced-settings toggle bypasses the version check (off by default).
- **Shared `~/.claude`** — The Code tab and CLI share the same directory, so Claude Code sessions from the terminal appear in Desktop's sidebar.
- **Auto-update** — The macOS release is signed and notarized. Signing is done out of band: `signingIdentity` in `tauri.conf.json` is `"-"` (ad-hoc), so a local build is not notarized.

## Getting started

1. Install official Claude Desktop and Node.js 22+ (needs global `fetch` and `WebSocket`; the app probes for both and falls back to a bundled Node if yours is older).
2. Open ClaudeDeskDown and follow the prompts to trust the local certificate (local-only).
3. Add an upstream (or import from cc-switch) and select a default model.
4. Click **Launch Claude**. You are in the logged-in UI.

## Project layout

| Directory | Description |
|-----------|-------------|
| `crates/claude-local` | Core local compatible service (`runtime/`), certificates, profile switching, start/stop, dictation |
| `apps/claude-deskdown` | Desktop app (Tauri + React) |

## Development

```bash
cd apps/claude-deskdown
npm install
npm run check && npm test            # Frontend type check + unit tests
cargo test --workspace               # Rust, from repo root (needs rustc >= 1.90)
npx tauri dev                        # Run the app locally
npx tauri build                      # Windows installer (targets = ["nsis"])
```

On macOS, `bundle.targets` is trimmed to Windows for day-to-day development. Restore `["app", "dmg"]` in `apps/claude-deskdown/src-tauri/tauri.conf.json`, then run `npx tauri build --bundles app,dmg` (a signing identity is required for distribution).

Updater artifacts are opt-in (`createUpdaterArtifacts` is off so a plain build works). A minisign key pair has been generated at `~/.tauri/claude-deskdown-updater.key` (private — keep it secret and backed up) and `.key.pub` (already in `plugins.updater.pubkey`). To produce signed artifacts, set the private key and opt in:

```bash
export TAURI_SIGNING_PRIVATE_KEY=$HOME/.tauri/claude-deskdown-updater.key
npx tauri build --config '{"bundle":{"createUpdaterArtifacts":true}}'
```

Without a private key, Tauri aborts the release build after bundling.

Core Node tests:

```bash
cd crates/claude-local/runtime && node --test managed.test.mjs proxy.test.mjs
```

These require local test certificates (`scripts/gen-ca.sh`, needs bash + OpenSSL).

## Contributing

Issues and pull requests are welcome. Let's make Claude Desktop work better.

## License

AGPL-3.0-only
