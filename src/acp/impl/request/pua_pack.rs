use super::*;

pub(super) fn pua_report_enabled(server: &AcpServer, params: &Option<Value>) -> bool {
    server.runtime_config.pua_report
        || params
            .as_ref()
            .and_then(|value| value.get("debug_pua_report"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

pub(super) fn encode_pua_report(report: &PuaExecutionReport) -> Option<String> {
    serde_json::to_vec(report)
        .ok()
        .map(|bytes| BASE64_STANDARD.encode(bytes))
}

pub(super) fn stash_pua_report(id: Option<&Value>, encoded: String) {
    let Some(id) = id else {
        return;
    };
    if let Ok(mut guard) = pua_response_reports().lock() {
        guard.insert(value_to_id(id), encoded);
    }
}

pub(super) fn take_pua_report(id: Option<&Value>) -> Option<String> {
    let id = id?;
    pua_response_reports()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&value_to_id(id)))
}

pub(super) fn inject_pua_report_into_result(result: Value, encoded: String) -> Value {
    match result {
        Value::Object(mut object) => {
            let meta = object
                .entry("meta".to_string())
                .or_insert_with(|| json!({}));
            if let Value::Object(meta_obj) = meta {
                meta_obj.insert("x_pua_report".to_string(), Value::String(encoded));
            }
            Value::Object(object)
        }
        other => json!({
            "value": other,
            "meta": { "x_pua_report": encoded }
        }),
    }
}

pub(super) fn inject_pua_report_into_error_data(data: Option<Value>, encoded: String) -> Value {
    match data {
        Some(Value::Object(mut object)) => {
            let meta = object
                .entry("meta".to_string())
                .or_insert_with(|| json!({}));
            if let Value::Object(meta_obj) = meta {
                meta_obj.insert("x_pua_report".to_string(), Value::String(encoded));
            }
            Value::Object(object)
        }
        Some(other) => json!({
            "data": other,
            "meta": { "x_pua_report": encoded }
        }),
        None => json!({
            "meta": { "x_pua_report": encoded }
        }),
    }
}

pub(super) fn infer_pua_stage(method: &str) -> Option<&'static str> {
    if matches!(method, "initialize" | "mcp.initialize") {
        return Some("intake");
    }
    if matches!(method, "task.plan" | "workflow.clarify") {
        return Some("planning");
    }
    if matches!(
        method,
        "task.execute"
            | "workflow.execute"
            | "workflow.generate"
            | "workflow.research"
            | "workflow.consult"
            | "mcp.tools.call"
            | "chat"
    ) {
        return Some("execution");
    }
    if matches!(method, "health" | "runtime.health" | "metrics.get") {
        return Some("verification");
    }
    None
}

pub(super) fn extract_pua_completed_actions(params: &Option<Value>, method: &str) -> Vec<String> {
    let mut completed = vec![method.to_string()];
    if let Some(raw) = params
        .as_ref()
        .and_then(|value| value.get("completed_actions"))
        .and_then(Value::as_array)
    {
        for item in raw {
            if let Some(text) = item.as_str() {
                completed.push(text.to_string());
            }
        }
    }
    completed
}

pub(super) fn build_pua_execution_report(
    stage: &str,
    completed_actions: &[String],
    required_actions: &[String],
    risk_score: f64,
) -> PuaExecutionReport {
    PuaExecutionReport {
        stage: stage.to_string(),
        status: if required_actions.iter().all(|required| {
            completed_actions
                .iter()
                .any(|item| item.eq_ignore_ascii_case(required))
        }) {
            "pass".to_string()
        } else {
            "fail".to_string()
        },
        escalation_level: if risk_score >= 0.8 {
            "L3"
        } else if risk_score >= 0.6 {
            "L2"
        } else {
            "L1"
        }
        .to_string(),
        required_evidence: required_actions.to_vec(),
        completed_checks: completed_actions.to_vec(),
        missing_checks: required_actions
            .iter()
            .filter(|required| {
                !completed_actions
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(required))
            })
            .cloned()
            .collect::<Vec<_>>(),
    }
}
