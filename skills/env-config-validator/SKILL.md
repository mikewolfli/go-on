---
name: env-config-validator
description: Validates environment variable configurations and config files (YAML, TOML, JSON, .env) for missing required variables, type mismatches, deprecated keys, naming convention violations, secret exposure risks, and invalid value ranges
version: 1.0.0
author: go-on-team
tags: [env, config, validation, yaml, toml, json, dotenv, lint, security]
min_go_on_version: 1.0.0
---

# Env Config Validator Skill

Validates environment variable configurations and config files against a provided schema. Supports YAML, TOML, JSON, and .env formats. Detects missing required variables, type mismatches, deprecated keys, inconsistent naming conventions, secret exposure risks, and invalid value ranges. Can also compare a `.env.example` template against an actual `.env` file to flag drift.

## How It Works

1. **Parse input** — Extract the config content, config format, and schema/expected structure from the input parameters.
2. **Parse config** — Parse the provided config content according to its format (YAML, TOML, JSON, or .env key-value pairs).
3. **Validate against schema** — Run the following checks against each entry in the schema:
   - **Missing required variables** — Required keys that are absent from the config.
   - **Type mismatches** — Values that do not match the expected type (`string`, `number`, `boolean`, `array`, `object`).
   - **Deprecated keys** — Keys marked as deprecated in the schema that are still present in the config.
   - **Naming convention violations** — Keys that do not follow the expected naming convention (`UPPER_SNAKE_CASE`, `lower_snake_case`, `camelCase`, `kebab-case`).
   - **Secret exposure risks** — Values that look like secrets (API keys, tokens, passwords) in non-secret fields, or secrets exposed in committed config files.
   - **Invalid value ranges** — Numeric values outside allowed `min`/`max` bounds, or string values not in an allowed `enum` list.
4. **Compare .env.example drift** — If a `.env.example` template is provided alongside the actual `.env` file, flag missing, extra, or default-changed entries.
5. **Format output** — Return a structured validation report with error, warning, and suggestion counts, plus a detailed list of each finding with severity, affected key, and remediation guidance.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `config` | string | Raw config content to validate |
| `format` | string | Config format (`yaml`, `toml`, `json`, `env`) |
| `schema` | array | Array of expected entries: `[{"key": "...", "required": true, "type": "string", "deprecated": false, "convention": "UPPER_SNAKE_CASE", "min": null, "max": null, "enum": null}]` |
| `env_example` | string | Optional: content of `.env.example` to compare against the actual `.env` config |

## Example

```json
{
  "config": "DATABASE_URL=postgres://user:pass@localhost:5432/db\nREDIS_HOST=localhost\nREDIS_PORT=6379\nSECRET_KEY=sk-live-abc123\nLOG_LEVEL=verbose\n",
  "format": "env",
  "schema": [
    {"key": "DATABASE_URL", "required": true, "type": "string", "deprecated": false, "convention": "UPPER_SNAKE_CASE"},
    {"key": "REDIS_HOST", "required": true, "type": "string", "deprecated": false, "convention": "UPPER_SNAKE_CASE"},
    {"key": "REDIS_PORT", "required": true, "type": "number", "deprecated": false, "convention": "UPPER_SNAKE_CASE", "min": 1024, "max": 65535},
    {"key": "SECRET_KEY", "required": true, "type": "string", "deprecated": false, "convention": "UPPER_SNAKE_CASE"},
    {"key": "LOG_LEVEL", "required": true, "type": "string", "deprecated": false, "convention": "UPPER_SNAKE_CASE", "enum": ["debug", "info", "warn", "error"]},
    {"key": "API_TIMEOUT", "required": false, "type": "number", "deprecated": false, "convention": "UPPER_SNAKE_CASE", "min": 1, "max": 120}
  ],
  "env_example": "DATABASE_URL=postgres://user:pass@localhost:5432/db\nREDIS_HOST=localhost\nREDIS_PORT=6379\nLOG_LEVEL=info\nAPI_TIMEOUT=30\n"
}
```

**Example Output**

```json
{
  "summary": {
    "total_findings": 5,
    "errors": 2,
    "warnings": 2,
    "suggestions": 1
  },
  "findings": [
    {
      "severity": "error",
      "type": "type-mismatch",
      "key": "REDIS_PORT",
      "message": "Expected type 'number' but got string '6379'",
      "remediation": "Remove quotes around numeric values, or use a non-string type in the config file."
    },
    {
      "severity": "error",
      "type": "invalid-value",
      "key": "LOG_LEVEL",
      "message": "Value 'verbose' is not in allowed enum: [debug, info, warn, error]",
      "remediation": "Change LOG_LEVEL to one of: debug, info, warn, error."
    },
    {
      "severity": "warning",
      "type": "secret-exposure",
      "key": "SECRET_KEY",
      "message": "Value 'sk-live-abc123' matches a live secret pattern. Avoid hardcoding secrets in config files.",
      "remediation": "Rotate this key immediately and use a secrets manager or environment variable injection."
    },
    {
      "severity": "warning",
      "type": "missing-key",
      "key": "API_TIMEOUT",
      "message": "Optional key 'API_TIMEOUT' is missing; no default will be applied.",
      "remediation": "Add 'API_TIMEOUT' to the config or ensure the application handles its absence gracefully."
    },
    {
      "severity": "suggestion",
      "type": "env-example-drift",
      "key": "LOG_LEVEL",
      "message": "Default in .env.example is 'info', but actual config uses 'verbose'. This may indicate intentional override or configuration drift.",
      "remediation": "Verify the override is intentional, or update .env.example to match."
    }
  ]
}
```
