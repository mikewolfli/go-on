#!/usr/bin/env bash
# =============================================================================
# skills-setup.sh — Go-On Skills Environment Setup
#
# Automates the creation and verification of the skills environment:
#   1. Validates system dependencies
#   2. Creates the required directory structure
#   3. Validates configuration presence
#   4. Creates a sample skills source file
#   5. Integrates agent skills from ~/.agents/skills/
#   6. Sets up Node.js / TypeScript SDK
#   7. Sets up Python SDK
#   8. Validates Rust SDK compilation
#   9. Runs end-to-end connectivity test
#
# Usage:
#   ./scripts/skills-setup.sh              # Run all steps
#   ./scripts/skills-setup.sh --step 1     # Run only step 1
#   ./scripts/skills-setup.sh --step 1,3   # Run steps 1 and 3
#   ./scripts/skills-setup.sh --list-steps # List available steps
# =============================================================================

set -uo pipefail
# Soft-fail mode: set -e is NOT used globally because SDK steps (Node.js,
# Python, Rust) may fail when the corresponding toolchain is absent.
# Instead, individual steps opt into strict mode via run_step_strict().

# ---- Constants -------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Default config root — can be overridden via GOON_CONFIG_DIR env var
CONFIG_DIR="${GOON_CONFIG_DIR:-$HOME/.config/go-on}"

SKILLS_DIR="$CONFIG_DIR/skills"
SKILLS_IMPORT_DIR="$CONFIG_DIR/skills-import"
SKILLS_CACHE_DIR="$CONFIG_DIR/skills-cache"
CONFIG_FILE="$CONFIG_DIR/config.toml"

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
    "Node.js/TypeScript SDK setup"
    "Python SDK setup"
    "Rust SDK validation"
    "End-to-end connectivity test"
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

