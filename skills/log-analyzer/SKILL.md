---
name: log-analyzer
description: Analyzes application and system logs to detect anomalies, patterns, errors, and performance issues
version: 1.0.0
author: go-on-team
tags: [logging, analysis, monitoring, debugging, observability, pattern-detection]
min_go_on_version: 1.0.0
---

# Log Analyzer Skill

Analyzes application logs, system logs, and structured event streams to detect error patterns, performance anomalies, recurring issues, and security-relevant events. Produces structured summaries, timeline visualizations, and actionable remediation suggestions.

## How It Works

1. **Parse** — Auto-detects log format (JSON lines, plain text, syslog, Apache/Nginx combined, structured, mixed) and extracts timestamp, severity, component, and message
2. **Cluster** — Groups similar log entries by error signature, stack trace hash, or message template to identify recurring issues
3. **Analyze** — Detects temporal patterns (spikes, periodic errors, correlated failures across components), severity escalations, and known error signatures
4. **Summarize** — Produces a structured report with error frequency, affected components, timeline, root cause candidates, and recommended actions

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `logs` | string | Raw log content to analyze |
| `format` | string | Log format hint: `auto`, `json-lines`, `plain`, `syslog`, `nginx` (default: `auto`) |
| `time_range` | string | Optional: focus on a specific time range (`last-hour`, `last-day`, `custom:start-end`) |
| `filter_severity` | string | Optional: minimum severity to report (`error`, `warn`, `info`, default: `warn`) |
| `include_anomalies` | boolean | Optional: detect anomalous patterns (default: true) |
| `include_timeline` | boolean | Optional: include event timeline (default: false) |

## Example

```json
{
  "logs": "[2026-06-28T10:23:01Z] ERROR api_server: connection pool exhausted (max=20, active=20, idle=0)\n[2026-06-28T10:23:02Z] WARN  db_query: query timed out after 30s: SELECT * FROM orders WHERE status = 'pending'\n[2026-06-28T10:23:03Z] ERROR api_server: request /api/orders failed: DatabaseError(Timeout)\n[2026-06-28T10:23:04Z] ERROR api_server: connection pool exhausted (max=20, active=20, idle=0)\n[2026-06-28T10:23:05Z] INFO  scheduler: health check passed (uptime=72h, p99_latency=2450ms)\n[2026-06-28T10:23:06Z] ERROR api_server: connection pool exhausted (max=20, active=20, idle=0)\n[2026-06-28T10:23:10Z] WARN  db_migrate: migration 0042_reindex_orders took 45s (threshold=30s)",
  "format": "plain",
  "filter_severity": "warn",
  "include_anomalies": true,
  "include_timeline": true
}
```

**Example output (abbreviated):**

```markdown
# Log Analysis Report

## Critical Issues

### Connection Pool Exhaustion (3 occurrences)
- **Severity:** ERROR
- **Timeline:** 10:23:01 → 10:23:04 → 10:23:06 (5-second window)
- **Pattern:** Recurring every ~3 seconds — suggests sustained load exceeding pool capacity
- **Root Cause Candidates:**
  1. Connection pool too small (max=20) for concurrent request volume
  2. Connections not released promptly after query completion
  3. Long-running queries holding connections (see: query timeout at 10:23:02)
- **Recommendation:** Increase `max_connections` to 50 and add `connection_timeout_ms: 5000`

## Warnings

| Time | Component | Message | Frequency |
|------|-----------|---------|-----------|
| 10:23:02 | db_query | query timed out after 30s | 1 |
| 10:23:10 | db_migrate | migration 0042_reindex_orders took 45s | 1 |

## Anomaly Detection

✅ **Cluster detected:** 3× connection_pool_exhausted errors within 5s (p<0.01)
⚠️ **Correlation:** connection_pool_exhausted → db_query timeout → all errors are database-related
✅ **No security-relevant patterns detected**

## Event Timeline

```
10:23:01 ── ERROR api_server: connection pool exhausted
10:23:02 ── WARN  db_query: query timed out
10:23:03 ── ERROR api_server: request failed (DatabaseError)
10:23:04 ── ERROR api_server: connection pool exhausted (repeat)
10:23:06 ── ERROR api_server: connection pool exhausted (repeat)
10:23:10 ── WARN  db_migrate: slow migration
```
