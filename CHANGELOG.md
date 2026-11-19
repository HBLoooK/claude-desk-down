# Changelog

## 0.1.0-local - 2026-11-19

- Model list: retry without `anthropic-version` when the first probe returns an empty list (NVIDIA NIM).
- Chat: fall back to `/v1/chat/completions` when the upstream has no Anthropic `/v1/messages` route.
- Rendering: normalize `files`/`attachments`/`sync_sources` to arrays and fill missing conversation fields so newer Claude Desktop versions render replies.
- Language: force `en-US` when localization is disabled.

## 0.1.0 - 2026-11-17

Initial release of ClaudeDeskDown — English fork of ClaudeMom.

- No Claude account required — use your own API Key in login mode
- Full Claude Desktop features: Chat / Code / Cowork, Computer Control, Voice Input
- Multiple upstreams (Base URL + Key), switch anytime, fetch model lists
- One-click import from cc-switch; use Magpie/local gateway as upstream
- English UI
- Local offline dictation (SenseVoice), audio never leaves your machine
- Computer Control / Background Mode toggle for newer macOS
- Code tab and CLI share `~/.claude`
- Auto-update; macOS signed + notarized