use super::*;

pub(super) async fn handle_learning_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .max(1);
    let guardrail = summarize_learning_guardrail(window, &params)?;
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("learning.summary", task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("learning.summary", task, &params, &learning_profile);
    let knowledge_bus =
        read_latest_artifact::<KnowledgeBusArtifact>(&ledger, "spec", "latest-knowledge.json");
    let Some(bus) = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    ) else {
        return send_result(
            server,
            request_id,
            json!({
                "ok": true,
                "summary": {"sampled_events": 0, "totals": {}, "averages": {}, "rates": {}},
                "guardrail": guardrail,
                "knowledge": knowledge_bus.as_ref().map(|bus| json!({
                    "total_events": bus.total_events,
                    "sampled_events": bus.events.len().min(window),
                    "latest_generated_at": bus.generated_at,
                    "recent": bus.events.iter().rev().take(window).cloned().collect::<Vec<_>>()
                })).unwrap_or_else(|| json!({"total_events": 0, "sampled_events": 0, "recent": []})),
                "events": [],
                "learning_profile": learning_profile,
                "knowledge_refinement": knowledge_refinement,
            }),
        )
        .await;
    };

    let events = bus
        .events
        .iter()
        .rev()
        .take(window)
        .cloned()
        .collect::<Vec<_>>();
    let count = events.len().max(1);
    let avg_success = events
        .iter()
        .map(|item| item.predicted_success_rate as f64)
        .sum::<f64>()
        / count as f64;
    let avg_speedup = events.iter().map(|item| item.parallel_speedup).sum::<f64>() / count as f64;
    let avg_risk = events.iter().map(|item| item.risk_score).sum::<f64>() / count as f64;
    let failover_total = events
        .iter()
        .map(|item| item.failover_count as u64)
        .sum::<u64>();
    let avg_rounds = events
        .iter()
        .map(|item| item.clarification_rounds as f64)
        .sum::<f64>()
        / count as f64;
    let avg_quality = events
        .iter()
        .map(|item| item.clarification_quality_score)
        .sum::<f64>()
        / count as f64;
    let requirement_change_total = events
        .iter()
        .map(|item| item.requirement_change_count as u64)
        .sum::<u64>();
    let gates_pass_rate = events.iter().filter(|item| item.gates_ok).count() as f64 / count as f64;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "summary": {
                "total_events": bus.total_events,
                "sampled_events": events.len(),
                "latest_generated_at": bus.generated_at,
                "totals": {
                    "requirement_change_count": requirement_change_total,
                    "failover_count": failover_total,
                },
                "averages": {
                    "predicted_success_rate": avg_success,
                    "parallel_speedup": avg_speedup,
                    "risk_score": avg_risk,
                    "clarification_rounds": avg_rounds,
                    "clarification_quality_score": avg_quality,
                },
                "rates": {
                    "gates_pass_rate": gates_pass_rate,
                }
            },
            "guardrail": guardrail,
            "knowledge": knowledge_bus.as_ref().map(|bus| json!({
                "total_events": bus.total_events,
                "sampled_events": bus.events.len().min(window),
                "latest_generated_at": bus.generated_at,
                "recent": bus.events.iter().rev().take(window).cloned().collect::<Vec<_>>()
            })).unwrap_or_else(|| json!({"total_events": 0, "sampled_events": 0, "recent": []})),
            "events": events,
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}

#[derive(Debug, Clone, Copy)]
struct LearningGuardrailConfig {
    window: usize,
    min_samples: usize,
    dedup_similarity_threshold: f64,
    high_risk_threshold: f64,
    min_parseable_ratio: f64,
    min_quality_ratio: f64,
    cooldown_seconds: i64,
}

#[derive(Debug, Clone, Copy, Default)]
struct LearningGuardrailStats {
    records_total: usize,
    parseable_records: usize,
    parse_errors: usize,
    evidence_complete: usize,
    attributable: usize,
    high_risk_records: usize,
    high_risk_complete: usize,
    duplicate_records: usize,
    weighted_total: f64,
    weighted_pass: f64,
    last_high_risk_incomplete_at: i64,
}

