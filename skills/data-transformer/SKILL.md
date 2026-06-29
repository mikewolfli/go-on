---
name: data-transformer
description: Converts data between structured formats (JSON, CSV, YAML, TOML, XML) with schema mapping and validation
version: 1.0.0
author: go-on-team
tags: [data, conversion, json, csv, yaml, toml, xml, etl]
min_go_on_version: 1.0.0
---

# Data Transformer Skill

Converts structured data between common serialization formats (JSON, CSV, YAML, TOML, XML) with optional field mapping, type coercion, filtering, sorting, and validation. Supports nested flattening, array expansion, and custom delimiter configuration.

## How It Works

1. **Parse** — Detects the input format and deserializes the data into an internal representation
2. **Map** — Applies optional field renames, type coercions, default values, and filter predicates
3. **Transform** — Performs structural transformations (flatten nested objects, expand arrays, pivot/unpivot tables)
4. **Serialize** — Outputs in the requested format with configurable indentation, quoting, and encoding options

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Source data as a string |
| `input_format` | string | Input format: `json`, `csv`, `yaml`, `toml`, `xml` |
| `output_format` | string | Output format: `json`, `csv`, `yaml`, `toml`, `xml` |
| `field_mapping` | object | Optional: map of `{source_field: target_field}` renames |
| `filter` | string | Optional: filter expression (e.g. `age > 18`) |
| `pretty` | boolean | Optional: pretty-print output (default: true) |

## Example

```json
{
  "input": "name,age,email\nAlice,30,alice@example.com\nBob,25,bob@example.com\nCharlie,35,charlie@example.com",
  "input_format": "csv",
  "output_format": "json",
  "pretty": true
}
```

**Example output:**

```json
[
  { "name": "Alice", "age": 30, "email": "alice@example.com" },
  { "name": "Bob", "age": 25, "email": "bob@example.com" },
  { "name": "Charlie", "age": 35, "email": "charlie@example.com" }
]
```

## Advanced Example: Nested Flatten

Input (JSON with nested `address` object) → output (flat CSV with `address.street`, `address.city` columns):

```json
{
  "input": "[{\"name\":\"Alice\",\"address\":{\"street\":\"123 Main\",\"city\":\"Springfield\"}}]",
  "input_format": "json",
  "output_format": "csv",
  "field_mapping": { "address.street": "street", "address.city": "city" }
}
```
