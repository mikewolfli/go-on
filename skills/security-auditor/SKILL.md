---
name: security-auditor
description: Analyzes source code for common security vulnerabilities including SQL injection, XSS, command injection, hardcoded secrets, insecure deserialization, path traversal, and SSRF
version: 1.0.0
author: go-on-team
tags: [security, audit, vulnerability, static-analysis, rust, python, typescript, go]
min_go_on_version: 1.0.0
---

# Security Auditor Skill

Scans source code for security vulnerabilities across multiple programming languages. Returns categorized findings with severity levels, affected source lines, and actionable remediation suggestions to help developers fix issues before they reach production.

## How It Works

1. **Parse input** — Extract the source code and target language from the input parameters.
2. **Analyze for vulnerabilities** — Scan the code against a built-in knowledge base of common vulnerability patterns, including:
   - **SQL injection** — Unsanitized user input in raw SQL queries, string concatenation in query builders
   - **Cross-Site Scripting (XSS)** — Unsafe HTML interpolation, missing output encoding in templates
   - **Command injection** — Unsanitized shell execution via `exec`, `spawn`, `Command`, `os.system`
   - **Hardcoded secrets** — Inline API keys, tokens, passwords, private keys in source code
   - **Insecure deserialization** — Unsafe deserialization of untrusted data (`pickle`, `JSON.parse`, `serde_json::from_reader` without validation)
   - **Path traversal** — Unsanitized file path construction from user input
   - **Server-Side Request Forgery (SSRF)** — Unsanitized user-controlled URLs passed to HTTP clients
3. **Assign severity** — Each finding is rated `critical`, `high`, `medium`, or `low` based on exploitability and potential impact.
4. **Format output** — Return a structured JSON report with a summary and a list of categorized findings, each including the affected line, severity, vulnerability type, description, and remediation suggestion.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | Source code to audit for security vulnerabilities |
| `language` | string | Programming language (`rust`, `python`, `typescript`, `go`, `javascript`) |
| `include_suppressions` | boolean | Optional: honor inline suppression comments (default: true) |
| `risk_acceptance_path` | string | Optional: path to a known risk-acceptance file to exclude pre-approved patterns |

## Example

```json
{
  "code": "import sqlite3\n\ndef get_user(user_id):\n    conn = sqlite3.connect('users.db')\n    cursor = conn.cursor()\n    query = f\"SELECT * FROM users WHERE id = '{user_id}'\"\n    cursor.execute(query)\n    return cursor.fetchall()\n\nAPI_KEY = 'sk-live-abc123def456'\n\ndef run(cmd):\n    import os\n    os.system(cmd)\n",
  "language": "python",
  "include_suppressions": true
}
```

**Example Output**

```json
{
  "summary": {
    "total_findings": 3,
    "critical": 1,
    "high": 2,
    "medium": 0,
    "low": 0,
    "language": "python"
  },
  "findings": [
    {
      "type": "sql-injection",
      "severity": "critical",
      "line": 6,
      "code_snippet": "query = f\"SELECT * FROM users WHERE id = '{user_id}'\"",
      "description": "User-controlled input `user_id` is interpolated directly into a SQL query string, enabling SQL injection.",
      "remediation": "Use parameterized queries: `cursor.execute('SELECT * FROM users WHERE id = ?', (user_id,))`",
      "cwe": "CWE-89"
    },
    {
      "type": "hardcoded-secret",
      "severity": "high",
      "line": 9,
      "code_snippet": "API_KEY = 'sk-live-abc123def456'",
      "description": "A live API key appears as a string literal in source code. Secrets in source can be leaked via version control.",
      "remediation": "Move the key to an environment variable or a secrets manager, and rotate the exposed key immediately.",
      "cwe": "CWE-798"
    },
    {
      "type": "command-injection",
      "severity": "high",
      "line": 12,
      "code_snippet": "os.system(cmd)",
      "description": "User-controlled input is passed to `os.system()` without sanitization, enabling arbitrary command execution.",
      "remediation": "Avoid `os.system()` and `os.popen()`. Use `subprocess.run()` with a list argument and no `shell=True`. Validate or constrain input to an allowlist of safe commands.",
      "cwe": "CWE-78"
    }
  ]
}
```