fn parse_learning_guardrail_config(window: usize, params: &Value) -> LearningGuardrailConfig {
    LearningGuardrailConfig {
        window,
        min_samples: params
            .get("min_samples")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(8)
            .max(1),
        dedup_similarity_threshold: params
            .get("dedup_similarity_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.92)
            .clamp(0.75, 0.99),
        high_risk_threshold: params
            .get("high_risk_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.7)
            .clamp(0.3, 0.99),
        min_parseable_ratio: params
            .get("min_parseable_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(0.95)
            .clamp(0.5, 1.0),
        min_quality_ratio: params
            .get("min_quality_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(0.75)
            .clamp(0.4, 1.0),
        cooldown_seconds: params
            .get("cooldown_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(300)
            .max(0),
    }
}

fn extract_record_signature(record: &LearningRecord) -> String {
    match record {
        LearningRecord::Workflow(payload) => {
            let task = payload
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let executor = payload
                .get("executor")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("{}::{}", task.trim().to_ascii_lowercase(), executor)
        }
        LearningRecord::Pua(payload) => {
            let status = if payload.passed { "pass" } else { "fail" };
            format!(
                "{}::{}::{}",
                payload.stage.trim().to_ascii_lowercase(),
                status,
                payload.escalation_level
            )
        }
    }
}

fn signature_similarity(left: &str, right: &str) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }

    let lhs = left
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|item| !item.is_empty())
        .collect::<HashSet<_>>();
    let rhs = right
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|item| !item.is_empty())
        .collect::<HashSet<_>>();

    if lhs.is_empty() || rhs.is_empty() {
        return if left == right { 1.0 } else { 0.0 };
    }

    let overlap = lhs.intersection(&rhs).count() as f64;
    overlap / (lhs.len().max(rhs.len()) as f64)
}

fn scan_learning_records_with_parseability(
    window: usize,
) -> Result<(Vec<LearningRecord>, usize, usize)> {
    let storage_dir = Path::new(".goon").join("learning");
    let records_path = storage_dir.join(crate::pua::LEARNING_RECORDS_FILE);
    if !records_path.exists() {
        return Ok((Vec::new(), 0, 0));
    }

    let content = fs::read_to_string(&records_path)?;
    let mut parsed = Vec::new();
    let mut parse_errors = 0usize;
    let mut total_lines = 0usize;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        total_lines = total_lines.saturating_add(1);
        match serde_json::from_str::<LearningRecord>(trimmed) {
            Ok(record) => parsed.push(record),
            Err(_) => parse_errors = parse_errors.saturating_add(1),
        }
    }

    if parsed.len() > window {
        let split_at = parsed.len() - window;
        parsed = parsed.split_off(split_at);
    }

    Ok((parsed, total_lines, parse_errors))
}

