---
name: knowledge-retriever
description: Retrieves relevant knowledge from project documentation, code comments, and README files
version: 1.0.0
author: go-on-team
tags: [knowledge, retrieval, documentation, search, context]
min_go_on_version: 1.0.0
---

# Knowledge Retriever Skill

Scans project documentation, comments, READMEs, and design docs to surface the most relevant information for a given query. Useful for onboarding, answering architecture questions, or gathering context before making changes.

## How It Works

1. **Index** — The skill crawls known documentation files (`README.md`, `CONTRIBUTING.md`, `ARCHITECTURE.md`, `CHANGELOG.md`, doc-comments in source, and any `docs/` directory) and builds a lightweight search index.
2. **Query** — Your natural-language query is matched against the index using keyword and semantic similarity.
3. **Rank** — Results are scored by relevance and returned up to the requested `max_results`.
4. **Present** — Each result includes the source file path, a snippet of the surrounding context, and a relevance score.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `query` | string | Natural-language question or keywords to search for |
| `max_results` | integer | Maximum number of results to return (default: 5, max: 20) |

## Example

```json
{
  "query": "How does the authentication middleware work?",
  "max_results": 3
}
```

### Example Output

```json
{
  "results": [
    {
      "file": "docs/auth/overview.md",
      "snippet": "The JWT middleware is registered in src/main.rs on line 42. It extracts the Bearer token from the Authorization header...",
      "score": 0.94
    },
    {
      "file": "src/auth/middleware.rs",
      "snippet": "// validate_token decodes the JWT, checks expiry, and injects the Claims into the request context.",
      "score": 0.87
    },
    {
      "file": "README.md",
      "snippet": "Authentication: set AUTH_SECRET env var and include an Authorization: Bearer <token> header on protected routes.",
      "score": 0.72
    }
  ]
}
```
