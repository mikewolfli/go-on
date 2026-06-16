#!/usr/bin/env bash
# =============================================================================
# skills-setup.sh — Go-On Skills Environment Setup
#
# Automates the creation and verification of the skills environment:
#   1. Creates the required directory structure
#   2. Validates system dependencies
#   3. Validates configuration presence
#   4. Creates a sample skills source file
#
# Usage:
#   ./scripts/skills-setup.sh              # Run all steps
#   ./scripts/skills-setup.sh --step 1     # Run only step 1
#   ./scripts/skills-setup.sh --step 1,3   # Run steps 1 and 3
#   ./scripts/skills-setup.sh --list-steps # List available steps
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

# List of step labels (order-dependent)
STEPS=(
    "System dependencies"
    "Directory structure"
    "Configuration validation"
    "Sample skills source"
)

# ---- Functions -------------------------------------------------------------

info()  { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Tracks overall and per-step failures
FAILED_STEPS=()
record_failure() {
    local step="$1"
    local message="$2"
    error "$message"
    FAILED_STEPS+=("$step")
}

check_cmd() {
    if command -v "$1" &>/dev/null; then
        ok "Found $1: $(command -v "$1")"
        return 0
    else
        error "$1 is not installed"
        return 1
    fi
}

# ---- Argument parsing ------------------------------------------------------

RUN_ALL=true
SELECTED_STEPS=()

usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --step N        Run only step N (can be comma-separated, e.g. --step 1,3)"
    echo "  --list-steps    List available steps and exit"
    echo "  --help          Show this help message and exit"
    echo ""
    echo "Steps:"
    for i in "${!STEPS[@]}"; do
        printf "  %d. %s\n" $((i + 1)) "${STEPS[$i]}"
    done
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --step)
            RUN_ALL=false
            IFS=',' read -ra PARTS <<< "$2"
            for part in "${PARTS[@]}"; do
                # Validate it's a number in range
                if [[ "$part" =~ ^[0-9]+$ ]] && [ "$part" -ge 1 ] && [ "$part" -le "${#STEPS[@]}" ]; then
                    SELECTED_STEPS+=("$part")
                else
                    echo "Error: invalid step number '$part'. Valid range: 1-${#STEPS[@]}"
                    exit 1
                fi
            done
            shift 2
            ;;
        --list-steps)
            usage
            ;;
        --help)
            usage
            ;;
        *)
            echo "Error: unknown argument '$1'"
            usage
            ;;
    esac
done

should_run_step() {
    local num="$1"
    if [ "$RUN_ALL" = true ]; then
        return 0
    fi
    for selected in "${SELECTED_STEPS[@]}"; do
        if [ "$selected" -eq "$num" ]; then
            return 0
        fi
    done
    return 1
}

# ---- Header ----------------------------------------------------------------

echo ""
echo "=============================================="
echo "  Go-On Skills Environment Setup"
echo "=============================================="
echo ""

if [ "$RUN_ALL" = false ]; then
    info "Running selected steps: ${SELECTED_STEPS[*]}"
    echo ""
fi

# ── Step 1: System Dependencies ─────────────────────────────────────────────

if should_run_step 1; then
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
        record_failure "1" "One or more required dependencies are missing."
        echo "  Install them via your package manager, for example:"
        echo "    apt install build-essential pkg-config libssl-dev git curl  (Debian/Ubuntu)"
        echo "    dnf install gcc gcc-c++ openssl-devel git curl              (Fedora)"
        echo "    brew install rust git curl                                  (macOS)"
        echo ""
    fi
else
    echo "── Step 1/4: (skipped) ──"
    echo ""
fi

# ── Step 2: Directory Structure ──────────────────────────────────────────────

if should_run_step 2; then
    echo "── Step 2/4: Creating directory structure ──"
    echo ""

    mkdir -p "$SKILLS_DIR"
    mkdir -p "$SKILLS_IMPORT_DIR"
    mkdir -p "$SKILLS_CACHE_DIR"

    ok "Skills source folder:   $SKILLS_DIR"
    ok "Skills import folder:   $SKILLS_IMPORT_DIR"
    ok "Skills cache folder:    $SKILLS_CACHE_DIR"
else
    echo "── Step 2/4: (skipped) ──"
    echo ""
fi

# ── Step 3: Validate Configuration ──────────────────────────────────────────

if should_run_step 3; then
    echo ""
    echo "── Step 3/4: Validating configuration ──"
    echo ""

    if [ -f "$CONFIG_FILE" ]; then
        ok "Configuration file found: $CONFIG_FILE"
    else
        record_failure "3" "Configuration file not found: $CONFIG_FILE"
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
else
    echo "── Step 3/4: (skipped) ──"
    echo ""
fi

# ── Step 4: Create Sample Source ─────────────────────────────────────────────

if should_run_step 4; then
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
else
    echo "── Step 4/4: (skipped) ──"
    echo ""
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "=============================================="
echo "  Setup Summary"
echo "=============================================="
echo ""
echo "  Config directory:   $CONFIG_DIR"
echo "  Skills directory:   $SKILLS_DIR"
echo "  Import directory:   $SKILLS_IMPORT_DIR"
echo "  Cache directory:    $SKILLS_CACHE_DIR"
echo ""

if [ ${#FAILED_STEPS[@]} -eq 0 ]; then
    ok "All executed steps completed successfully."
else
    echo "  ${RED}Failed steps:${NC}"
    for step in "${FAILED_STEPS[@]}"; do
        idx=$((step - 1))
        echo "    ${RED}• Step ${step}: ${STEPS[$idx]}${NC}"
    done
    echo ""
fi

# ── Connectivity test (optional, non-fatal) ───────────────────────────────────

if command -v go-on &>/dev/null; then
    info "Testing connectivity to skills system..."
    if go-on skill list &>/dev/null 2>&1; then
        ok "Skill system is operational"
    else
        warn "go-on skill list returned an error — Go-On may not be fully configured"
        warn "This is not a setup failure; verify configuration and try again after setup."
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

# ── Final exit code ──────────────────────────────────────────────────────────

if [ ${#FAILED_STEPS[@]} -ne 0 ]; then
    exit 1
fi
