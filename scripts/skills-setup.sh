#!/usr/bin/env bash
# =============================================================================
# SKILLS Environment Setup — one-shot install per language (Node.js / Rust / Python)
#
# Usage:
#   ./skills-setup.sh         # show available targets
#   ./skills-setup.sh rust    # install Rust toolchain only
#   ./skills-setup.sh node    # install Node.js + npm only
#   ./skills-setup.sh python  # install Python + pip + venv only
#   ./skills-setup.sh all     # install all three
#
# This script is idempotent — safe to re-run. Each language installs only
# if the primary binary is not already on PATH.
# =============================================================================

set -euo pipefail

info()  { echo -e "\\033[1;34m[INFO]\\033[0m  $*"; }
ok()    { echo -e "\\033[1;32m[OK]\\033[0m    $*"; }
warn()  { echo -e "\\033[1;33m[WARN]\\033[0m  $*" >&2; }
err()   { echo -e "\\033[1;31m[ERROR]\\033[0m $*" >&2; }

# ── Detect OS ─────────────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
  Linux-x86_64)  RUST_TRIPLE="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64) RUST_TRIPLE="aarch64-unknown-linux-gnu" ;;
  Darwin-x86_64) RUST_TRIPLE="x86_64-apple-darwin"      ;;
  Darwin-arm64)  RUST_TRIPLE="aarch64-apple-darwin"      ;;
  *)
    err "Unsupported OS/arch: ${OS}-${ARCH}"
    exit 1
    ;;
esac

# ── Install Rust ──────────────────────────────────────────────────────────────
install_rust() {
  if command -v rustc &>/dev/null; then
    local ver
    ver="$(rustc --version 2>/dev/null | head -1)"
    ok "Rust already installed: ${ver}"
    # Ensure stable toolchain + wasm target
    rustup toolchain install stable 2>/dev/null || true
    rustup target add wasm32-unknown-unknown --toolchain stable 2>/dev/null || true
    return
  fi

  info "Installing Rust toolchain (rustup) …"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile default 2>&1
  # shellcheck source=/dev/null
  . "$HOME/.cargo/env"
  rustup target add wasm32-unknown-unknown --toolchain stable 2>/dev/null || true
  ok "Rust $(rustc --version) installed"
}

# ── Install Node.js via nvm / fnm ─────────────────────────────────────────────
install_node() {
  if command -v node &>/dev/null; then
    local ver
    ver="$(node --version 2>/dev/null)"
    ok "Node.js already installed: ${ver}"
    return
  fi

  info "Installing Node.js via fnm (Fast Node Manager) …"
  # Use fnm — faster than nvm and supports .node-version
  if ! command -v fnm &>/dev/null; then
    curl -fsSL https://fnm.vercel.app/install | bash 2>&1
    # shellcheck source=/dev/null
    export PATH="$HOME/.local/share/fnm:$PATH"
    if [ -f "$HOME/.bashrc" ]; then
      # shellcheck source=/dev/null
      . "$HOME/.bashrc" 2>/dev/null || true
    fi
  fi

  # Install latest LTS
  fnm install --lts 2>/dev/null || fnm install 22 2>/dev/null || true
  fnm default lts-latest 2>/dev/null || true

  if command -v node &>/dev/null; then
    ok "Node.js $(node --version) installed"
  else
    warn "Node.js installation may need a shell restart or manual PATH update."
  fi
}

# ── Install Python via uv (fast) ──────────────────────────────────────────────
install_python() {
  if command -v python3 &>/dev/null; then
    local ver
    ver="$(python3 --version 2>/dev/null)"
    ok "Python already installed: ${ver}"
    return
  fi

  info "Installing Python via uv (fast Python package installer) …"
  if ! command -v uv &>/dev/null; then
    curl -LsSf https://astral.sh/uv/install.sh | bash 2>&1
    export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
  fi

  # Install Python 3.12 via uv
  uv python install 3.12 2>/dev/null || true
  uv venv --python 3.12 2>/dev/null || true

  if command -v python3 &>/dev/null; then
    ok "Python $(python3 --version) installed"
  else
    warn "Python installation may need a shell restart or manual PATH update."
  fi
}

# ── Main ──────────────────────────────────────────────────────────────────────
main() {
  local target="${1:-help}"

  case "$target" in
    rust)
      install_rust
      ;;
    node|nodejs)
      install_node
      ;;
    python)
      install_python
      ;;
    all)
      install_rust
      install_node
      install_python
      ;;
    help|--help|-h|"")
      echo "Usage: $0 {rust|node|python|all}"
      echo ""
      echo "  rust    Install Rust toolchain (rustup + stable + wasm target)"
      echo "  node    Install Node.js via fnm (Fast Node Manager), LTS release"
      echo "  python  Install Python 3.12 via uv (fast Python package manager)"
      echo "  all     Install all three language toolchains"
      echo ""
      echo "Each target is idempotent — skips if already installed."
      ;;
    *)
      err "Unknown target: $target"
      echo "Usage: $0 {rust|node|python|all}"
      exit 1
      ;;
  esac
}

main "$@"
