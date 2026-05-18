# GUI Desktop Console

The GUI is an EGUI (Rust native) desktop application located in the `gui/` directory.
It provides backend monitoring, multi-session chat, skills management, and settings editing,
so operations and integration debugging don't have to stay in the terminal.

## Architecture Overview

The GUI is a Rust native desktop app built with the EGUI framework (based on `eframe`/`egui`).
It communicates with the backend via ACP+HTTP JSON-RPC and manages the backend process lifecycle.

The GUI stores and uses three core values:

- backend executable path
- working directory
- runtime config file inside the working directory

The backend process is started from the configured working directory. The GUI expects `config.toml` to live there.

## Feature Tabs

### Monitor Tab
- Backend health: auto-polls `/health` endpoint
- AI provider status: real-time provider connection status
- Live metrics: request count, latency, error rate

### Chat Tab
- Multi-session management: create, switch, delete sessions
- Multi-model support: each session can select a different AI model
- Phase selection: coding / review / debug / test / deploy
- Mode switching: Ask / Plan / Edit / Safeguard / Full Auto
- File attachments: upload files as chat context
- Dynamic send button: changes based on AI status (loading / ready / error)
- **Risk Decision panel**: displays risk assessment, mitigation strategy, and review requirements for high-risk AI responses (see dedicated section below)

### Skills Tab
- Create and import AI skills
- Built-in `skill-creator`: lets AI define new skills autonomously
- Skill list management: enable, disable, delete

### Prompts Tab

Browse, search, create, edit, and delete prompt templates across 12 industry categories.
See the [Prompts System](prompts.md) document for full details.

- **Browse by category** — filter templates by 12 industry categories (Software Development, Writing,
  Academic Research, Business Analysis, Marketing, Legal, Medical, Education, Finance, Data Science, Design, System Operations)
- **Search** — keyword search across template titles and content
- **Custom templates** — create your own templates with `{{variable}}` placeholders
- **Insert to Chat** — click the **Insert to Chat** button on any template card to insert the template
  content into the Chat input box
- **Chat `/` commands** — type `/` in the Chat input box to trigger command completion;
  type a template ID and press Enter to expand the template directly

> For details on the template system, see the [Prompts System](prompts.md).

### Chat Tab — Risk Decision & Safeguard Mode

The GUI displays a **Risk Decision panel** on AI responses when the backend's governance layer detects potentially high-risk content. This feature is part of the Safeguard mode and multi-model voting system.

**What triggers it:** The backend analyzes user messages against domain keywords (medical, legal, financial, security, etc.) and decision-related keywords. When the risk score exceeds the threshold, the backend activates one or more mitigation strategies:

- **Multi-model vote**: Sends the request to multiple models concurrently and compares outputs
- **Multi-agent vote**: Routes the request to different AI agents and aggregates results
- **Escalation**: Uses more capable (and expensive) models for final judgment
- **Review required**: Flags the response for human review

**What the GUI shows:**
- Risk state: **High Risk** or **Normal**
- Strategy used: e.g. "multi_model_vote", "multi_agent_vote", "escalation"
- Review requirement indication
- Specific risk reasons (up to 4 reasons displayed inline)

The panel adapts theme-wise: high-risk topics get a warm orange/red background, normal risk gets a subtle green tint — both respecting dark/light mode correctly.

### Settings Tab
- **Provider Management**: dynamic environment variable injection (all 34+ providers), no longer hardcoded to 8
- **Config Editor**: manages `gui_config.json` with JSON syntax validation
- **Theme Selection**: 6 visual themes (Minimal / Chinese-Classic / Wuxia / Landscape / Hello Kitty / Dark)
- **Language Switching**: English, Simplified Chinese, Traditional Chinese
- **Feature Toggles**: enable/disable GUI features (including **Prompts** toggle that controls the Prompts Tab visibility and related RPC/MCP interfaces)

## Development and build commands

From `gui/`:

```bash
# Run in development mode
cargo run

# Build release
cargo build --release

# From project root
cargo run --manifest-path gui/Cargo.toml
```

## Linking the backend

The GUI can auto-discover the backend executable. When auto-link succeeds it uses the executable's parent directory as the working directory and stores logs as `go-on.log` there.

If auto-discovery does not succeed, configure manually:

1. set the backend executable path
2. set the working directory
3. ensure `config.toml` exists in that directory

## Key Management

The GUI uses dual storage for secrets:

- **System keyring**: priority storage using OS-level key management (Linux Secret Service, macOS Keychain, Windows Credential Manager)
- **Config file**: backup and portable fallback

API keys no longer need to be written to `.env.goon` — all keys are injected dynamically through the GUI's Provider management panel.

## Runtime process behavior

When the GUI starts the backend process, it launches the configured executable from the working directory and writes stdout and stderr to `go-on.log`.

**Auto-restart**: if the backend crashes, the GUI automatically restarts it after a 3-second cool-down.

Because startup depends on the working directory, the most common operator mistake is pointing the GUI at the correct binary but the wrong directory.

## Health and integration probes

The GUI probes:

- ACP or runtime health at `/health`
- OpenAI-compatible models at `/v1/models`

The integration status page interprets those probes for:

- Zed ACP or A2A over HTTP
- Zed MCP or model-provider style `/v1`
- VS Code addon runtime health

## Recommended backend modes for GUI usage

- `adaptive`: best default when the GUI is used alongside Zed or VS Code.
- `acp_http`: good when you want HTTP-only ACP behavior.
- `mcp_http`: useful when your main concern is `/v1` provider compatibility.

The GUI itself can still manage the backend executable even when a different mode is selected; the mode choice mostly affects what external clients can do afterward.

## Recommended operator flow

1. Build the backend: `cargo build`
2. Initialize backend (first time): `cargo run -- --init`
3. Build the GUI: `cargo build --manifest-path gui/Cargo.toml`
4. Launch the GUI: `cargo run --manifest-path gui/Cargo.toml`
5. Use auto-link or manual executable-path configuration
6. Confirm the working directory contains `config.toml`
7. Configure API keys in Provider management (auto-stored in system keyring)
8. Start the backend
9. Check health and integration status

## Troubleshooting

- If startup fails with a file error, re-check the executable path first.
- If startup succeeds but probes fail, re-check protocol mode and provider readiness.
- If the GUI shows health but editors still fail, compare the editor transport contract against the current runtime mode.
- GUI-specific issues: check if `gui_config.json` is corrupted; delete it to reset if necessary.
