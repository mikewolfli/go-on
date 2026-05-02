# macOS First Run Guide

This package keeps the same structure across platforms.
On macOS, Gatekeeper may block unsigned binaries or apps on first launch.

## Quick Start

From the extracted release folder:

```bash
./first-run.sh
```

This command is safe to run multiple times.

## What `first-run.sh` does

- On macOS:
  - Creates local trusted copies for backend/app when found
  - Removes quarantine attribute on the copies
  - Applies ad-hoc local signature to the copies
- On Linux/Windows:
  - Exits without modifying anything

## Standalone Backend for VS Code Addon

If you use the standalone backend, run:

```bash
cd backend
./first-run.sh
```

Then link the addon to the backend binary copy:

- `backend/go-on.local-signed` (macOS)
- `backend/go-on` (Linux)
- `backend/go-on.exe` (Windows)

## GUI Launch on macOS

If app copy is created, open:

- `go-on GUI-local-signed.app`

## Manual fallback

If needed, run helper directly:

```bash
./macos-gui-unblock.sh --copy "path/to/go-on GUI.app"
./macos-gui-unblock.sh --copy "path/to/go-on"
```
