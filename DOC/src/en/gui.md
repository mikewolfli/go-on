# GUI Console

The GUI is a Tauri desktop console around the backend. It is designed for operators who want process control, integration status, configuration editing, and local environment management without staying in a terminal.

## What the GUI needs

The GUI stores and uses three core values:

- backend executable path
- working directory
- runtime config file inside the working directory

The backend process is started from the configured working directory. The GUI expects `config.toml` to live there.

## Development and build commands

From `GUI/`:

```bash
npm install
npm run dev
```

Production build:

```bash
npm run build
```

Desktop packaging and native shell:

```bash
npm run tauri dev
npm run tauri build
```

## Linking the backend

The GUI can auto-discover the backend executable. When auto-link succeeds it uses the executable's parent directory as the working directory and stores logs as `go-on.log` there.

If auto-discovery does not succeed, configure manually:

1. set the backend executable path
2. set the working directory
3. ensure `config.toml` exists in that directory

## Config and environment behavior

The GUI manages:

- `config.toml`
- `config.toml.autopilot-adaptive` as the reset template source
- `.env.goon` for persisted environment variables

That means a practical GUI-based onboarding sequence is:

1. link the backend executable
2. restore default config if needed
3. save provider API keys through the GUI
4. start the backend process
5. verify integration probes

## Runtime process behavior

When the GUI starts the backend process, it launches the configured executable from the working directory and writes stdout and stderr to `go-on.log`.

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

1. Build the backend.
2. Launch the GUI.
3. Use auto-link or manual executable-path configuration.
4. Confirm the working directory contains `config.toml`.
5. Save API keys into `.env.goon` if needed.
6. Start the backend.
7. Check health and integration status.

## Troubleshooting

- If startup fails with a file error, re-check the executable path first.
- If startup succeeds but probes fail, re-check protocol mode and provider readiness.
- If the GUI shows health but editors still fail, compare the editor transport contract against the current runtime mode.