#!/usr/bin/env bash
# =============================================================================
# skills-setup.sh — Go-On Skills Environment Setup
#
# Automates the creation and verification of the skills environment:
#   1. Creates the required directory structure
#   2. Validates system dependencies
#   3. Validates configuration presence
#   4. Creates a sample skills source file
# =============================================================================

set -euo pipefail

# ---- Constants -------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Default config root — can be overridden via GOON_CONFIG_DIR env var
CONFIG_DIR="${GOON_CONFIG_DIR:-$HOME/.config/go-on}"

SKILLS_DIR="$CONFIG_DIR/skills"
SKILLS_IMPORT_DIR="$CONFIG_DIR/skills-import"
SKILLS_CACHE_DIR="$CONFIG_DIR/skills-cache"
CONFIG_FILE="$CONFIG_DIR/go-on.yaml"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# ---- Functions -------------------------------------------------------------

info()  { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; }

check_cmd() {
    if command -v "$1" &>/dev/null; then
        ok "Found $1: $(command -v "$1")"
        return 0
    else
        error "$1 is not installed"
        return 1
    fi
}

# ---- Steps -----------------------------------------------------------------

echo ""
echo "=============================================="
echo "  Go-On Skills Environment Setup"
echo "=============================================="
echo ""

# ── Step 1: System Dependencies ─────────────────────────────────────────────

echo "── Step 1/4: Checking system dependencies ──"
echo ""

DEP_MISSING=0
check_cmd "cargo"    || DEP_MISSING=1
check_cmd "rustc"    || DEP_MISSING=1
check_cmd "git"      || DEP_MISSING=1

# curl is used for remote fetch tests (optional)
if ! check_cmd "curl" &>/dev/null; then
    warn "curl not found — remote fetch tests will be skipped (optional)"
fi

echo ""
if [ "$DEP_MISSING" -ne 0 ]; then
    error "One or more required dependencies are missing."
    echo "  Install them via your package manager, for example:"
    echo "    apt install build-essential pkg-config libssl-dev git curl  (Debian/Ubuntu)"
    echo "    dnf install gcc gcc-c++ openssl-devel git curl              (Fedora)"
    echo "    brew install rust git curl                                  (macOS)"
    echo ""
fi

# ── Step 2: Directory Structure ──────────────────────────────────────────────

echo "── Step 2/4: Creating directory structure ──"
echo ""

mkdir -p "$SKILLS_DIR"
mkdir -p "$SKILLS_IMPORT_DIR"
mkdir -p "$SKILLS_CACHE_DIR"

ok "Skills source folder:   $SKILLS_DIR"
ok "Skills import folder:   $SKILLS_IMPORT_DIR"
ok "Skills cache folder:    $SKILLS_CACHE_DIR"

# ── Step 3: Validate Configuration ──────────────────────────────────────────

echo ""
echo "── Step 3/4: Validating configuration ──"
echo ""

if [ -f "$CONFIG_FILE" ]; then
    ok "Configuration file found: $CONFIG_FILE"
else
    warn "Configuration file not found: $CONFIG_FILE"
    warn "Create one at $CONFIG_FILE or set GOON_CONFIG_DIR"
    echo ""
    echo "  Minimal skill config example:"
    echo ""
    echo "  skill_import:"
    echo "    enabled: true"
    echo "    allowed_sources:"
    echo "      - \"github.com/my-org/*\""
    echo "    require_sha256: false"
    echo "    allow_floating_ref: true"
    echo ""
fi

# ── Step 4: Create Sample Source ─────────────────────────────────────────────

echo "── Step 4/4: Creating sample skills source ──"
echo ""

SAMPLE_FILE="$SKILLS_DIR/community.txt"
if [ ! -f "$SAMPLE_FILE" ]; then
    cat > "$SAMPLE_FILE" <<-'EOF'
# Community skill sources
# Uncomment the line below to add a remote source:
# https://raw.githubusercontent.com/my-org/community-skills/main/index.json
EOF
    ok "Sample source file created: $SAMPLE_FILE"
else
    ok "Source file already exists: $SAMPLE_FILE"
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=============================================="
echo "  Setup Complete"
echo "=============================================="
echo ""
echo "  Config directory:   $CONFIG_DIR"
echo "  Skills directory:   $SKILLS_DIR"
echo "  Import directory:   $SKILLS_IMPORT_DIR"
echo "  Cache directory:    $SKILLS_CACHE_DIR"
echo ""

if command -v go-on &>/dev/null; then
    info "Testing connectivity to skills system..."
    if go-on skill list &>/dev/null 2>&1; then
        ok "Skill system is operational"
    else
        warn "Skill system responds but Go-On may not be fully configured"
    fi
else
    hint="go-on binary not found in PATH — build with 'cargo build' first"
    echo "  ${YELLOW}[HINT]${NC}  $hint"
fi

echo ""
echo "  Next steps:"
echo "    1. Add remote skill URLs to: $SKILLS_DIR/"
echo "    2. Import skills: go-on skill import <name>"
echo "    3. List registered skills: go-on skill list"
echo ""
