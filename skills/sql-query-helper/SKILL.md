---
name: sql-query-helper
description: Generates, explains, optimizes, and migrates SQL queries across multiple database dialects
version: 1.0.0
author: go-on-team
tags: [sql, database, query, postgres, mysql, sqlite, migration]
min_go_on_version: 1.0.0
---

# SQL Query Helper Skill

Assists with all SQL-related tasks: writing queries from natural language, explaining what existing queries do, optimizing slow queries, generating schema migrations, and translating between database dialects (PostgreSQL, MySQL, SQLite, BigQuery, Snowflake).

## How It Works

1. **Understand** — Accepts a natural-language description or an existing SQL query
2. **Generate** — Produces correct, idiomatic SQL for the target dialect with appropriate indexing hints and join strategies
3. **Explain** — Annotates queries with logical operator breakdown, estimated complexity (sequential scan vs index scan), and data flow
4. **Optimize** — Identifies anti-patterns (SELECT *, N+1 queries, missing JOIN conditions, implicit type coercions) and suggests rewrites
5. **Translate** — Converts between dialects, handling syntax differences (LIMIT vs TOP, ILIKE vs LOWER, JSON operators)

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `description` | string | Natural-language description of what the query should do |
| `sql` | string | Optional: existing SQL to explain/optimize instead of generating new |
| `dialect` | string | Target SQL dialect: `postgres`, `mysql`, `sqlite`, `bigquery`, `snowflake` (default: `postgres`) |
| `schema` | string | Optional: table schema context for accurate generation |
| `action` | string | Action: `generate`, `explain`, `optimize`, `translate` (default: `generate`) |
| `target_dialect` | string | Required when `action=translate`: destination dialect |

## Example

```json
{
  "description": "Find the top 10 customers by total order value in the last 30 days, including their email and order count",
  "dialect": "postgres",
  "action": "generate",
  "schema": "customers(id, email, name, created_at)\norders(id, customer_id, total, ordered_at)"
}
```

**Example output:**

```sql
-- Generated for PostgreSQL
-- Estimated complexity: Index Scan (efficient with proper indexes)
SELECT
    c.email,
    c.name,
    COUNT(o.id) AS order_count,
    SUM(o.total) AS total_spent
FROM customers c
JOIN orders o ON o.customer_id = c.id
WHERE o.ordered_at >= NOW() - INTERVAL '30 days'
GROUP BY c.id, c.email, c.name
ORDER BY total_spent DESC
LIMIT 10;

-- Recommended indexes:
-- CREATE INDEX idx_orders_customer_date ON orders(customer_id, ordered_at);
```
