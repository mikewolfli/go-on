---
name: classify-text
description: Classify text into predefined categories with confidence scores
version: 1.0.0
---

# Classify Text Skill

Classifies input text into one or more predefined categories, returning confidence scores and reasoning for each classification.

## How It Works

1. **Parse input** — Reads the text to classify
2. **Category matching** — Evaluates the text against known categories using semantic understanding
3. **Confidence scoring** — Assigns a confidence score (0.0–1.0) for the primary and alternative categories
4. **Format output** — Returns a structured JSON result with category, confidence, alternatives, and reasoning

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Text to classify |

## Example

```
Input: "The server returned a 500 error when processing the payment endpoint"
```

## Example Output

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

Classify the following text into one or more of the predefined categories.

Text to classify:
```
{{input}}
```

Return a JSON object with:
- `category`: The best matching category
- `confidence`: A score from 0.0 to 1.0
- `alternative_categories`: Other possible categories with scores
- `reasoning`: Brief explanation of why this classification was chosen
