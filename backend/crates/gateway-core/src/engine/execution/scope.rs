//! 请求嵌套图、活动权限与作用域 guard

use super::admission::{AdmissionLease, settle_budget};
use super::contract::{AuthenticatedClient, ExecutionRequestMetadata};
use crate::engine::budget::{ClientBudgetCharge, ClientBudgetPort};
use crate::{
    engine::{ModelRequestId, coordinator::CoordinationContext, request_context::RequestScope},
    identity::ProviderKind,
    lifecycle::{CancellationToken, Deadline},
    operation::Operation,
    policy::ClientApiKeyId,
    routing::{FrozenAccountScope, PublicModelId, UpstreamModelId},
};
use std::{sync::Arc, time::SystemTime};

pub(super) enum ExecutionTarget {
    Model(PublicModelId),
    ProviderEndpoint {
        provider: ProviderKind,
        upstream_model: Option<UpstreamModelId>,
    },
}

impl ExecutionTarget {
    pub(super) fn into_public_model(self) -> Option<PublicModelId> {
        match self {
            Self::Model(model) => Some(model),
            Self::ProviderEndpoint { .. } => None,
        }
    }
}

pub(super) struct PendingStartExecution {
    pub(super) client: AuthenticatedClient,
    pub(super) target: ExecutionTarget,
    pub(super) operation: Operation,
    pub(super) metadata: ExecutionRequestMetadata,
}

pub(super) struct AuthorizedExecution {
    pub(super) response_control: Option<crate::engine::response_control::ResponseControl>,
    pub(super) account_scope: Arc<FrozenAccountScope>,
    pub(super) deadline_at: Deadline,
    pub(super) cancellation: CancellationToken,
    pub(super) request_scope: RequestScope,
}

pub(super) struct ExecutionStartGuard {
    pub(super) admission: ExecutionAdmission,
    pub(super) admission_decision_ms: Option<u64>,
}

pub(super) struct PreparedExecutionStart {
    pub(super) request_id: ModelRequestId,
    pub(super) started_at: SystemTime,
    pub(super) plan: crate::routing::RoutingPlan,
    pub(super) extensions: CoordinationContext,
    pub(super) authorization: AuthorizedExecution,
    pub(super) guard: ExecutionStartGuard,
}

impl ExecutionStartGuard {
    pub(super) async fn release_failed(
        self,
        budget: Option<&dyn ClientBudgetPort>,
        request_id: ModelRequestId,
        key_id: ClientApiKeyId,
        diagnostics: &dyn crate::diagnostics::OperationalDiagnostics,
    ) {
        if let Some(budget) = budget {
            settle_budget(
                budget,
                ClientBudgetCharge {
                    key_id,
                    request_id,
                    amount_usd: crate::metering::Decimal::ZERO,
                    completed_at: SystemTime::now(),
                },
                diagnostics,
            )
            .await;
        }
        self.admission.release().await;
    }
}

pub(super) enum ExecutionAdmission {
    Client(AdmissionLease),
}

impl ExecutionAdmission {
    pub(super) async fn release(self) {
        match self {
            Self::Client(admission) => admission.release().await,
        }
    }
}
