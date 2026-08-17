// Request dispatch for the content method family.
//
// Extracted from the single dispatch table in `request/mod.rs` (module split
// M0.4): each family matches the normalized method name exactly as the
// original `handle_request` match did, so dispatch behavior is unchanged.
// Content-family dispatch: prompt templates, skill management, and
// ACP tool listing/call.
use anyhow::Result;
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcRequest, RequestTraceContext};

use super::protocol_pack;
use super::prompts_pack;
use super::protocol::AcpErrorCode;
use super::{dispatch_to_client, DispatchOutput};
use crate::i18n::runtime::tf;

pub(crate) async fn dispatch_content(
    server: &AcpServer,
    request: JsonRpcRequest,
    request_id: Option<Value>,
    _http_headers: Option<&str>,
    _trace: &RequestTraceContext,
    method: &str,
) -> Result<()> {
    match method {
        "prompts.list" => {
            let lang = request
                .params
                .as_ref()
                .and_then(|p| p.get("lang"))
                .and_then(|v| v.as_str())
                .unwrap_or(&server.runtime_config.i18n_default_language);
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                prompts_pack::handle_prompts_list(&server.prompt_manager, lang)
                    .map_err(|e| anyhow::anyhow!("{}", e)),
            )
            .await
        }
        "prompts.search" => {
            let lang = request
                .params
                .as_ref()
                .and_then(|p| p.get("lang"))
                .and_then(|v| v.as_str())
                .unwrap_or(&server.runtime_config.i18n_default_language);
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                prompts_pack::handle_prompts_search(
                    &server.prompt_manager,
                    lang,
                    request
                        .params
                        .as_ref()
                        .and_then(|p| p.get("query"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                )
                .map_err(|e| anyhow::anyhow!("{}", e)),
            )
            .await
        }
        "prompts.get" => {
            let lang = request
                .params
                .as_ref()
                .and_then(|p| p.get("lang"))
                .and_then(|v| v.as_str())
                .unwrap_or(&server.runtime_config.i18n_default_language);
            let id = request
                .params
                .as_ref()
                .and_then(|p| p.get("id"))
                .and_then(|v| v.as_str());
            match id {
                Some(id) => {
                    crate::acp::r#impl::io::respond(
                        server,
                        request_id,
                        prompts_pack::handle_prompts_get(&server.prompt_manager, lang, id)
                            .map_err(|e| anyhow::anyhow!("{}", e)),
                    )
                    .await
                }
                None => {
                    dispatch_to_client(
                        server,
                        request_id,
                        Ok(DispatchOutput::error(
                            AcpErrorCode::InvalidParams as i32,
                            tf("error.request.missing_field_id", &[]),
                        )),
                    )
                    .await
                }
            }
        }
        "prompts.create" => {
            let lang = request
                .params
                .as_ref()
                .and_then(|p| p.get("lang"))
                .and_then(|v| v.as_str())
                .unwrap_or(&server.runtime_config.i18n_default_language);
            let params = request.params.clone().unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                prompts_pack::handle_prompts_create(&server.prompt_manager, lang, &params)
                    .map_err(|e| anyhow::anyhow!("{}", e)),
            )
            .await
        }
        "prompts.update" => {
            let lang = request
                .params
                .as_ref()
                .and_then(|p| p.get("lang"))
                .and_then(|v| v.as_str())
                .unwrap_or(&server.runtime_config.i18n_default_language);
            let params = request.params.clone().unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                prompts_pack::handle_prompts_update(&server.prompt_manager, lang, &params)
                    .map_err(|e| anyhow::anyhow!("{}", e)),
            )
            .await
        }
        "prompts.delete" => {
            let lang = request
                .params
                .as_ref()
                .and_then(|p| p.get("lang"))
                .and_then(|v| v.as_str())
                .unwrap_or(&server.runtime_config.i18n_default_language);
            let params = request.params.clone().unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                prompts_pack::handle_prompts_delete(&server.prompt_manager, lang, &params)
                    .map_err(|e| anyhow::anyhow!("{}", e)),
            )
            .await
        }
        "skill.import" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_import_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "skill.enable" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_enabled_toggle_payload(
                    server,
                    request.params.unwrap_or_default(),
                    true,
                )
                .await,
            )
            .await
        }
        "skill.disable" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_enabled_toggle_payload(
                    server,
                    request.params.unwrap_or_default(),
                    false,
                )
                .await,
            )
            .await
        }
        "skill.list_imported" | "skill.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_list_imported_payload(server).await,
            )
            .await
        }
        "skill.create" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_create_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "skill.update" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_update_payload(
                    server,
                    &request.params.clone().unwrap_or_default(),
                ),
            )
            .await
        }
        "skill.version.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_version_list_payload(
                    server,
                    &request.params.clone().unwrap_or_default(),
                ),
            )
            .await
        }
        "skill.version.rollback" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_version_rollback_payload(
                    server,
                    &request.params.clone().unwrap_or_default(),
                ),
            )
            .await
        }
        "skill.remove" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::skill_remove_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "tools/list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::acp_tools_list_payload(server).await,
            )
            .await
        }
        "tools/call" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::acp_tools_call_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        _ => {
            // Unreachable: handle_request routes only methods belonging to this
            // family to this dispatcher; the MethodNotFound fallback stays in
            // the parent module's dispatch table.
            Ok(())
        }
    }
}
