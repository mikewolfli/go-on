use super::*;

// NOTE: The handler functions handle_chat, handle_phase, handle_primary_secondary_summary
// were previously here but were dead code — the dispatch in request.rs routes to
// protocol_pack and learning_pack versions. These utility functions
// (parse_messages, send_error, send_result) remain because they are actively used.

pub(super) fn parse_messages(params: &Value) -> Option<Vec<Message>> {
    if let Some(messages) = params.get("messages") {
        return serde_json::from_value(messages.clone()).ok();
    }
    if let Some(message) = params.get("message") {
        return serde_json::from_value(message.clone())
            .ok()
            .map(|message| vec![message]);
    }

    params
        .get("content")
        .and_then(Value::as_str)
        .map(|content| {
            vec![Message {
                role: params
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user")
                    .to_string(),
                content: content.to_string(),
            }]
        })
}

/// Send error response
pub(super) async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    mark_error_response(id.as_ref());
    let error_data =
        inject_platform_profiles_if_absent(data.unwrap_or_else(|| json!({})), "acp.error");
    let data = Some(error_data);
    let data = match take_pua_report(id.as_ref()) {
        Some(encoded) => Some(inject_pua_report_into_error_data(data, encoded)),
        None => data,
    };
    let data = with_error_contract_data(code, &message, data);
    crate::acp::r#impl::io::send_error(server, id, code, message, data).await
}

/// Send result response
pub(super) async fn send_result(
    server: &AcpServer,
    id: Option<Value>,
    result: Value,
) -> Result<()> {
    let method = DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_default();
    let result = inject_platform_profiles_if_absent(result, &method);
    let result = match take_pua_report(id.as_ref()) {
        Some(encoded) => inject_pua_report_into_result(result, encoded),
        None => result,
    };
    crate::acp::r#impl::io::send_result(server, id, result).await
}