fn summarize_learning_guardrail(window: usize, params: &Value) -> Result<Value> {
    let cfg = parse_learning_guardrail_config(window, params);
    let (records, total_lines, parse_errors) = scan_learning_records_with_parseability(cfg.window)?;

    let mut stats = LearningGuardrailStats {
        records_total: records.len(),
        parseable_records: records.len(),
        parse_errors,
        ..LearningGuardrailStats::default()
    };
    let mut signatures: Vec<String> = Vec::new();

    for record in &records {
        let signature = extract_record_signature(record);
        let duplicate = signatures.iter().any(|existing| {
            signature_similarity(existing, &signature) >= cfg.dedup_similarity_threshold
        });
        if duplicate {
            stats.duplicate_records = stats.duplicate_records.saturating_add(1);
        }

        let (evidence_complete, attributable, high_risk, generated_at) = match record {
            LearningRecord::Workflow(payload) => {
                let task_ok = payload
                    .get("task")
                    .and_then(Value::as_str)
                    .map(|item| !item.trim().is_empty())
                    .unwrap_or(false);
                let executor_ok = payload
                    .get("executor")
                    .and_then(Value::as_str)
                    .map(|item| !item.trim().is_empty())
                    .unwrap_or(false);
                let source_ok = payload
                    .get("source")
                    .and_then(Value::as_str)
                    .map(|item| !item.trim().is_empty())
                    .unwrap_or(false);
                let complexity_ok = payload.get("complexity").and_then(Value::as_u64).is_some();
                let totals_ok = payload
                    .get("subtasks_total")
                    .and_then(Value::as_u64)
                    .is_some();
                let risk_score = payload
                    .get("risk_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let failed = payload
                    .get("subtasks_failed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0;
                let gates_ok = payload
                    .get("gates_ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let generated_at = payload
                    .get("generated_at")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                (
                    task_ok && executor_ok && source_ok && complexity_ok && totals_ok,
                    task_ok && executor_ok,
                    risk_score >= cfg.high_risk_threshold || failed || !gates_ok,
                    generated_at,
                )
            }
            LearningRecord::Pua(payload) => {
                let stage_ok = !payload.stage.trim().is_empty();
                let checks_ok = !payload
                    .missing_checks
                    .iter()
                    .any(|item| item.trim().is_empty());
                (
                    stage_ok && checks_ok,
                    stage_ok,
                    !payload.passed || payload.escalation_level >= 2,
                    0,
                )
            }
        };

        if evidence_complete {
            stats.evidence_complete = stats.evidence_complete.saturating_add(1);
        }
        if attributable {
            stats.attributable = stats.attributable.saturating_add(1);
        }
        if high_risk {
            stats.high_risk_records = stats.high_risk_records.saturating_add(1);
            if evidence_complete && attributable {
                stats.high_risk_complete = stats.high_risk_complete.saturating_add(1);
            }
            if !(evidence_complete && attributable)
                && generated_at > stats.last_high_risk_incomplete_at
            {
                stats.last_high_risk_incomplete_at = generated_at;
            }
        }

        let weight = if high_risk { 2.0 } else { 1.0 };
        stats.weighted_total += weight;
        if evidence_complete && attributable && !duplicate {
            stats.weighted_pass += weight;
        }

        signatures.push(signature);
    }

    let parseable_ratio = if total_lines == 0 {
        1.0
    } else {
        stats.parseable_records as f64 / total_lines as f64
    };
    let quality_ratio = if stats.weighted_total <= f64::EPSILON {
        1.0
    } else {
        stats.weighted_pass / stats.weighted_total
    };
    let high_risk_coverage = if stats.high_risk_records == 0 {
        1.0
    } else {
        stats.high_risk_complete as f64 / stats.high_risk_records as f64
    };
    let dedup_ratio = if stats.records_total == 0 {
        0.0
    } else {
        stats.duplicate_records as f64 / stats.records_total as f64
    };

    let sample_ready = stats.records_total >= cfg.min_samples;
    let now_ts = crate::acp::prelude::now_ts();
    let cooldown_active = stats.last_high_risk_incomplete_at > 0
        && (now_ts - stats.last_high_risk_incomplete_at) <= cfg.cooldown_seconds;

    let mut warnings = Vec::new();
    if !sample_ready {
        warnings.push(format!(
            "learning sample volume below threshold: {}/{}",
            stats.records_total, cfg.min_samples
        ));
    }
    if parseable_ratio < cfg.min_parseable_ratio {
        warnings.push(format!(
            "learning parseability below threshold: {:.2}% < {:.2}%",
            parseable_ratio * 100.0,
            cfg.min_parseable_ratio * 100.0
        ));
    }
    if quality_ratio < cfg.min_quality_ratio {
        warnings.push(format!(
            "learning quality gate below threshold: {:.2}% < {:.2}%",
            quality_ratio * 100.0,
            cfg.min_quality_ratio * 100.0
        ));
    }
    if cooldown_active {
        warnings.push(
            "learning cooldown active due to recent high-risk incomplete evidence".to_string(),
        );
    }

    let status = if !warnings.is_empty() {
        if !sample_ready
            || parseable_ratio < cfg.min_parseable_ratio
            || quality_ratio < cfg.min_quality_ratio
        {
            "block"
        } else {
            "warn"
        }
    } else {
        "pass"
    };

    Ok(json!({
        "status": status,
        "window": cfg.window,
        "sample_ready": sample_ready,
        "cooldown_active": cooldown_active,
        "thresholds": {
            "min_samples": cfg.min_samples,
            "dedup_similarity_threshold": cfg.dedup_similarity_threshold,
            "high_risk_threshold": cfg.high_risk_threshold,
            "min_parseable_ratio": cfg.min_parseable_ratio,
            "min_quality_ratio": cfg.min_quality_ratio,
            "cooldown_seconds": cfg.cooldown_seconds,
        },
        "stats": {
            "records_total": stats.records_total,
            "parseable_records": stats.parseable_records,
            "parse_errors": stats.parse_errors,
            "evidence_complete": stats.evidence_complete,
            "attributable": stats.attributable,
            "high_risk_records": stats.high_risk_records,
            "high_risk_complete": stats.high_risk_complete,
            "duplicate_records": stats.duplicate_records,
            "parseable_ratio": parseable_ratio,
            "quality_ratio": quality_ratio,
            "high_risk_coverage": high_risk_coverage,
            "dedup_ratio": dedup_ratio,
        },
        "warnings": warnings,
    }))
}

pub(super) async fn handle_learning_guardrail(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(50)
        .max(1);
    let guardrail = summarize_learning_guardrail(window, &params)?;
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("learning.guardrail", task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("learning.guardrail", task, &params, &learning_profile);
    send_result(server, request_id, json!({ "ok": true, "guardrail": guardrail, "learning_profile": learning_profile, "knowledge_refinement": knowledge_refinement })).await
}

pub(super) async fn handle_learning_replay(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .max(1);

    let storage_dir = Path::new(".goon").join("learning");
    let records = load_learning_records(&storage_dir, window).unwrap_or_default();
    let workflow_count = records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Workflow(_)))
        .count();
    let pua_count = records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Pua(_)))
        .count();
    let learning_bus = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    );
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("learning.replay", task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("learning.replay", task, &params, &learning_profile);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "replay": {
                "source": storage_dir.display().to_string(),
                "window": window,
                "records_total": records.len(),
                "workflow_records": workflow_count,
                "pua_records": pua_count,
                "records": records,
                "learning_bus": learning_bus.as_ref().map(|bus| json!({
                    "generated_at": bus.generated_at,
                    "total_events": bus.total_events,
                    "sampled_events": bus.events.len().min(window),
                    "recent": bus.events.iter().rev().take(window).cloned().collect::<Vec<_>>()
                })).unwrap_or_else(|| json!({
                    "generated_at": 0,
                    "total_events": 0,
                    "sampled_events": 0,
                    "recent": []
                }))
            },
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}

