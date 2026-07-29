---
name: analyze-text
description: Classify text into predefined categories or generate semantic embeddings/vector representations for similarity search
version: 1.0.0
author: go-on-team
min_go_on_version: 1.0.0
---

# Analyze Text Skill

Analyzes input text in two modes: `classify` for categorizing text with confidence scores, and `embed` for extracting semantic representations suitable for similarity matching and retrieval.

## Mode: `classify`

Classifies input text into one or more predefined categories, returning confidence scores and reasoning for each classification.

### How It Works

1. **Parse input** — Reads the text to classify
2. **Category matching** — Evaluates the text against known categories using semantic understanding
3. **Confidence scoring** — Assigns a confidence score (0.0–1.0) for the primary and alternative categories
4. **Format output** — Returns a structured JSON result with category, confidence, alternatives, and reasoning

### Input Schema (classify)

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Text to classify |

### Example (classify)

**Input:**
```
The server returned a 500 error when processing the payment endpoint
```

**Output:**
```json
{
  "category": "bug-report",
  "confidence": 0.92,
  "alternative_categories": [
    {"category": "error-log", "confidence": 0.65},
    {"category": "support-ticket", "confidence": 0.40}
  ],
  "reasoning": "Describes a specific HTTP 500 server error on a named endpoint, characteristic of a bug report."
}
```

---

## Mode: `embed`

Extracts the semantic essence of text and returns a structured representation suitable for similarity matching and retrieval.

### How It Works

1. **Parse input** — Reads the text to analyze
2. **Semantic extraction** — Identifies key semantic dimensions, important terms, domain context, and sentiment
3. **Structure output** — Returns a structured JSON object with dimensions, keywords, domain, and sentiment

### Input Schema (embed)

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Text to embed/analyze semantically |

### Example (embed)

**Input:**
```
The new authentication middleware uses JWT tokens with 256-bit encryption and supports OAuth2 refresh token rotation.
```

**Output:**
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
