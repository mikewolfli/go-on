---
name: translate-text
description: Translate text between languages with natural-sounding results
version: 1.0.0
---

# Translate Text Skill

Translates text between languages while preserving formatting, code blocks, technical terminology, and idiomatic meaning.

## How It Works

1. **Parse input** — Extracts the source language, target language, and text from the input
2. **Detect language (if needed)** — If no language pair is specified, auto-detects the source language
3. **Translate** — Translates the text preserving formatting, code blocks, technical terms, and proper nouns
4. **Format output** — Returns the translated text

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Translation request in format: `source_lang → target_lang | text` |

## Example

```
Input: en → zh | The quick brown fox jumps over the lazy dog.
```

## Example Output

```
敏捷的棕色狐狸跃过了懒狗。
```

---

Translate the following text to the target language. Preserve formatting, code blocks, and technical terms when appropriate.

Input:
```
{{input}}
```

The input format is: `source_lang → target_lang | text`
If no language pair is specified, auto-detect the source and ask for the target.