const KNOWLEDGE_TOMBSTONE_FILE: &str = "tombstones.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnowledgeTombstoneEntry {
    timestamp: i64,
    key: String,
    reason: String,
    replaced_by: Option<String>,
    superseded: Value,
}

fn knowledge_storage_dir() -> PathBuf {
    Path::new(".goon").join("knowledge")
}

fn knowledge_tombstone_path() -> PathBuf {
    knowledge_storage_dir().join(KNOWLEDGE_TOMBSTONE_FILE)
}

fn load_knowledge_tombstones(limit: usize) -> Vec<KnowledgeTombstoneEntry> {
    let path = knowledge_tombstone_path();
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut items = raw
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            serde_json::from_str::<KnowledgeTombstoneEntry>(trimmed).ok()
        })
        .collect::<Vec<_>>();

    if items.len() > limit {
        let split_at = items.len() - limit;
        items = items.split_off(split_at);
    }
    items
}

fn append_knowledge_tombstones(entries: &[KnowledgeTombstoneEntry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let dir = knowledge_storage_dir();
    fs::create_dir_all(&dir)?;
    let path = knowledge_tombstone_path();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    for entry in entries {
        let encoded = serde_json::to_string(entry)?;
        file.write_all(encoded.as_bytes())?;
        file.write_all(b"\n")?;
    }

    Ok(())
}

fn detect_knowledge_conflicts(
    events: &[crate::reinforcement::KnowledgeInsightArtifact],
    apply_tombstone: bool,
) -> (Vec<Value>, Vec<KnowledgeTombstoneEntry>) {
    let mut grouped: HashMap<String, Vec<&crate::reinforcement::KnowledgeInsightArtifact>> =
        HashMap::new();

    for event in events {
        let key = format!(
            "{}::{}",
            event.task.trim().to_ascii_lowercase(),
            event.phase.trim().to_ascii_lowercase()
        );
        grouped.entry(key).or_default().push(event);
    }

    let mut conflicts = Vec::new();
    let mut tombstones = Vec::new();

    for (key, mut items) in grouped {
        if items.len() < 2 {
            continue;
        }
        items.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.generated_at.cmp(&left.generated_at))
        });

        let primary = items[0];
        let conflicting = items
            .iter()
            .skip(1)
            .filter(|item| {
                item.agent != primary.agent
                    || item.response_excerpt != primary.response_excerpt
                    || (primary.confidence - item.confidence).abs() > f64::EPSILON
            })
            .copied()
            .collect::<Vec<_>>();

        if conflicting.is_empty() {
            continue;
        }

        conflicts.push(json!({
            "key": key,
            "primary": {
                "agent": primary.agent,
                "confidence": primary.confidence,
                "source": primary.source,
                "generated_at": primary.generated_at,
            },
            "conflicting": conflicting.iter().map(|item| json!({
                "agent": item.agent,
                "confidence": item.confidence,
                "source": item.source,
                "generated_at": item.generated_at,
            })).collect::<Vec<_>>(),
        }));

        if apply_tombstone {
            for item in conflicting {
                tombstones.push(KnowledgeTombstoneEntry {
                    timestamp: crate::acp::prelude::now_ts(),
                    key: key.clone(),
                    reason: "knowledge_conflict_superseded".to_string(),
                    replaced_by: Some(primary.agent.clone()),
                    superseded: json!({
                        "task": item.task,
                        "phase": item.phase,
                        "agent": item.agent,
                        "source": item.source,
                        "confidence": item.confidence,
                        "generated_at": item.generated_at,
                        "response_excerpt": item.response_excerpt,
                    }),
                });
            }
        }
    }

    conflicts.sort_by(|left, right| {
        left.get("key")
            .and_then(Value::as_str)
            .cmp(&right.get("key").and_then(Value::as_str))
    });

    (conflicts, tombstones)
}

