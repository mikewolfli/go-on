---
name: api-tester
description: Tests API endpoints with configurable requests, assertions, and response validation
version: 1.0.0
author: go-on-team
tags: [api, testing, http, validation, integration, quality]
min_go_on_version: 1.0.0
---

# API Tester Skill

Tests API endpoints by sending configurable HTTP requests and validating responses against expected schemas, status codes, headers, and body content. Supports chained test sequences, environment variables, and generates structured test reports.

## How It Works

1. **Request configuration** — Specify method, URL, headers, body, and query parameters
2. **Assertion engine** — Validates response status, headers, body content, and JSON schema
3. **Chained tests** — Run sequences of API calls where later requests can reference earlier responses
4. **Environment variables** — Template variables in request config are resolved from an environment map
5. **Report generation** — Produces a structured test report with pass/fail per assertion

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `endpoint` | string | The API endpoint URL |
| `method` | string | HTTP method (GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS) |
| `headers` | object | Optional HTTP headers as key-value pairs |
| `body` | string | Optional request body (JSON string) |
| `query` | object | Optional query parameters as key-value pairs |
| `expected_status` | integer | Expected HTTP status code (default: 200) |
| `expected_body_contains` | array | Optional list of strings that must appear in response body |
| `expected_json_schema` | object | Optional JSON Schema to validate the response body against |
| `timeout_ms` | integer | Request timeout in milliseconds (default: 15000) |
| `auth` | object | Optional authentication: `{"bearer": "token"}` or `{"basic": {"username": "...", "password": "..."}}` |

## Example

```json
{
  "endpoint": "https://api.example.com/users",
  "method": "GET",
  "headers": {
    "Accept": "application/json"
  },
  "expected_status": 200,
  "expected_body_contains": ["users", "total"],
  "expected_json_schema": {
    "type": "object",
    "properties": {
      "users": {"type": "array"},
      "total": {"type": "integer"}
    },
    "required": ["users", "total"]
  },
  "timeout_ms": 10000
}
```

**Example output:**

```json
{
  "success": true,
  "status": 200,
  "assertions": {
    "status_match": {"passed": true, "expected": 200, "actual": 200},
    "body_contains": {"passed": true, "checks": ["users": true, "total": true]},
    "schema_valid": {"passed": true}
  },
  "response_time_ms": 234,
  "response_size_bytes": 15420
}
```
