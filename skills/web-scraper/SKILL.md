---
name: web-scraper
description: Extracts, cleans, and structures content from web pages by URL or HTML input
version: 1.0.0
author: go-on-team
tags: [web, scraping, html, data-extraction, url]
min_go_on_version: 1.0.0
---

# Web Scraper Skill

Extracts meaningful content from web pages — article text, tables, metadata, links, and structured data — given a URL or raw HTML. Handles pagination, JavaScript-rendered content hints, and common anti-scraping patterns via fallback extraction strategies.

## How It Works

1. **Fetch** — Accepts a URL for live fetching or raw HTML for offline processing
2. **Parse** — Extracts the DOM, removes boilerplate (nav, ads, scripts, styles), identifies the main content area
3. **Structure** — Detects content type (article, listing, table, API response) and structures accordingly
4. **Clean** — Normalizes whitespace, resolves relative URLs, extracts metadata (title, description, author, publish date)

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `url` | string | Target URL to scrape (mutually exclusive with `html`) |
| `html` | string | Raw HTML to process (mutually exclusive with `url`) |
| `extract` | string | Content to extract: `article`, `metadata`, `links`, `tables`, `all` (default: `article`) |
| `max_length` | integer | Optional: maximum characters in output (default: 10000) |
| `include_raw_html` | boolean | Optional: include cleaned HTML alongside text (default: false) |

## Example

```json
{
  "url": "https://example.com/blog/rust-async-await-guide",
  "extract": "article",
  "max_length": 5000
}
```

**Example output (abbreviated):**

```json
{
  "url": "https://example.com/blog/rust-async-await-guide",
  "title": "A Practical Guide to Rust Async/Await",
  "author": "Jane Developer",
  "publish_date": "2026-06-15",
  "content": "Async/await in Rust provides zero-cost abstractions for concurrent programming...",
  "word_count": 1247,
  "links": [
    {"text": "Tokio tutorial", "href": "https://tokio.rs/tutorial"},
    {"text": "Async book", "href": "https://rust-lang.github.io/async-book/"}
  ]
}
```