# Run a command with soft-fail: if it exits non-zero, log a warning and
# record a failure but do NOT abort the entire setup script.
# This is used for optional SDK setup steps where missing tooling should
# not block the rest of the setup.
soft_fail() {
    local step="$1"
    local label="$2"
    shift 2
    if ! "$@"; then
        warn "$label encountered issues (exit code $?) — continuing"
        record_failure "$step" "$label failed (non-fatal)"
        return 1
    fi
    return 0
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
    echo "── Step 1/${#STEPS[@]}: Checking system dependencies ──"
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

    # Node.js and npm are optional runtime dependencies
    if ! command -v node &>/dev/null; then
        warn "node not found — Node.js skill execution will be unavailable (optional)"
    else
        ok "Found node: $(command -v node)"
    fi

    if ! command -v npm &>/dev/null; then
        warn "npm not found — npm package installation for skills will be unavailable (optional)"
    else
        ok "Found npm: $(command -v npm)"
    fi

    # Python is an optional runtime dependency
    if command -v python3 &>/dev/null; then
        ok "Found python3: $(command -v python3)"
    elif command -v python &>/dev/null; then
        ok "Found python: $(command -v python)"
    else
        warn "python3/python not found — Python skill execution will be unavailable (optional)"
    fi

    # Check Rust toolchain components (clippy, rustfmt)
    if command -v rustup &>/dev/null; then
        missing_components=""
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
    echo "── Step 1/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 2: Directory Structure ──────────────────────────────────────────────

if should_run_step 2; then
    echo "── Step 2/${#STEPS[@]}: Creating directory structure ──"
    echo ""

    mkdir -p "$SKILLS_DIR"
    mkdir -p "$SKILLS_IMPORT_DIR"
    mkdir -p "$SKILLS_CACHE_DIR"

    ok "Skills source folder:   $SKILLS_DIR"
    ok "Skills import folder:   $SKILLS_IMPORT_DIR"
    ok "Skills cache folder:    $SKILLS_CACHE_DIR"
else
    echo "── Step 2/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 3: Validate Configuration ──────────────────────────────────────────

if should_run_step 3; then
    echo ""
    echo "── Step 3/${#STEPS[@]}: Validating configuration ──"
    echo ""

    if [ -f "$CONFIG_FILE" ]; then
        ok "Configuration file found: $CONFIG_FILE"

        # Validate the config has required sections
        if grep -q '^default_phase' "$CONFIG_FILE" 2>/dev/null; then
            ok "Config has default_phase set"
        else
            warn "Config is missing default_phase — skills may not work correctly"
        fi

        # Check for AI provider configuration
        HAS_PROVIDER=false
        if grep -q '\[agents\.' "$CONFIG_FILE" 2>/dev/null; then
            HAS_PROVIDER=true
            ok "Config has at least one [agents.*] section"
        fi

        # Also check for provider environment variables referenced in config
        if grep -qE 'api_key_env\s*=' "$CONFIG_FILE" 2>/dev/null; then
            while IFS='=' read -r _ env_var; do
                env_var="$(echo "$env_var" | tr -d ' "')"
                if [ -n "$env_var" ] && [ -n "${!env_var:-}" ]; then
                    ok "Environment variable $env_var is set (referenced in config)"
                    HAS_PROVIDER=true
                fi
            done < <(grep 'api_key_env' "$CONFIG_FILE" 2>/dev/null || true)
        fi

        if [ "$HAS_PROVIDER" = false ]; then
            warn "No AI providers found in config — prompt-based skills will not execute"
            warn "  Add an [agents.<name>] section to $CONFIG_FILE"
            warn "  Example: see the minimal config below"

            # Offer to auto-create provider config from env vars
            echo ""
            echo "  -- AI Provider Detection --"
            AUTO_CREATED=false

            if [ -n "${OPENAI_API_KEY:-}" ]; then
                info "OPENAI_API_KEY is set in environment"
                echo "    Would you like to write it into config.toml? [y/N] "
                read -r answer
                if [ "$answer" = "y" ] || [ "$answer" = "Y" ]; then
                    cat >> "$CONFIG_FILE" <<-PROXYEOF

[agents.openai]
type = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
PROXYEOF
                    ok "OpenAI provider added to $CONFIG_FILE"
                    AUTO_CREATED=true
                fi
            fi

            if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
                info "ANTHROPIC_API_KEY is set in environment"
                echo "    Would you like to write it into config.toml? [y/N] "
                read -r answer
                if [ "$answer" = "y" ] || [ "$answer" = "Y" ]; then
                    cat >> "$CONFIG_FILE" <<-PROXYEOF

[agents.anthropic]
type = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
PROXYEOF
                    ok "Anthropic provider added to $CONFIG_FILE"
                    AUTO_CREATED=true
                fi
            fi

            if [ "$AUTO_CREATED" = false ] && { [ -n "${OPENAI_API_KEY:-}" ] || [ -n "${ANTHROPIC_API_KEY:-}" ]; }; then
                warn "API keys are set but were not written to config"
                warn "  You can manually add an [agents.*] section to $CONFIG_FILE"
            fi
        fi
    else
        info "Config file not found at $CONFIG_FILE — creating minimal config..."

        # Create a minimal config.toml with skills enabled
        cat > "$CONFIG_FILE" <<-EOF
# Auto-generated by skills-setup.sh

default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "auto"

[cache]
enabled = true
path = "acp_cache.sqlite3"
default_ttl_seconds = 3600
max_entries = 5000

[vector]
enabled = true
auto_mode = true
path = "acp_vector.sqlite3"
dimensions = 192
min_query_chars = 80
top_k = 2
min_similarity = 0.82
max_snippet_chars = 800
max_entries = 10000
summary_enabled = true
summary_trigger_messages = 8
summary_max_chars = 1200

[runtime]
skills_enabled = true
skills_import_enabled = true
skills_allowed_sources = ["github.com/*", "https://*"]
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 30

[autotune]
enabled = false
evaluate_interval = 20

[agents]

[flow]
name = "Default"
workflow_type = "auto"
phases = ["planning", "coding", "review", "delivery"]

[phases.planning]
description = "Planning phase"
agents = []
fallback = true

[phases.planning.options]
request_timeout_seconds = 120
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128

[phases.coding]
description = "Coding phase"
agents = []
fallback = true

[phases.coding.options]
request_timeout_seconds = 300
review_timeout_seconds = 120
cache_enabled = true
vector_enabled = true
phase_max_inflight = 24
global_max_inflight = 128

[phases.review]
description = "Review phase"
agents = []
fallback = false

[phases.review.options]
request_timeout_seconds = 120
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
phase_max_inflight = 16
global_max_inflight = 128

[phases.delivery]
description = "Delivery phase"
agents = []
fallback = false

[phases.delivery.options]
request_timeout_seconds = 90
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 64
EOF
        ok "Minimal config.toml created at $CONFIG_FILE"

        # Detect API keys from environment and offer to add provider
        echo ""
        echo "  -- AI Provider Detection --"
        PROVIDER_ADDED=false

        if [ -n "${OPENAI_API_KEY:-}" ]; then
            info "OPENAI_API_KEY is set in environment"
            echo "    Add OpenAI provider to config? [Y/n] "
            read -r answer
            if [ "$answer" != "n" ] && [ "$answer" != "N" ]; then
                cat >> "$CONFIG_FILE" <<-PROXYEOF

[agents.openai]
type = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
PROXYEOF
                ok "OpenAI provider added to $CONFIG_FILE"
                PROVIDER_ADDED=true
            fi
        fi

        if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
            info "ANTHROPIC_API_KEY is set in environment"
            if [ "$PROVIDER_ADDED" = true ]; then
                echo "    Add Anthropic provider to config? [y/N] "
            else
                echo "    Add Anthropic provider to config? [Y/n] "
            fi
            read -r answer
            if [ "$answer" != "n" ] && [ "$answer" != "N" ] && [ "$PROVIDER_ADDED" = false ]; then
                cat >> "$CONFIG_FILE" <<-PROXYEOF

[agents.anthropic]
type = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
PROXYEOF
                ok "Anthropic provider added to $CONFIG_FILE"
                PROVIDER_ADDED=true
            elif [ "$answer" = "y" ] || [ "$answer" = "Y" ]; then
                cat >> "$CONFIG_FILE" <<-PROXYEOF

[agents.anthropic]
type = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"
PROXYEOF
                ok "Anthropic provider added to $CONFIG_FILE"
            fi
        fi

        if [ "$PROVIDER_ADDED" = false ]; then
            warn "No AI provider configured — prompt-based skills will not execute"
            warn "  Set OPENAI_API_KEY or ANTHROPIC_API_KEY and re-run this script"
            warn "  Or manually add an [agents.<name>] section to $CONFIG_FILE"
        fi
    fi
else
    echo "── Step 3/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 4: Create Sample Source ─────────────────────────────────────────────

if should_run_step 4; then
    echo "── Step 4/${#STEPS[@]}: Creating sample skills source ──"
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
    echo "── Step 4/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 5: Agent Skills Integration ──────────────────────────────────────────

if should_run_step 5; then
    echo "── Step 5/${#STEPS[@]}: Integrating agent skills from ~/.agents/skills/ ──"
    echo ""

    AGENT_SKILLS_DIR="$HOME/.agents/skills"
    IMPORTED_COUNT=0
    SKIPPED_COUNT=0

    if [ -d "$AGENT_SKILLS_DIR" ]; then
        info "Scanning $AGENT_SKILLS_DIR for installed skills..."
        echo ""

        for skill_dir in "$AGENT_SKILLS_DIR"/*/; do
            [ -d "$skill_dir" ] || continue
            skill_name="$(basename "$skill_dir")"
            skill_md="$skill_dir/SKILL.md"
            agent_md="$skill_dir/agent.md"

            # Determine which metadata file to use: SKILL.md takes precedence
            source_file=""
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
            import_name="$skill_name"
            if head -20 "$source_file" 2>/dev/null | grep -q '^name:' ; then
                import_name="$(head -20 "$source_file" | grep '^name:' | head -1 | sed 's/^name:[[:space:]]*//' | tr -d '"')"
            fi

            # Destination: skills-import/<name>/SKILL.md
            dest_dir="$SKILLS_IMPORT_DIR/$import_name"
            dest_file="$dest_dir/SKILL.md"

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

    # ── Reverse sync: copy skills from skills-import/ to ~/.agents/skills/ ──
    # This ensures skills placed in $SKILLS_IMPORT_DIR are also discoverable
    # by the Rust discovery mechanism which scans ~/.agents/skills/.
    if [ -d "$SKILLS_IMPORT_DIR" ]; then
        info "Syncing skills from $SKILLS_IMPORT_DIR to $AGENT_SKILLS_DIR..."
        mkdir -p "$AGENT_SKILLS_DIR"
        for skill_dir in "$SKILLS_IMPORT_DIR"/*/; do
            [ -d "$skill_dir" ] || continue
            skill_name="$(basename "$skill_dir")"
            source_md="$skill_dir/SKILL.md"
            dest_dir="$AGENT_SKILLS_DIR/$skill_name"

            if [ ! -f "$source_md" ]; then
                continue
            fi

            mkdir -p "$dest_dir"
            if cp "$source_md" "$dest_dir/SKILL.md"; then
                ok "Synced skill '$skill_name' to $dest_dir"
                IMPORTED_COUNT=$((IMPORTED_COUNT + 1))

                # Also copy any agents/ subdirectory if present
                if [ -d "$skill_dir/agents" ]; then
                    cp -r "$skill_dir/agents" "$dest_dir/" 2>/dev/null || true
                fi
                # Also copy any scripts/ subdirectory if present
                if [ -d "$skill_dir/scripts" ]; then
                    cp -r "$skill_dir/scripts" "$dest_dir/" 2>/dev/null || true
                fi
            else
                warn "Failed to sync skill '$skill_name' to $AGENT_SKILLS_DIR"
            fi
        done
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
    echo "── Step 5/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 6: Node.js/TypeScript SDK Setup ──────────────────────────────────────

if should_run_step 6; then
    echo "── Step 6/${#STEPS[@]}: Node.js/TypeScript SDK setup ──"
    echo ""
    TS_SDK_MISSING=0

    # Check for TypeScript compiler
    if command -v tsc &>/dev/null; then
        ok "Found tsc: $(command -v tsc)"
    else
        warn "tsc not found — will attempt via npx or project-local typescript"
    fi

    # Helper to install dependencies and build one SDK directory
    setup_ts_sdk() {
        local sdk_label="$1"
        local sdk_path="$2"

        if [ ! -d "$sdk_path" ]; then
            warn "SDK directory not found: $sdk_path — skipping"
            return 1
        fi

        if [ ! -f "$sdk_path/package.json" ]; then
            warn "No package.json in $sdk_path — skipping"
            return 1
        fi

        info "Setting up $sdk_label SDK at $sdk_path"

        # Install dependencies
        if [ -f "$sdk_path/package-lock.json" ]; then
            info "Running npm ci in $sdk_label..."
            (cd "$sdk_path" && npm ci) && ok "$sdk_label: npm ci succeeded" || {
                warn "$sdk_label: npm ci failed — trying npm install"
                (cd "$sdk_path" && npm install) || {
                    record_failure "6" "$sdk_label: npm install failed"
                    return 1
                }
            }
        else
            info "Running npm install in $sdk_label..."
            (cd "$sdk_path" && npm install) || {
                record_failure "6" "$sdk_label: npm install failed"
                return 1
            }
        fi

        # Build
        info "Running npm run build in $sdk_label..."
        (cd "$sdk_path" && npm run build) && ok "$sdk_label: build succeeded" || {
            record_failure "6" "$sdk_label: build failed"
            return 1
        }

        # Validate build output exists
        if [ -d "$sdk_path/dist" ] && [ "$(ls -A "$sdk_path/dist" 2>/dev/null)" ]; then
            ok "$sdk_label: build output present in dist/"
        else
            warn "$sdk_label: dist/ directory missing or empty after build"
        fi

        return 0
    }

    if ! setup_ts_sdk "Node.js" "$PROJECT_DIR/sdk/nodejs"; then
        warn "Node.js SDK setup non-fatal — continuing"
    fi
    if ! setup_ts_sdk "TypeScript" "$PROJECT_DIR/sdk/typescript"; then
        warn "TypeScript SDK setup non-fatal — continuing"
    fi
    echo ""
    echo "  Node.js/TypeScript SDK: if setup had issues, install Node.js from https://nodejs.org/"
    echo ""
    echo "  Setup status recorded above (individual SDK warnings are non-fatal)."
else
    echo "── Step 6/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 7: Python SDK Setup ──────────────────────────────────────────────────

if should_run_step 7; then
    echo "── Step 7/${#STEPS[@]}: Python SDK setup ──"
    echo ""
    PY_SDK_MISSING=0

    PYTHON_SDK_DIR="$PROJECT_DIR/sdk/python"

    # Check for pip3
    if command -v pip3 &>/dev/null; then
        ok "Found pip3: $(command -v pip3)"
    elif command -v pip &>/dev/null; then
        ok "Found pip: $(command -v pip)"
    else
        record_failure "7" "pip3/pip not found — install python3-pip or ensure pip is in PATH"
        PY_SDK_MISSING=1
    fi

    # Determine which python to use
    if command -v python3 &>/dev/null; then
        PYTHON_BIN="python3"
    elif command -v python &>/dev/null; then
        PYTHON_BIN="python"
    else
        record_failure "7" "python3/python not found"
        PY_SDK_MISSING=1
    fi

    if [ "$PY_SDK_MISSING" -eq 0 ]; then
        # Create virtual environment
        VENV_DIR="$CONFIG_DIR/venv"
        if [ ! -d "$VENV_DIR" ]; then
            info "Creating Python virtual environment at $VENV_DIR ..."
            "$PYTHON_BIN" -m venv "$VENV_DIR" && {
                ok "Virtual environment created at $VENV_DIR"
            } || {
                record_failure "7" "Failed to create virtual environment at $VENV_DIR"
                PY_SDK_MISSING=1
            }
        else
            ok "Virtual environment already exists at $VENV_DIR"
        fi
    fi

    if [ "$PY_SDK_MISSING" -eq 0 ]; then
        # Activate venv and install SDK in dev mode
        info "Installing Python SDK in dev mode from $PYTHON_SDK_DIR ..."

        # Determine the correct pip in the venv
        VENV_PIP="$VENV_DIR/bin/pip"
        if [ ! -f "$VENV_PIP" ]; then
            VENV_PIP="$VENV_DIR/bin/pip3"
        fi

        if "$VENV_PIP" install -e "$PYTHON_SDK_DIR" 2>&1; then
            ok "Python SDK installed in dev mode"
        else
            record_failure "7" "pip install -e failed for $PYTHON_SDK_DIR"
            PY_SDK_MISSING=1
        fi
    fi

    if [ "$PY_SDK_MISSING" -eq 0 ]; then
        # Verify the SDK import works
        VENV_PYTHON="$VENV_DIR/bin/python"
        if [ ! -f "$VENV_PYTHON" ]; then
            VENV_PYTHON="$VENV_DIR/bin/python3"
        fi

        if "$VENV_PYTHON" -c "import go_on_sdk; print(f'go_on_sdk version: {go_on_sdk.__version__}')" 2>&1; then
            ok "Python SDK import verified (go_on_sdk)"
        else
            record_failure "7" "Python SDK import failed for go_on_sdk"
            PY_SDK_MISSING=1
        fi
    fi

    echo ""
    if [ "$PY_SDK_MISSING" -ne 0 ]; then
        warn "Python SDK setup had issues (non-fatal)."
        echo "  Install Python 3 from https://python.org/ to enable Python skills."
    fi
    echo ""
else
    echo "── Step 7/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 8: Rust SDK Validation ───────────────────────────────────────────────

if should_run_step 8; then
    echo "── Step 8/${#STEPS[@]}: Rust SDK validation ──"
    echo ""
    RS_SDK_MISSING=0

    # Optional pkg-config check
    if command -v pkg-config &>/dev/null; then
        ok "Found pkg-config: $(command -v pkg-config)"
    else
        warn "pkg-config not found — some native crate builds may fail (optional, install via your package manager)"
    fi

    RUST_SDK_DIR="$PROJECT_DIR/sdk/rust"
    if [ ! -d "$RUST_SDK_DIR" ]; then
        record_failure "8" "Rust SDK directory not found: $RUST_SDK_DIR"
        RS_SDK_MISSING=1
    fi

    if [ "$RS_SDK_MISSING" -eq 0 ]; then
        info "Building go_on_sdk package (workspace default features)..."
        if cargo build -p go_on_sdk 2>&1; then
            ok "Rust SDK (go_on_sdk) built successfully"
        else
            record_failure "8" "cargo build -p go_on_sdk failed"
            RS_SDK_MISSING=1
        fi
    fi

    # Verify the build artifact exists
    if [ "$RS_SDK_MISSING" -eq 0 ]; then
        # Check for the build artifacts in the SDK target directory
        SDK_TARGET="$RUST_SDK_DIR/target"
        if [ -d "$SDK_TARGET/debug" ] && ls "$SDK_TARGET/debug/libgo_on_sdk"* &>/dev/null 2>&1; then
            ok "Rust SDK build artifacts present"
        else
            # The workspace target might be at the project root instead
            WS_TARGET="$PROJECT_DIR/target"
            if [ -d "$WS_TARGET/debug" ] && ls "$WS_TARGET/debug/libgo_on_sdk"* &>/dev/null 2>&1; then
                ok "Rust SDK build artifacts present (workspace target)"
            else
                warn "Rust SDK build artifacts not found (expected in target/debug/)"
            fi
        fi
    fi

    echo ""
    if [ "$RS_SDK_MISSING" -ne 0 ]; then
        warn "Rust SDK setup had issues (non-fatal)."
        echo "  Ensure Rust toolchain is installed: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    fi
    echo ""
else
    echo "── Step 8/${#STEPS[@]}: (skipped) ──"
    echo ""
fi

# ── Step 9: End-to-End Connectivity Test ──────────────────────────────────────

if should_run_step 9; then
    echo "── Step 9/${#STEPS[@]}: End-to-end connectivity test ──"
    echo ""
    E2E_MISSING=0

    # Determine go-on binary location
    GO_ON_BIN=""
    if command -v go-on &>/dev/null; then
        GO_ON_BIN="$(command -v go-on)"
        ok "Found go-on binary in PATH: $GO_ON_BIN"
    elif [ -x "$PROJECT_DIR/target/debug/go-on" ]; then
        GO_ON_BIN="$PROJECT_DIR/target/debug/go-on"
        ok "Found go-on binary from workspace build: $GO_ON_BIN"
    else
        info "go-on binary not found — building with cargo..."
        if cargo build 2>&1; then
            GO_ON_BIN="$PROJECT_DIR/target/debug/go-on"
            ok "go-on binary built: $GO_ON_BIN"
        else
            record_failure "9" "Failed to build go-on binary"
            E2E_MISSING=1
        fi
    fi

    if [ "$E2E_MISSING" -eq 0 ] && [ -n "$GO_ON_BIN" ]; then
        # Start the server in the background
        SERVER_PORT="${GO_ON_PORT:-8080}"
        info "Starting go-on server on port $SERVER_PORT ..."
        SERVER_LOG=$(mktemp)
        "$GO_ON_BIN" server &>/dev/null &
        SERVER_PID=$!
        info "Server PID: $SERVER_PID"

        # Cleanup function
        cleanup() {
            if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
                info "Stopping server (PID $SERVER_PID)..."
                kill "$SERVER_PID" 2>/dev/null || true
                wait "$SERVER_PID" 2>/dev/null || true
                ok "Server stopped"
            fi
            rm -f "$SERVER_LOG"
        }

        # Wait for the /health endpoint to respond
        HEALTH_URL="http://localhost:${SERVER_PORT}/health"
        HEALTH_OK=false
        info "Waiting for health endpoint at $HEALTH_URL ..."
        for i in $(seq 1 15); do
            if curl -sf "$HEALTH_URL" &>/dev/null; then
                HEALTH_OK=true
                ok "Health endpoint responding (attempt $i)"
                break
            fi
            sleep 1
        done

        if [ "$HEALTH_OK" = false ]; then
            # Check if the process is still running
            if ! kill -0 "$SERVER_PID" 2>/dev/null; then
                warn "Server process exited prematurely"
            else
                warn "Health endpoint not responding after 15 seconds"
            fi
            cleanup
            record_failure "9" "Server health check failed"
            E2E_MISSING=1
        fi

        if [ "$E2E_MISSING" -eq 0 ]; then
            # Test go-on skill list
            info "Testing: go-on skill list ..."
            if "$GO_ON_BIN" skill list 2>&1; then
                ok "go-on skill list succeeded"
            else
                warn "go-on skill list returned a non-zero exit (may be expected if no skills are configured)"
            fi
        fi

        # Stop the server
        cleanup
    fi

    echo ""
else
    echo "── Step 9/${#STEPS[@]}: (skipped) ──"
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
