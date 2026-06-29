---
name: api-docs-generator
description: Generates structured API documentation from source code, including endpoints, types, and examples
version: 1.0.0
author: go-on-team
tags: [documentation, api, openapi, rustdoc, typedoc]
min_go_on_version: 1.0.0
---

# API Documentation Generator Skill

Parses source code (Rust, Python, TypeScript, Go) and generates comprehensive API documentation. Supports OpenAPI/Swagger spec output for HTTP APIs, rustdoc-style for libraries, and Markdown for general use. Extracts doc comments, type signatures, and endpoint routing annotations.

## How It Works

1. **Parse** — Extracts public function/type signatures, doc comments, generics, and trait bounds from source code
2. **Annotate** — Recognizes common annotations (`#[openapi]`, `#[derive]`, `#[tokio::main]`, Python decorators, JSDoc tags)
3. **Structure** — Groups by module/namespace, identifies HTTP endpoints (method + path), request/response types
4. **Render** — Outputs in the requested format with cross-references, examples, and type definitions

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | Source code to document |
| `language` | string | Programming language (rust, python, ts, go) |
| `output_format` | string | Output format: `markdown`, `openapi-json`, `openapi-yaml` (default: `markdown`) |
| `include_private` | boolean | Optional: include non-public items (default: false) |
| `title` | string | Optional: documentation title |

## Example

```json
{
  "code": "/// Retrieve a user by their unique identifier.\n///\n/// Returns `None` if no user with the given `user_id` exists.\n///\n/// # Errors\n/// Returns `DatabaseError` if the query fails.\npub async fn get_user(\n    pool: &PgPool,\n    user_id: Uuid,\n) -> Result<Option<User>, DatabaseError> {\n    sqlx::query_as!(\n        User,\n        \"SELECT id, name, email, created_at FROM users WHERE id = $1\",\n        user_id\n    )\n    .fetch_optional(pool)\n    .await\n    .map_err(DatabaseError::from)\n}",
  "language": "rust",
  "output_format": "markdown",
  "title": "User Service API"
}
```

**Example output (abbreviated):**

```markdown
# User Service API

## `get_user`

Retrieve a user by their unique identifier.

**Signature:**

```rust
pub async fn get_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<User>, DatabaseError>
```

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `pool` | `&PgPool` | Database connection pool |
| `user_id` | `Uuid` | The user's UUID |

**Returns:** `Option<User>` — `None` if no user exists with the given ID

**Errors:** `DatabaseError` — if the database query fails
```
