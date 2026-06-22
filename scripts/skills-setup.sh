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
    "Agent skills integration"
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
    if ! command -v curl &>/dev/null; then
        warn "curl not found — remote fetch tests will be skipped (optional)"
    else
        ok "Found curl: $(command -v curl)"
    fi

    # Check Rust toolchain components (clippy, rustfmt)
    if command -v rustup &>/dev/null; then
        local missing_components=""
        if ! rustup component list --installed 2>/dev/null | grep -q clippy; then
            missing_components="clippy"
            warn "clippy not installed — run: rustup component add clippy"
        fi
        if ! rustup component list --installed 2>/dev/null | grep -q rustfmt; then
            if [ -n "$missing_components" ]; then
                missing_components="$missing_components, rustfmt"
            else
                missing_components="rustfmt"
            fi
            warn "rustfmt not installed — run: rustup component add rustfmt"
        fi
        if [ -z "$missing_components" ]; then
            ok "Rust toolchain complete: $(rustc --version)"
        fi
    else
        warn "rustup not found — cannot validate toolchain components"
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

# ── Step 5: Agent Skills Integration ──────────────────────────────────────────

if should_run_step 5; then
    echo "── Step 5/5: Integrating agent skills from ~/.agents/skills/ ──"
    echo ""

    AGENT_SKILLS_DIR="$HOME/.agents/skills"
    IMPORTED_COUNT=0
    SKIPPED_COUNT=0

    if [ -d "$AGENT_SKILLS_DIR" ]; then
        info "Scanning $AGENT_SKILLS_DIR for installed skills..."
        echo ""

        for skill_dir in "$AGENT_SKILLS_DIR"/*/; do
            [ -d "$skill_dir" ] || continue
            local skill_name="$(basename "$skill_dir")"
            local skill_md="$skill_dir/SKILL.md"
            local agent_md="$skill_dir/agent.md"

            # Determine which metadata file to use: SKILL.md takes precedence
            local source_file=""
            if [ -f "$skill_md" ]; then
                source_file="$skill_md"
            elif [ -f "$agent_md" ]; then
                source_file="$agent_md"
            fi

            if [ -z "$source_file" ]; then
                warn "No SKILL.md or agent.md found in $skill_dir — skipping"
                SKIPPED_COUNT=$((SKIPPED_COUNT + 1))
                continue
            fi

            # Derive a safe import name from the SKILL.md frontmatter if possible,
            # otherwise use the directory name.
            local import_name="$skill_name"
            if head -20 "$source_file" 2>/dev/null | grep -q '^name:' ; then
                import_name="$(head -20 "$source_file" | grep '^name:' | head -1 | sed 's/^name:[[:space:]]*//' | tr -d '"')"
            fi

            # Destination: skills-import/<name>/SKILL.md
            local dest_dir="$SKILLS_IMPORT_DIR/$import_name"
            local dest_file="$dest_dir/SKILL.md"

            mkdir -p "$dest_dir"

            if cp "$source_file" "$dest_file"; then
                ok "Installed skill '$import_name' from $source_file"
                IMPORTED_COUNT=$((IMPORTED_COUNT + 1))

                # Also copy any agents/ subdirectory if present (carries agent config)
                if [ -d "$skill_dir/agents" ]; then
                    cp -r "$skill_dir/agents" "$dest_dir/" 2>/dev/null || true
                fi
                # Also copy any scripts/ subdirectory if present
                if [ -d "$skill_dir/scripts" ]; then
                    cp -r "$skill_dir/scripts" "$dest_dir/" 2>/dev/null || true
                fi
            else
                warn "Failed to copy skill '$import_name' from $source_file"
                SKIPPED_COUNT=$((SKIPPED_COUNT + 1))
            fi
        done
    else
        warn "Agent skills directory $AGENT_SKILLS_DIR does not exist — nothing to import"
    fi

    echo ""
    if [ "$IMPORTED_COUNT" -gt 0 ]; then
        ok "Imported $IMPORTED_COUNT skill(s) from $AGENT_SKILLS_DIR"
    fi
    if [ "$SKIPPED_COUNT" -gt 0 ]; then
        warn "$SKIPPED_COUNT skill(s) skipped (missing SKILL.md/agent.md)"
    fi
    if [ "$IMPORTED_COUNT" -eq 0 ] && [ "$SKIPPED_COUNT" -eq 0 ]; then
        info "No skills found in $AGENT_SKILLS_DIR"
    fi
else
    echo "── Step 5/5: (skipped) ──"
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
echo "  Agent skills:       $HOME/.agents/skills/ (if present)"
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
echo "    4. Run 'skills-setup.sh --step 5' to re-scan ~/.agents/skills/"
echo ""

# ── Final exit code ──────────────────────────────────────────────────────────

if [ ${#FAILED_STEPS[@]} -ne 0 ]; then
    exit 1
fi
