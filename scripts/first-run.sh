#!/usr/bin/env bash
set -euo pipefail

# ROOT_DIR should point to the project root, not the scripts/ subdirectory.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OS_NAME="$(uname -s || echo unknown)"

# ── macOS-specific: Gatekeeper unblock ──────────────────────────────────
if [[ "$OS_NAME" == "Darwin" ]]; then
  UNBLOCK_SCRIPT="$ROOT_DIR/scripts/macos-gui-unblock.sh"
  if [[ ! -x "$UNBLOCK_SCRIPT" ]]; then
    if [[ -f "$UNBLOCK_SCRIPT" ]]; then
      chmod +x "$UNBLOCK_SCRIPT" || true
    else
      echo "macOS unblock helper not found: $UNBLOCK_SCRIPT"
      exit 1
    fi
  fi

  # Support both package layouts:
  # 1) root contains backend/ and optional *.app
  # 2) script is inside backend/ folder
  BACKEND_BIN=""
  if [[ -x "$ROOT_DIR/target/release/go-on" ]]; then
    BACKEND_BIN="$ROOT_DIR/target/release/go-on"
  elif [[ -x "$ROOT_DIR/target/debug/go-on" ]]; then
    BACKEND_BIN="$ROOT_DIR/target/debug/go-on"
  fi

  if [[ -n "$BACKEND_BIN" ]]; then
    "$UNBLOCK_SCRIPT" --copy "$BACKEND_BIN"
  fi

  APP_BUNDLE="$(find "$ROOT_DIR" -maxdepth 2 -type d -name "*.app" | head -1 || true)"
  if [[ -n "$APP_BUNDLE" ]]; then
    "$UNBLOCK_SCRIPT" --copy "$APP_BUNDLE"
  fi

  echo "macOS first-run trust bootstrap completed."
else
  echo "Skipping macOS-specific unblock (OS: $OS_NAME)."
fi

# ── Run skills environment setup (all platforms) ────────────────────────
SKILLS_SETUP="$ROOT_DIR/scripts/skills-setup.sh"
if [[ -x "$SKILLS_SETUP" ]]; then
  echo "Skills environment setup..."
  "$SKILLS_SETUP" rust
  "$SKILLS_SETUP" node
  "$SKILLS_SETUP" python
  echo "Skills installed: rust, node, python."
else
  echo "Skills setup script not found at $SKILLS_SETUP — skipping."
fi

# ── Create agent skills directory for SKILL.md auto-discovery (all platforms) ─
AGENT_SKILLS_DIR="$HOME/.agents/skills"
if [ ! -d "$AGENT_SKILLS_DIR" ]; then
  mkdir -p "$AGENT_SKILLS_DIR"
  echo "Created $AGENT_SKILLS_DIR — place SKILL.md files here for auto-discovery"
  # Write an example SKILL.md for users to reference.
  EXAMPLE_SKILL="$AGENT_SKILLS_DIR/example-skill/SKILL.md"
  mkdir -p "$(dirname "$EXAMPLE_SKILL")"
  cat > "$EXAMPLE_SKILL" << 'EOF'
---
name: example-skill
description: Example skill demonstrating SKILL.md format for go-on auto-discovery
version: 1.0.0
---

# Example Skill

This is an example skill for the go-on agent system.

## Usage

Run this command to test the skill:

```bash
echo "Hello from example-skill!"
```

## Input Schema

- `command` (string): The command to execute
EOF
  echo "Created example skill at $EXAMPLE_SKILL"
fi

echo "first-run setup finished."
