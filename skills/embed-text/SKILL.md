---
name: embed-text
description: Generate a semantic embedding/vector representation of text for similarity search
version: 1.0.0
author: go-on-team
min_go_on_version: 1.0.0
---

# Embed Text Skill

Extracts the semantic essence of text and returns a structured representation suitable for similarity matching and retrieval.

## How It Works

1. **Parse input** — Reads the text to analyze
2. **Semantic extraction** — Identifies key semantic dimensions, important terms, domain context, and sentiment
3. **Structure output** — Returns a structured JSON object with dimensions, keywords, domain, and sentiment

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Text to embed/analyze semantically |

## Example

```
Input: "The new authentication middleware uses JWT tokens with 256-bit encryption and supports OAuth2 refresh token rotation."
```

## Example Output

```json
{
  "dimensions": [
    {"name": "security-level", "value": "high"},
    {"name": "protocol-type", "value": "authentication"},
    {"name": "implementation-scope", "value": "middleware"},
    {"name": "crypto-strength", "value": "256-bit"},
    {"name": "standard-compliance", "value": "OAuth2"}
  ],
  "keywords": ["JWT", "authentication", "middleware", "OAuth2", "refresh token rotation", "256-bit encryption"],
  "domain": "cybersecurity / authentication",
  "sentiment": "neutral"
}
```

---

Analyze the following text and extract its semantic essence. Return a structured representation:

Text:
```
{{input}}
```

Return:
- `dimensions`: 5 key semantic dimensions
- `keywords`: Important terms and concepts
- `domain`: The domain/field of the text
- `sentiment`: Overall sentiment (positive/negative/neutral)
