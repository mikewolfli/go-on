fn request_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.request_timeout_seconds)
        .map(Duration::from_secs)
}

async fn autotune_state_snapshot(autotune: &Arc<Mutex<AutoTuneState>>) -> AutoTuneState {
    autotune.lock().await.clone()
}

fn effective_vector_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.enabled))
        .unwrap_or(true)
}

fn effective_vector_auto(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_auto)
        .or_else(|| vector_config.map(|cfg| cfg.auto_mode))
        .unwrap_or(true)
}

fn effective_vector_min_query_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_min_query_chars)
        .or_else(|| options.and_then(|opts| opts.vector_min_query_chars))
        .or_else(|| vector_config.map(|cfg| cfg.min_query_chars))
        .unwrap_or(DEFAULT_VECTOR_MIN_QUERY_CHARS)
}

fn effective_vector_top_k(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_top_k)
        .or_else(|| options.and_then(|opts| opts.vector_top_k))
        .or_else(|| vector_config.map(|cfg| cfg.top_k))
        .unwrap_or(DEFAULT_VECTOR_TOP_K)
}

fn effective_vector_min_similarity(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> f32 {
    options
        .and_then(|opts| opts.vector_min_similarity)
        .or_else(|| vector_config.map(|cfg| cfg.min_similarity))
        .unwrap_or(DEFAULT_VECTOR_MIN_SIMILARITY)
}

fn effective_vector_max_snippet_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.vector_max_snippet_chars)
        .or_else(|| vector_config.map(|cfg| cfg.max_snippet_chars))
        .unwrap_or(DEFAULT_VECTOR_MAX_SNIPPET_CHARS)
}

fn effective_summary_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.summary_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.summary_enabled))
        .unwrap_or(true)
}

fn effective_summary_trigger_messages(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_trigger_messages)
        .or_else(|| vector_config.map(|cfg| cfg.summary_trigger_messages))
        .unwrap_or(DEFAULT_SUMMARY_TRIGGER_MESSAGES)
}

fn effective_summary_max_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_max_chars)
        .or_else(|| vector_config.map(|cfg| cfg.summary_max_chars))
        .unwrap_or(DEFAULT_SUMMARY_MAX_CHARS)
}

fn optimize_messages(messages: &[Message], options: Option<&PhaseOptions>) -> Vec<Message> {
    let mut trimmed = messages.to_vec();

    if let Some(max_messages) = options.and_then(|opts| opts.max_history_messages) {
        if trimmed.len() > max_messages {
            trimmed = trimmed[trimmed.len() - max_messages..].to_vec();
        }
    }

    if let Some(max_chars) = options.and_then(|opts| opts.max_history_chars) {
        let mut kept_reversed = Vec::new();
        let mut total_chars = 0usize;

        for message in trimmed.iter().rev() {
            let message_chars = message.content.chars().count();
            if !kept_reversed.is_empty() && total_chars + message_chars > max_chars {
                break;
            }

            kept_reversed.push(message.clone());
            total_chars += message_chars;
        }

        kept_reversed.reverse();
        trimmed = kept_reversed;
    }

    trimmed
}

fn latest_user_query(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

fn build_vector_context_message(hits: &[VectorHit]) -> String {
    let normalized = dedupe_vector_hits(hits);
    let mut content = String::from("Relevant prior context from similar requests:\n");
    for (index, hit) in normalized.iter().enumerate() {
        content.push_str(&format!(
            "{}. [similarity {:.2}] {}\n",
            index + 1,
            hit.similarity,
            hit.response_snippet
        ));
    }
    content
}

fn append_recent_summary(
    existing_summary: Option<&str>,
    latest_user_query: Option<&str>,
    response_text: &str,
    max_chars: usize,
) -> String {
    let mut segments: Vec<String> = Vec::new();
    if let Some(existing) = existing_summary {
        if !existing.trim().is_empty() {
            segments.push(existing.trim().to_string());
        }
    }
    if let Some(query) = latest_user_query {
        segments.push(format!("User focus: {}", query.trim()));
    }
    if !response_text.trim().is_empty() {
        segments.push(format!("Latest response: {}", response_text.trim()));
    }

    trim_to_tail_chars(&segments.join("\n\n"), max_chars)
}

fn trim_to_tail_chars(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }

    chars[chars.len() - max_chars..].iter().collect()
}

fn build_cache_key(
    phase: &ResolvedPhase,
    messages: &[Message],
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    build_cache_key_from_parts(
        &phase.phase_name,
        messages,
        phase.principles.as_ref(),
        phase.options.as_ref(),
        mode_name,
        approval_strategy,
        agent_names,
    )
}

fn build_cache_key_from_parts(
    phase_name: &str,
    messages: &[Message],
    principles: Option<&Vec<String>>,
    options: Option<&PhaseOptions>,
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    let payload = json!({
        "phase": phase_name,
        "messages": messages,
        "principles": principles,
        "options": options,
        "mode": mode_name,
        "approval_strategy": approval_strategy,
        "agents": agent_names,
    });

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload)?);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn dedupe_vector_hits(hits: &[VectorHit]) -> Vec<VectorHit> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for hit in hits {
        let key = hit
            .response_snippet
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if seen.insert(key) {
            out.push(hit.clone());
        }
    }
    out
}

fn filter_env_ready_agents(config_path: Option<&PathBuf>, candidates: &[String]) -> Vec<String> {
    let Some(path) = config_path else {
        return candidates.to_vec();
    };
    let config = match load_app_config_lazy(path) {
        Some(cfg) => cfg,
        None => return candidates.to_vec(),
    };

    candidates
        .iter()
        .filter(|agent| is_agent_env_ready(config.as_ref(), agent))
        .cloned()
        .collect()
}

fn capability_max_complexity(ready_agents: usize) -> u8 {
    match ready_agents {
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 5,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkGrade {
    Ask,
    Edit,
    Agent,
    Safeguard,
    FullAuto,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewPolicy {
    min_review_level: String,
    required_reviews: usize,
    required_checks: Vec<String>,
    timeout_policy: String,
    enforce_dual_review: bool,
    enforce_action_gates: bool,
}

