---
name: data-pipeline-optimizer
description: Analyzes ETL and data pipeline code for optimization opportunities across Python (Pandas, PySpark), Rust (polars, datafusion), SQL, and general pipeline descriptions
version: 1.0.0
author: go-on-team
tags: [etl, pipeline, data, optimization, pandas, pyspark, polars, datafusion, sql, performance]
min_go_on_version: 1.0.0
---

# Data Pipeline Optimizer Skill

Analyzes ETL and data pipeline code — or natural-language pipeline descriptions — and returns ranked optimization suggestions with estimated performance impact. Supports Python (Pandas, PySpark), Rust (polars, datafusion), SQL, and arbitrary pipeline descriptions.

## How It Works

1. **Parse input** — Accept pipeline code (Python, Rust, SQL) or a natural-language pipeline description, along with optional metadata (data volume, cluster size, current runtime).
2. **Analyze for bottlenecks** — Scan for six categories of optimization opportunity:
   - **Unnecessary shuffling**: Redundant `orderBy`, `sort`, `repartition` calls, cross-join followed by filter instead of inner join.
   - **Missing partitioning**: Full table scans where partition pruning is possible, unsorted data in window functions.
   - **Inefficient join strategies**: Broadcast joins where both sides are large, sort-merge joins on skewed keys, missing bloom filters.
   - **Serialization bottlenecks**: Row-by-row Python UDFs on large DataFrames, missing `mapInPandas`/`vectorized` usage, excessive `collect()` calls.
   - **Memory pressure points**: Unbounded aggregations, lack of spilling config, large shuffles without disk spill tuning.
   - **Parallelism gaps**: Single-partition reads on large files, insufficient `spark.sql.shuffle.partitions`, hardcoded executor counts.
3. **Rank results** — Each finding is scored by estimated performance impact (`high`, `medium`, `low`) and implementation difficulty, then returned as a sorted list.
4. **Generate output** — Return structured JSON with a summary, per-finding recommendations with rationale, code snippets (before/after), and estimated speedup range.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `pipeline_code` | string | Pipeline code in Python, Rust, or SQL. Can also be a natural-language description of the pipeline steps |
| `language` | string | Language or framework: `pandas`, `pyspark`, `polars`, `datafusion`, `sql`, or `description` |
| `data_volume` | string | Optional: estimated data volume (e.g. `"10GB"`, `"5M rows"`, `"1TB"`) |
| `cluster_size` | string | Optional: current cluster or executor configuration (e.g. `"4 nodes x 16GB"`, `"8 executors"`) |
| `current_runtime` | string | Optional: observed runtime for baseline comparison (e.g. `"45 min"`, `"2.3 hours"`) |

## Example

```json
{
  "pipeline_code": "df = spark.read.parquet(\"events/\")\ndf_agg = df.groupBy(\"user_id\").agg(sum(\"amount\").alias(\"total\"))\ndf_sorted = df_agg.orderBy(\"total\")\ndf_joined = df_sorted.join(df_users, \"user_id\")\ndf_joined.show()",
  "language": "pyspark",
  "data_volume": "500GB",
  "cluster_size": "8 executors x 32GB",
  "current_runtime": "47 min"
}
```

**Example output:**

```json
{
  "summary": "Found 4 optimization opportunities with estimated total speedup of 3–8×",
  "current_runtime": "47 min",
  "estimated_optimized_runtime": "6–16 min",
  "findings": [
    {
      "category": "unnecessary_shuffling",
      "impact": "high",
      "difficulty": "easy",
      "estimated_speedup": "1.5–3×",
      "title": "Premature global sort before join",
      "description": "`orderBy(\"total\")` triggers a full shuffle before the join, which will re-shuffle the same data on `user_id`.",
      "code_before": "df_agg.orderBy(\"total\").join(df_users, \"user_id\")",
      "code_after": "df_agg.join(df_users, \"user_id\").orderBy(\"total\")",
      "rationale": "Moving `orderBy` after the join eliminates one full shuffle (200 partitions × 500 GB). Only the joined result (typically smaller) is sorted."
    },
    {
      "category": "missing_partitioning",
      "impact": "high",
      "difficulty": "medium",
      "estimated_speedup": "2–4×",
      "title": "Partition pruning opportunity on `events/`",
      "description": "Reading all of `events/` without filtering. If data is partitioned by `date` or `event_type`, pruning could skip most files.",
      "code_before": "spark.read.parquet(\"events/\")",
      "code_after": "spark.read.parquet(\"events/\").filter(\"event_date >= '2024-01-01'\")",
      "rationale": "Partition pruning reduces scan size from full table to relevant partitions — critical at 500 GB scale."
    },
    {
      "category": "inefficient_join",
      "impact": "medium",
      "difficulty": "easy",
      "estimated_speedup": "1.2–2×",
      "title": "Skew-aware join hint recommended",
      "description": "Join on `user_id` may suffer from key skew (power users with many events). No skew hint or salting applied.",
      "code_before": "df_joined = df_sorted.join(df_users, \"user_id\")",
      "code_after": "df_joined = df_sorted.hint(\"skew\").join(df_users, \"user_id\")",
      "rationale": "Skew hint enables Spark to split imbalanced partitions, preventing straggler tasks from dominating runtime."
    },
    {
      "category": "serialization_bottleneck",
      "impact": "low",
      "difficulty": "hard",
      "estimated_speedup": "1.1–1.5×",
      "title": "Consider Tungsten serialization for UDF-heavy stages",
      "description": "If any Python UDFs are applied downstream, consider replacing with native Spark SQL expressions or Pandas UDFs.",
      "code_before": "df.withColumn(\"bucket\", udf(lambda x: x // 100, IntegerType())(\"amount\"))",
      "code_after": "df.withColumn(\"bucket\", (F.col(\"amount\") / 100).cast(\"int\"))",
      "rationale": "Python UDFs serialize each row across the JVM boundary. Native Spark SQL expressions run in the execution engine with no serialization overhead."
    }
  ]
}
```