pub(super) async fn handle_knowledge_distill(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .max(1);
    let strategy_limit = params
        .get("strategy_limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(12)
        .clamp(1, 64);
    let tombstone_limit = params
        .get("tombstone_limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .clamp(1, 200);
    let apply_tombstone = params
        .get("apply_tombstone")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let learning_dir = Path::new(".goon").join("learning");
    let evidence_records = load_learning_records(&learning_dir, window).unwrap_or_default();
    let workflow_records = evidence_records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Workflow(_)))
        .count();
    let pua_records = evidence_records
        .iter()
        .filter(|record| matches!(record, LearningRecord::Pua(_)))
        .count();

    let knowledge_bus =
        read_latest_artifact::<KnowledgeBusArtifact>(&ledger, "spec", "latest-knowledge.json");
    let summary_events = knowledge_bus
        .as_ref()
        .map(|bus| {
            bus.events
                .iter()
                .rev()
                .take(window)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let (conflicts, new_tombstones) = detect_knowledge_conflicts(&summary_events, apply_tombstone);
    if apply_tombstone {
        append_knowledge_tombstones(&new_tombstones)?;
    }
    let tombstones = load_knowledge_tombstones(tombstone_limit);
    let task_ref = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("knowledge.distill", task_ref, &params);
    let knowledge_refinement = build_knowledge_refinement_profile(
        "knowledge.distill",
        task_ref,
        &params,
        &learning_profile,
    );

    let mut strategy_rules = Vec::new();
    for event in summary_events.iter().take(strategy_limit) {
        let then_action = event
            .reusable_insights
            .first()
            .cloned()
            .or_else(|| {
                event.verification_steps.first().map(|step| {
                    format!(
                        "Prioritize verification step '{}' for phase '{}'",
                        step, event.phase
                    )
                })
            })
            .unwrap_or_else(|| {
                format!(
                    "Use '{}' insights as baseline strategy for task '{}'",
                    event.agent, event.task
                )
            });

        strategy_rules.push(json!({
            "rule_id": format!("k-rule-{}", strategy_rules.len() + 1),
            "when": {
                "task": event.task,
                "phase": event.phase,
                "agent": event.agent,
            },
            "then": then_action,
            "confidence": event.confidence,
            "source": event.source,
        }));
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "distillation": {
                "window": window,
                "layers": {
                    "evidence": {
                        "source": learning_dir.display().to_string(),
                        "records_total": evidence_records.len(),
                        "workflow_records": workflow_records,
                        "pua_records": pua_records,
                        "records": evidence_records.into_iter().map(|record| serde_json::to_value(record).unwrap_or_else(|_| json!({}))).collect::<Vec<_>>()
                    },
                    "summary": {
                        "source": "spec/latest-knowledge.json",
                        "total_events": knowledge_bus.as_ref().map(|bus| bus.total_events).unwrap_or(0),
                        "sampled_events": summary_events.len(),
                        "latest_generated_at": knowledge_bus.as_ref().map(|bus| bus.generated_at).unwrap_or(0),
                        "recent": summary_events,
                    },
                    "strategy": {
                        "rules_total": strategy_rules.len(),
                        "rules": strategy_rules,
                    },
                    "conflicts": {
                        "count": conflicts.len(),
                        "items": conflicts,
                    },
                    "tombstones": {
                        "added_count": new_tombstones.len(),
                        "stored_count": tombstones.len(),
                        "items": tombstones,
                    }
                }
            },
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}

#[derive(Debug, Clone, Serialize)]
struct RlOfflineEvalSample {
    timestamp: i64,
    success: bool,
    latency_cost: f64,
    tool_error_rate: f64,
    safety_penalty: f64,
    reward: f64,
}

#[derive(Debug, Clone, Copy)]
struct RlRewardWeights {
    success: f64,
    latency: f64,
    tool_error: f64,
    safety: f64,
}

fn parse_rl_reward_weights(params: &Value) -> RlRewardWeights {
    RlRewardWeights {
        success: params
            .get("success_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.55)
            .clamp(0.0, 2.0),
        latency: params
            .get("latency_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.2)
            .clamp(0.0, 2.0),
        tool_error: params
            .get("tool_error_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.15)
            .clamp(0.0, 2.0),
        safety: params
            .get("safety_weight")
            .and_then(Value::as_f64)
            .unwrap_or(0.1)
            .clamp(0.0, 2.0),
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn collect_rl_offline_eval_samples(
    window: usize,
    weights: RlRewardWeights,
) -> Vec<RlOfflineEvalSample> {
    let learning_dir = Path::new(".goon").join("learning");
    let records = load_learning_records(&learning_dir, window).unwrap_or_default();

    records
        .into_iter()
        .filter_map(|record| match record {
            LearningRecord::Workflow(payload) => {
                let subtasks_total = payload
                    .get("subtasks_total")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1);
                let subtasks_failed = payload
                    .get("subtasks_failed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(subtasks_total);
                let success = subtasks_failed == 0;

                let explicit_duration_ms = payload
                    .get("duration_ms")
                    .or_else(|| payload.get("total_duration_ms"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .max(0.0);
                let latency_cost = if explicit_duration_ms <= f64::EPSILON {
                    0.0
                } else {
                    (explicit_duration_ms / 5000.0).clamp(0.0, 1.0)
                };

                let tool_error_rate =
                    (subtasks_failed as f64 / subtasks_total as f64).clamp(0.0, 1.0);
                let gates_ok = payload
                    .get("gates_ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let safety_penalty = if gates_ok { 0.0 } else { 1.0 };

                let reward = (weights.success * if success { 1.0 } else { 0.0 }
                    - weights.latency * latency_cost
                    - weights.tool_error * tool_error_rate
                    - weights.safety * safety_penalty)
                    .clamp(-1.0, 1.0);

                Some(RlOfflineEvalSample {
                    timestamp: payload
                        .get("generated_at")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    success,
                    latency_cost,
                    tool_error_rate,
                    safety_penalty,
                    reward,
                })
            }
            LearningRecord::Pua(_) => None,
        })
        .collect()
}

pub(super) fn build_rl_alignment_offline_eval_payload(params: &Value) -> Value {
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(120)
        .clamp(20, 2000);
    let pass_threshold = params
        .get("pass_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.05)
        .clamp(0.0, 0.5);
    let drift_threshold = params
        .get("drift_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.12)
        .clamp(0.01, 0.6);

    let weights = parse_rl_reward_weights(params);
    let mut samples = collect_rl_offline_eval_samples(window, weights);
    samples.sort_by_key(|sample| sample.timestamp);

    let (baseline_slice, candidate_slice) = if samples.len() < 2 {
        (&samples[..], &samples[..])
    } else {
        let split_index = ((samples.len() as f64) * 0.7).floor() as usize;
        let split_index = split_index.clamp(1, samples.len() - 1);
        samples.split_at(split_index)
    };

    let baseline_rewards = baseline_slice
        .iter()
        .map(|item| item.reward)
        .collect::<Vec<_>>();
    let candidate_rewards = candidate_slice
        .iter()
        .map(|item| item.reward)
        .collect::<Vec<_>>();
    let baseline_mean = mean(&baseline_rewards);
    let candidate_mean = mean(&candidate_rewards);
    let improvement = candidate_mean - baseline_mean;

    let baseline_safety = mean(
        &baseline_slice
            .iter()
            .map(|item| item.safety_penalty)
            .collect::<Vec<_>>(),
    );
    let candidate_safety = mean(
        &candidate_slice
            .iter()
            .map(|item| item.safety_penalty)
            .collect::<Vec<_>>(),
    );

    let recent_window = samples.len().clamp(1, 20);
    let recent_rewards = samples
        .iter()
        .rev()
        .take(recent_window)
        .map(|item| item.reward)
        .collect::<Vec<_>>();
    let historical_rewards = if samples.len() > recent_window {
        samples
            .iter()
            .take(samples.len() - recent_window)
            .map(|item| item.reward)
            .collect::<Vec<_>>()
    } else {
        baseline_rewards.clone()
    };
    let recent_mean = mean(&recent_rewards);
    let historical_mean = mean(&historical_rewards);
    let reward_drift = (recent_mean - historical_mean).abs();
    let drift_alert = reward_drift > drift_threshold;

    let enough_samples = samples.len() >= 20;
    let safe_to_promote = candidate_safety <= (baseline_safety + 0.05);
    let pass = enough_samples && improvement >= pass_threshold && safe_to_promote;
    let recommended_mode = if pass && !drift_alert {
        "adaptive"
    } else {
        "conservative"
    };

    let warnings = {
        let mut items = Vec::new();
        if !enough_samples {
            items.push(format!(
                "offline replay sample size below threshold: {} < 20",
                samples.len()
            ));
        }
        if improvement < pass_threshold {
            items.push(format!(
                "candidate reward uplift below threshold: {:.4} < {:.4}",
                improvement, pass_threshold
            ));
        }
        if !safe_to_promote {
            items.push(format!(
                "candidate safety penalty regressed: {:.4} > {:.4}",
                candidate_safety,
                baseline_safety + 0.05
            ));
        }
        if drift_alert {
            items.push(format!(
                "reward drift exceeds threshold: {:.4} > {:.4}",
                reward_drift, drift_threshold
            ));
        }
        items
    };

    json!({
        "ok": true,
        "offline_eval": {
            "window": window,
            "samples_total": samples.len(),
            "weights": {
                "success": weights.success,
                "latency": weights.latency,
                "tool_error": weights.tool_error,
                "safety": weights.safety,
            },
            "baseline": {
                "samples": baseline_slice.len(),
                "mean_reward": baseline_mean,
                "mean_safety_penalty": baseline_safety,
            },
            "candidate": {
                "samples": candidate_slice.len(),
                "mean_reward": candidate_mean,
                "mean_safety_penalty": candidate_safety,
            },
            "comparison": {
                "reward_uplift": improvement,
                "pass_threshold": pass_threshold,
                "passes": pass,
            },
            "drift": {
                "recent_mean": recent_mean,
                "historical_mean": historical_mean,
                "absolute_diff": reward_drift,
                "threshold": drift_threshold,
                "alert": drift_alert,
            },
            "decision": {
                "recommended_mode": recommended_mode,
                "fallback_triggered": !pass || drift_alert,
            },
            "warnings": warnings,
        }
    })
}

pub(super) async fn handle_rl_alignment_offline_eval(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("rl.alignment.offline_eval", task, &params);
    let knowledge_refinement = build_knowledge_refinement_profile(
        "rl.alignment.offline_eval",
        task,
        &params,
        &learning_profile,
    );
    let mut payload = build_rl_alignment_offline_eval_payload(&params);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("learning_profile".to_string(), learning_profile);
        obj.insert("knowledge_refinement".to_string(), knowledge_refinement);
    }
    send_result(server, request_id, payload).await
}

pub(super) async fn handle_phase_policy_replay(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(200)
        .max(1);
    let mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();

    let events = trace_events()
        .lock()
        .map(|guard| {
            guard
                .iter()
                .rev()
                .filter(|event| event.event_type == "phase.agent")
                .take(window)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut phase_stats: HashMap<String, (u64, u64, u64)> = HashMap::new();
    for event in &events {
        let entry = phase_stats.entry(event.phase.clone()).or_insert((0, 0, 0));
        entry.0 = entry.0.saturating_add(1);
        if event.status.eq_ignore_ascii_case("ok") {
            entry.1 = entry.1.saturating_add(1);
        }
        entry.2 = entry.2.saturating_add(event.duration_ms);
    }

    let mut ranked = phase_stats
        .iter()
        .map(|(phase, (attempts, successes, total_duration_ms))| {
            let success_rate = if *attempts == 0 {
                0.0
            } else {
                *successes as f64 / *attempts as f64
            };
            let avg_latency_ms = if *attempts == 0 {
                0.0
            } else {
                *total_duration_ms as f64 / *attempts as f64
            };
            let latency_factor = if avg_latency_ms <= f64::EPSILON {
                0.5
            } else {
                (1.0 / (1.0 + (avg_latency_ms / 5000.0))).clamp(0.0, 1.0)
            };
            let empirical_score = (0.75 * success_rate + 0.25 * latency_factor).clamp(0.0, 1.0);
            json!({
                "phase": phase,
                "attempts": attempts,
                "successes": successes,
                "success_rate": success_rate,
                "avg_latency_ms": avg_latency_ms,
                "empirical_score": empirical_score,
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .get("empirical_score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(
                &left
                    .get("empirical_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            )
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let candidate_phases = server
        .model_deps
        .flow_manager
        .as_ref()
        .map(|flow| flow.config().flow.phases.clone())
        .unwrap_or_default();
    let (controller_recommended, controller_snapshot) = server
        .online_controller
        .lock()
        .ok()
        .map(|ctrl| {
            (
                ctrl.recommend_phase(&candidate_phases),
                ctrl.phase_policy_snapshot(&candidate_phases),
            )
        })
        .unwrap_or((None, Vec::new()));
    let empirical_best = ranked
        .first()
        .and_then(|row| row.get("phase"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("phase.policy.replay", task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("phase.policy.replay", task, &params, &learning_profile);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "mode": mode,
            "sampled_events": events.len(),
            "candidate_phases": candidate_phases,
            "controller_recommended_phase": controller_recommended,
            "empirical_best_phase": empirical_best,
            "controller_phase_policy": controller_snapshot.into_iter().map(|(phase, mean_reward, reliability, pulls)| json!({
                "phase": phase,
                "mean_reward": mean_reward,
                "reliability": reliability,
                "pulls": pulls,
            })).collect::<Vec<_>>(),
            "phase_scores": ranked,
            "agreement": {
                "matches_empirical_best": controller_recommended.is_some() && controller_recommended == empirical_best,
            },
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}

pub(super) async fn handle_primary_secondary_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .max(1);
    let bus = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    );
    let policy = read_latest_artifact::<PrimarySecondaryPolicyArtifact>(
        &ledger,
        "spec",
        "latest-primary-secondary-policy.json",
    );
    let failover = read_latest_artifact::<PrimarySecondaryFailoverArtifact>(
        &ledger,
        "spec",
        "latest-primary-secondary-failover.json",
    );

    let events = bus
        .as_ref()
        .map(|bus| {
            bus.events
                .iter()
                .rev()
                .take(window)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let count = events.len().max(1);
    let avg_primary_stability = events
        .iter()
        .map(|item| item.primary_stability_score)
        .sum::<f64>()
        / count as f64;
    let avg_secondary_utilization = events
        .iter()
        .map(|item| item.secondary_utilization_rate)
        .sum::<f64>()
        / count as f64;
    let total_failovers = events
        .iter()
        .map(|item| item.failover_count as u64)
        .sum::<u64>();
    let mut root_causes = HashMap::new();
    for event in &events {
        if !event.failover_root_cause.is_empty() {
            *root_causes
                .entry(event.failover_root_cause.clone())
                .or_insert(0_u64) += 1;
        }
    }
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("primary_secondary.summary", task, &params);
    let knowledge_refinement = build_knowledge_refinement_profile(
        "primary_secondary.summary",
        task,
        &params,
        &learning_profile,
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "summary": {
                "total_events": events.len(),
                "averages": {
                    "primary_stability_score": avg_primary_stability,
                    "secondary_utilization_rate": avg_secondary_utilization,
                },
                "totals": {
                    "failover_count": total_failovers,
                },
                "failover_root_causes": root_causes,
                "latest_policy": policy,
                "latest_failover": failover,
            },
            "learning_profile": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }),
    )
    .await
}
