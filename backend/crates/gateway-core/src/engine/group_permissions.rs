//! Key 绑定分组的功能权限在执行入口统一校验

use crate::{account::scope::FrozenAccountScope, error::DeniedGroupFeature, operation::Operation};

pub(super) fn check_group_permissions(
    scope: &FrozenAccountScope,
    operation: &Operation,
) -> Result<(), DeniedGroupFeature> {
    super::image_generation::check_image_generation(scope.allows_image_generation(), operation)
        .map_err(|_| DeniedGroupFeature::ImageGeneration)?;
    if !scope.allows_subagents()
        && let Operation::Generate(request) = operation
        && request.protocol_payload().protocol() == "openai"
    {
        let payload = request.protocol_payload();
        // API 已把本轮 HTTP/WS 的协作头投影到协议上下文，Core 不重读握手旧头。
        if gateway_protocol::openai::codex::collaboration_subagent_requested(
            payload.body(),
            payload.context(),
            |_| None,
        ) {
            return Err(DeniedGroupFeature::CollaborativeSubagents);
        }
    }
    Ok(())
}
