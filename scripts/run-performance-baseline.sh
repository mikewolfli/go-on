#!/usr/bin/env bash
# Performance Benchmark Baseline
# Establishes P95/P99 latency baseline for key RPC endpoints.
set -euo pipefail

echo "=== go-on Performance Benchmark Baseline ==="
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

BINARY="${1:-./target/debug/go-on}"
CONFIG="${2:-config.test.toml}"
REPORT_DIR="bench-results/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$REPORT_DIR"

# Build if binary doesn't exist
if [ ! -f "$BINARY" ]; then
    echo "Building go-on..."
    cargo build 2>/dev/null
fi

echo "Binary: $BINARY"
echo "Config: $CONFIG"
echo "Report: $REPORT_DIR/"
echo ""

# Start server
$BINARY --config "$CONFIG" &
SERVER_PID=$!
sleep 2

# Cleanup on exit
cleanup() {
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Test endpoints
echo "Running benchmarks..."

# 1. Health check latency
echo "  1/5 health ..."
# Use sequential requests to measure latency
for i in $(seq 1 10); do
    curl -s -o /dev/null -w "%{time_total}\\n" \
        -X POST http://127.0.0.1:8080/health \
        >> "$REPORT_DIR/health-latency.txt" 2>/dev/null || echo "failed" >> "$REPORT_DIR/health-latency.txt"
done

# 2. Governance status
echo "  2/5 governance.status ..."
for i in $(seq 1 10); do
    curl -s -o /dev/null -w "%{time_total}\\n" \
        -X POST http://127.0.0.1:8080/governance/status \
        >> "$REPORT_DIR/governance-latency.txt" 2>/dev/null || echo "failed" >> "$REPORT_DIR/governance-latency.txt"
done

# 3. Capabilities list
echo "  3/5 capabilities.list ..."
for i in $(seq 1 10); do
    curl -s -o /dev/null -w "%{time_total}\\n" \
        -X POST http://127.0.0.1:8080/capabilities \
        >> "$REPORT_DIR/capabilities-latency.txt" 2>/dev/null || echo "failed" >> "$REPORT_DIR/capabilities-latency.txt"
done

# 4. Initialize
echo "  4/5 initialize ..."
for i in $(seq 1 10); do
    curl -s -o /dev/null -w "%{time_total}\\n" \
        -X POST http://127.0.0.1:8080/initialize \
        -H "Content-Type: application/json" \
        -d '{"protocol":"acp","version":"1.0"}' \
        >> "$REPORT_DIR/initialize-latency.txt" 2>/dev/null || echo "failed" >> "$REPORT_DIR/initialize-latency.txt"
done

# 5. Chat completion
echo "  5/5 chat completion ..."
for i in $(seq 1 5); do
    curl -s -o /dev/null -w "%{time_total}\\n" \
        -X POST http://127.0.0.1:8080/v1/chat/completions \
        -H "Content-Type: application/json" \
        -d '{"model":"test","messages":[{"role":"user","content":"hi"}],"max_tokens":5}' \
        >> "$REPORT_DIR/chat-latency.txt" 2>/dev/null || echo "failed" >> "$REPORT_DIR/chat-latency.txt"
done

# Generate report
echo ""
echo "=== Baseline Report ==="
echo "Endpoint | P50 | P95 | P99 | Samples"
echo "---------|-----|-----|-----|-------"

for endpoint in health governance capabilities initialize chat; do
    file="$REPORT_DIR/${endpoint}-latency.txt"
    if [ -f "$file" ]; then
        values=$(grep -E '^[0-9]+\.[0-9]+$' "$file" | sort -n | tr '\n' ' ')
        count=$(echo "$values" | wc -w)
        if [ "$count" -gt 0 ]; then
            p50=$(echo "$values" | awk '{print $int('"$count"'*0.5)}')
            p95=$(echo "$values" | awk '{print $int('"$count"'*0.95)}')
            p99=$(echo "$values" | awk '{print $int('"$count"'*0.99)}')
            echo "$endpoint | ${p50}s | ${p95}s | ${p99}s | $count"
        else
            echo "$endpoint | N/A | N/A | N/A | 0"
        fi
    fi
done

echo ""
echo "Report saved to: $REPORT_DIR/"
