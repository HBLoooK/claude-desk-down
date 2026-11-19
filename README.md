# ClaudeDeskDown

**No Claude account required! Turn your API Key into login mode — unlock full Claude Desktop features (localization / voice / computer control)**

Claude Desktop has always had two ways to use it:

- **Official account login**: Full features — Chat / Code / Cowork, Computer Control, Voice Input all available
- **API Key (3p mode)**: Works, but many feature entry points are simply missing

ClaudeDeskDown is a **new way to connect**: it runs locally and turns your API Key into **"login mode"**. Claude Desktop sees a logged-in account — UI and features are the full login version — but model requests route through your own Base URL + Key.

No Claude account needed. No modification to the original app. Official login state untouched. Official app updates normally.

Download: [Releases](https://github.com/HBLoooK/claude-desk-down/releases/latest) (macOS universal dmg / Windows exe)

## Three Access Modes Compared

| Feature | Official Account | API Key (3p mode) | **ClaudeDeskDown** |
|---------|------------------|-------------------|---------------------|
| Claude account required | Yes | No | **No** |
| Model source | Official | Your Key | **Your Key** |
| Chat / Code / Cowork | ✅ | Partial | **✅** |
| Computer Control / Voice Input | ✅ | ❌ | **✅** |
| English UI | ✅ | ❌ | **✅** |

## How It Works

A local compatible service runs on your machine, intercepting Claude Desktop's account and feature requests: account info and feature flags return "logged in", while chat requests forward to your configured upstream. Certificates only work locally — revoke trust anytime.

## Features

- **Login-mode experience**: Chat / Code / Cowork tabs all work — it's the original client, not a web wrapper
- **Multiple upstreams**: Each with its own Base URL + Key (Anthropic-compatible, same semantics as `ANTHROPIC_BASE_URL`), switch anytime, fetch model lists
- **One-click import**: cc-switch's Claude providers; also use Magpie's local gateway as upstream
- **English UI**: Full English interface
- **Voice Input**: Local offline dictation (SenseVoice), audio never leaves your machine
- **Computer Control / Background Mode**: On newer macOS, Claude prompts "Background mode not supported" — advanced settings have a toggle to bypass version restriction (off by default)
- **Code tab and CLI share `~/.claude`**: Terminal and other tools' Claude Code sessions appear in Desktop's sidebar
- **Auto-update**; macOS version signed + notarized, download and run

## Usage

1. Install official Claude Desktop and Node.js 22+
2. Open ClaudeDeskDown, follow prompts to trust local certificate (local-only)
3. Add upstream (or import from cc-switch), select default model
4. Click "Launch Claude" — you're in the logged-in UI

Welcome issues and PRs — let's make Claude Desktop work better!

## Structure

| Directory | Description |
|-----------|-------------|
| `crates/claude-local` | Core: local compatible service (`runtime/`), certificates, profile switching, start/stop, dictation |
| `apps/claude-deskdown` | ClaudeDeskDown desktop app (Tauri + React) |

## Development

```bash
cd apps/claude-deskdown
npm install
npm run check && npm test          # Frontend
cargo test --workspace             # Rust (from repo root)
npx tauri build --bundles app      # Build macOS .app
node scripts/relaunch-local.mjs    # Build and restart local instance (from repo root)
```

Core Node tests: `cd crates/claude-local/runtime && node --test managed.test.mjs proxy.test.mjs`  
(requires local test certs: `scripts/gen-ca.sh`)

## License

AGPL-3.0-only