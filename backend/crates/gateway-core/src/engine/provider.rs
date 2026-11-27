//! 原生 Provider 的执行边界

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use async_trait::async_trait;
use futures::{Stream, future::BoxFuture};
use thiserror::Error;

use crate::account::{
    AccountAttemptFeedback, AccountCapacitySnapshot, AccountFeedbackStats, AccountHistorySnapshot,
    ProviderAccountId,
};
use crate::engine::AttemptContext;
use crate::error::{PreDeliveryRetry, ProviderError, ProviderErrorKind};
use crate::event::{EventSequenceError, EventSequenceValidator, ProviderEvent};
use crate::identity::ProviderKind;
use crate::operation::Operation;
use crate::policy::ClientApiKeyId;
use crate::routing::{
    ProviderCandidate, ProviderCatalogGeneration, ProviderCatalogPort, ProviderCatalogUnavailable,
    ProviderModelCapabilities, PublicModelId, UpstreamModelId,
};
use crate::upstream::OpaqueUpstreamValue;
use crate::upstream::{UpstreamSendState, UpstreamTransport};

/// Box 只出现在 Provider Registry 的统一 event envelope 边界
pub type EventStream =
    Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send + 'static>>;

/// Provider 选定单个 credential 后返回的调用事实
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCallMetadata {
    provider: ProviderKind,
    upstream_model: Option<UpstreamModelId>,
    provider_account_id: ProviderAccountId,
    upstream_request_id: Option<OpaqueUpstreamValue>,
    transport: UpstreamTransport,
    selection_observation: Option<ProviderSelectionObservation>,
    account_snapshot: Option<AccountHistorySnapshot>,
    credential_revision: Option<crate::account::CredentialRevision>,
}

/// Provider 账号选择阶段输出的中立运行压力事实
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSelectionObservation {
    account_selection_wait_ms: u64,
    capacity: Option<AccountCapacitySnapshot>,
}

impl ProviderSelectionObservation {
    #[must_use]
    pub const fn new(
        account_selection_wait_ms: u64,
        capacity: Option<AccountCapacitySnapshot>,
    ) -> Self {
        Self {
            account_selection_wait_ms,
            capacity,
        }
    }

    #[must_use]
    pub const fn account_selection_wait_ms(self) -> u64 {
        self.account_selection_wait_ms
    }

    #[must_use]
    pub const fn capacity(self) -> Option<AccountCapacitySnapshot> {
        self.capacity
    }
}

impl ProviderCallMetadata {
    /// 创建一次调用的不可变事实
    #[must_use]
    pub const fn new(
        provider: ProviderKind,
        upstream_model: UpstreamModelId,
        provider_account_id: ProviderAccountId,
        transport: UpstreamTransport,
    ) -> Self {
        Self {
            provider,
            upstream_model: Some(upstream_model),
            provider_account_id,
            upstream_request_id: None,
            transport,
            selection_observation: None,
            account_snapshot: None,
            credential_revision: None,
        }
    }

    /// 创建一次不声明模型的 Provider 原生端点调用事实
    #[must_use]
    pub const fn for_provider_endpoint(
        provider: ProviderKind,
        provider_account_id: ProviderAccountId,
        transport: UpstreamTransport,
    ) -> Self {
        Self {
            provider,
            upstream_model: None,
            provider_account_id,
            upstream_request_id: None,
            transport,
            selection_observation: None,
            account_snapshot: None,
            credential_revision: None,
        }
    }

    /// 账号实际选定时冻结的凭据版本，不在反馈时重新读取当前账号
    #[must_use]
    pub const fn with_credential_revision(
        mut self,
        revision: crate::account::CredentialRevision,
    ) -> Self {
        self.credential_revision = Some(revision);
        self
    }

    #[must_use]
    pub const fn credential_revision(&self) -> Option<crate::account::CredentialRevision> {
        self.credential_revision
    }

    #[must_use]
    pub fn with_account_snapshot(mut self, snapshot: Option<AccountHistorySnapshot>) -> Self {
        self.account_snapshot = snapshot;
        self
    }

    #[must_use]
    pub const fn account_snapshot(&self) -> Option<&AccountHistorySnapshot> {
        self.account_snapshot.as_ref()
    }

    /// 设置 adapter 已分类为非 bearer 的 request ID
    #[must_use]
    pub fn with_upstream_request_id(mut self, request_id: OpaqueUpstreamValue) -> Self {
        self.upstream_request_id = Some(request_id);
        self
    }

    /// 附加 Provider 选择账号时测得的等待与容量快照
    #[must_use]
    pub const fn with_selection_observation(
        mut self,
        observation: ProviderSelectionObservation,
    ) -> Self {
        self.selection_observation = Some(observation);
        self
    }

    /// 返回 Provider
    #[must_use]
    pub const fn provider(&self) -> &ProviderKind {
        &self.provider
    }

    /// 返回实际模型；Provider 原生端点没有模型事实
    #[must_use]
    pub const fn upstream_model(&self) -> Option<&UpstreamModelId> {
        self.upstream_model.as_ref()
    }

    /// 返回 live Provider account ID
    #[must_use]
    pub const fn provider_account_id(&self) -> &ProviderAccountId {
        &self.provider_account_id
    }

    /// 返回安全上游 request ID
    #[must_use]
    pub const fn upstream_request_id(&self) -> Option<&OpaqueUpstreamValue> {
        self.upstream_request_id.as_ref()
    }

    /// 返回 transport
    #[must_use]
    pub const fn transport(&self) -> &UpstreamTransport {
        &self.transport
    }

    #[must_use]
    pub const fn selection_observation(&self) -> Option<ProviderSelectionObservation> {
        self.selection_observation
    }

    /// 确认 metadata 没有替换请求计划中冻结的 Provider 候选
    #[must_use]
    pub fn confirms(&self, candidate: &ProviderCandidate) -> bool {
        candidate.provider() == &self.provider
            && candidate.upstream_model() == self.upstream_model.as_ref()
    }
}

/// Provider 自己持有的 credential/concurrency 租约
///
/// 句柄必须通过 `Drop` 释放资源
/// Core 不读取 credential，也不为 lease 定义
/// 平台无关字段
pub trait ResourceLease: Send + Sync + 'static {}

impl<T> ResourceLease for T where T: Send + Sync + 'static {}

/// Provider 原生响应格式到统一响应格式的有状态转换边界
///
/// Core 只负责调用顺序、格式与终态约束，不解释协议正文
/// 实现必须由一次
/// [`ProviderStream`] 独占，不能在并发请求间共享可变转换状态
pub trait NativeResponseTranslator: Send + 'static {
    /// 转换前协议格式
    fn source_protocol(&self) -> &str;

    /// 转换后协议格式
    fn target_protocol(&self) -> &str;

    /// 把一个原生响应事件转换为零至多个统一响应事件
    ///
    /// # Errors
    ///
    /// 原生协议事件无效或转换状态不一致时返回已发送的 Provider 错误
    fn translate(
        &mut self,
        event: &crate::event::ProtocolWireEvent,
    ) -> Result<Vec<crate::event::ProtocolWireEvent>, ProviderError>;
}

const MAX_NATIVE_RESPONSE_EVENTS_PER_INPUT: usize = 64;

/// Metadata、canonical event stream 与 owned lease 的统一返回值
///
/// 底层 stream 必须是 cold stream：在第一次 poll 前不得发送请求级 handshake
/// 或业务 payload
/// 这样 Coordinator 可以先持久化 attempt，再越过发送屏障
pub struct ProviderStream {
    metadata: ProviderCallMetadata,
    source: Option<ProviderStreamSource>,
    native_response_translator: Option<Box<dyn NativeResponseTranslator>>,
    account_feedback: Option<ProviderStreamAccountFeedback>,
    validator: Result<EventSequenceValidator, EventSequenceError>,
    strict_canonical_seen: bool,
}

// 事件源也可能持有租约；停止读取时一起释放，避免结算 I/O 占用账号槽位。
struct ProviderStreamSource {
    events: EventStream,
    _lease: Box<dyn ResourceLease>,
}

struct ProviderStreamAccountFeedback {
    stats: Arc<AccountFeedbackStats>,
    provider_kind: ProviderKind,
    account_id: ProviderAccountId,
    failure_filter: fn(&ProviderError) -> bool,
    started_at: Option<Instant>,
    first_output_ms: Option<u64>,
    reported: bool,
}

fn score_all_confirmed_failures(_: &ProviderError) -> bool {
    true
}

impl ProviderStreamAccountFeedback {
    fn start(&mut self) {
        self.started_at.get_or_insert_with(Instant::now);
    }

    fn observe(&mut self, event: &ProviderEvent) {
        if self.first_output_ms.is_some()
            || !event.canonical_facts().iter().any(|event| {
                matches!(
                    event,
                    crate::event::GatewayEvent::TextDelta(_)
                        | crate::event::GatewayEvent::ReasoningDelta(_)
                        | crate::event::GatewayEvent::ToolCallDelta(_)
                )
            })
        {
            return;
        }
        let Some(started_at) = self.started_at else {
            return;
        };
        self.first_output_ms =
            Some(u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX));
    }

    fn report_success(&mut self) {
        if self.reported {
            return;
        }
        self.stats.report(
            &self.provider_kind,
            &self.account_id,
            AccountAttemptFeedback::Succeeded {
                first_output_ms: self.first_output_ms,
            },
        );
        self.reported = true;
    }

    fn report_failure(&mut self, error: &ProviderError) {
        if self.reported
            // `Ambiguous` 仍须关闭重放边界，但不能证明失败由账号造成；将它计入
            // 账号评分会把传输不确定性错误归因给账号
            || error.send_state() != UpstreamSendState::Sent
            || matches!(
                error.kind(),
                ProviderErrorKind::Cancelled | ProviderErrorKind::ProcessTerminated
            )
            || matches!(
                error.pre_delivery_retry(),
                Some(
                    PreDeliveryRetry::SameAccountTransportRetry { .. }
                        | PreDeliveryRetry::SameAccountTransportFallback
                )
            )
            || error.retries_same_account()
            || !(self.failure_filter)(error)
        {
            return;
        }
        let feedback = if error.kind() == ProviderErrorKind::UpstreamCapacityUnavailable {
            AccountAttemptFeedback::CapacityRejected {
                first_output_ms: self.first_output_ms,
            }
        } else {
            AccountAttemptFeedback::Failed {
                first_output_ms: self.first_output_ms,
            }
        };
        self.stats
            .report(&self.provider_kind, &self.account_id, feedback);
        self.reported = true;
    }
}

impl ProviderStream {
    /// 组装一次、且仅一次可见上游调用
    #[must_use]
    pub fn new<S>(metadata: ProviderCallMetadata, events: S, lease: impl ResourceLease) -> Self
    where
        S: Stream<Item = Result<ProviderEvent, ProviderError>> + Send + 'static,
    {
        Self {
            metadata,
            source: Some(ProviderStreamSource {
                events: Box::pin(events),
                _lease: Box::new(lease),
            }),
            native_response_translator: None,
            account_feedback: None,
            validator: Ok(EventSequenceValidator::new()),
            strict_canonical_seen: false,
        }
    }

    pub(super) fn abort(&mut self) {
        self.source.take();
    }

    /// 让公共 stream 边界统一回灌账号成功率与首个有效输出延迟
    #[must_use]
    pub fn with_account_feedback(mut self, stats: Arc<AccountFeedbackStats>) -> Self {
        self.set_account_feedback(stats, score_all_confirmed_failures);
        self
    }

    /// 使用 Provider 定义的闭集判断回灌账号成功率与首个有效输出延迟
    #[must_use]
    pub fn with_filtered_account_feedback(
        mut self,
        stats: Arc<AccountFeedbackStats>,
        failure_filter: fn(&ProviderError) -> bool,
    ) -> Self {
        self.set_account_feedback(stats, failure_filter);
        self
    }

    /// 在原生响应加工的两个策略阶段之间安装 Provider 自有转换器
    #[must_use]
    pub fn with_native_response_translator(
        mut self,
        translator: impl NativeResponseTranslator,
    ) -> Self {
        self.native_response_translator = Some(Box::new(translator));
        self
    }

    /// 在 Core 已记录原始事实且完成 `BeforeTranslation` 后执行原生转换
    ///
    /// # Errors
    ///
    /// source/target 格式不一致、单事件展开越界、终态被丢弃或 Provider 转换失败时
    /// 返回错误
    pub fn translate_native_response(
        &mut self,
        event: ProviderEvent,
        terminal: bool,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        self.translate_event(event, terminal)
            .inspect_err(|_| self.abort())
    }

    fn translate_event(
        &mut self,
        mut event: ProviderEvent,
        terminal: bool,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let Some(translator) = self.native_response_translator.as_mut() else {
            return Ok(vec![event]);
        };
        let Some(wire) = event.wire_event() else {
            return Ok(vec![event]);
        };
        if wire.protocol() != translator.source_protocol() {
            return Err(native_response_protocol_error());
        }
        let mut translated = translator.translate(wire)?;
        if translated.len() > MAX_NATIVE_RESPONSE_EVENTS_PER_INPUT
            || (terminal && translated.is_empty())
            || translated
                .iter()
                .any(|wire| wire.protocol() != translator.target_protocol())
        {
            return Err(native_response_protocol_error());
        }
        let Some(last) = translated.pop() else {
            // wire 转换器可以吞掉纯协议结构事件，但不能连带丢失 Core 已经
            // 识别出的 usage 等 canonical facts
            // facts-only 封套仍需交给
            // 客户端 adapter；没有 facts 时才是真正的零输出
            event.replace_wire(None);
            return Ok(event
                .has_canonical_facts()
                .then_some(event)
                .into_iter()
                .collect());
        };
        // 一对多时把 canonical/observation/session 信封放到最后一个 wire；这样
        // `Completed` 不会在同一源事件的前置输出交付前提前终结 Coordinator
        let mut events = translated
            .into_iter()
            .map(ProviderEvent::wire)
            .collect::<Vec<_>>();
        event.replace_wire(Some(last));
        events.push(event);
        Ok(events)
    }

    fn set_account_feedback(
        &mut self,
        stats: Arc<AccountFeedbackStats>,
        failure_filter: fn(&ProviderError) -> bool,
    ) {
        self.account_feedback = Some(ProviderStreamAccountFeedback {
            stats,
            provider_kind: self.metadata.provider().clone(),
            account_id: self.metadata.provider_account_id().clone(),
            failure_filter,
            started_at: None,
            first_output_ms: None,
            reported: false,
        });
    }

    /// 返回调用事实
    #[must_use]
    pub const fn metadata(&self) -> &ProviderCallMetadata {
        &self.metadata
    }
}

fn native_response_protocol_error() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::Protocol,
        crate::upstream::UpstreamSendState::Sent,
    )
    .redact_sensitive_context("invalid native response translation")
}

impl Stream for ProviderStream {
    type Item = Result<ProviderEvent, ProviderError>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let result = this.poll_event(context);
        if matches!(&result, Poll::Ready(Some(Err(_)) | None)) {
            this.abort();
        }
        result
    }
}

impl ProviderStream {
    fn poll_event(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<ProviderEvent, ProviderError>>> {
        let this = self;
        let Some(source) = this.source.as_mut() else {
            return Poll::Ready(None);
        };
        if let Some(feedback) = this.account_feedback.as_mut() {
            feedback.start();
        }

        match source.events.as_mut().poll_next(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(event))) => {
                // 纯 wire 流的 facts 仍只是旁路观测；一旦交付 canonical-only
                // 事件，就必须校验完整事实序列，不能漏掉带 wire 的开始或终态
                this.strict_canonical_seen |=
                    event.wire_event().is_none() && !event.canonical_facts().is_empty();
                for fact in event.canonical_facts() {
                    if let Ok(validator) = &mut this.validator
                        && let Err(error) = validator.observe(fact)
                    {
                        this.validator = Err(error);
                    }
                }
                if this.strict_canonical_seen && this.validator.is_err() {
                    let error =
                        ProviderError::new(ProviderErrorKind::Protocol, UpstreamSendState::Sent);
                    if let Some(feedback) = this.account_feedback.as_mut() {
                        feedback.report_failure(&error);
                    }
                    return Poll::Ready(Some(Err(error)));
                }
                if let Some(feedback) = this.account_feedback.as_mut() {
                    feedback.observe(&event);
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                if let Some(feedback) = this.account_feedback.as_mut() {
                    feedback.report_failure(&error);
                }
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                let validation = if this.strict_canonical_seen {
                    this.validator
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(EventSequenceValidator::finish)
                } else {
                    Ok(())
                };
                match validation {
                    Ok(()) => {
                        if let Some(feedback) = this.account_feedback.as_mut() {
                            feedback.report_success();
                        }
                        Poll::Ready(None)
                    }
                    Err(_) => {
                        let error = ProviderError::new(
                            ProviderErrorKind::Protocol,
                            UpstreamSendState::Sent,
                        );
                        if let Some(feedback) = this.account_feedback.as_mut() {
                            feedback.report_failure(&error);
                        }
                        Poll::Ready(Some(Err(error)))
                    }
                }
            }
        }
    }
}

/// 传给 Provider 的单候选请求
#[derive(Clone)]
pub struct ProviderRequest {
    operation: Operation,
    candidate: ProviderCandidate,
}

/// Provider 对公共观测表可解释的请求语义；未知字段保持空值
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderRequestObservation {
    /// 独立 Provider 端点声明的请求模型，仅用于观测，不参与文本模型目录路由
    pub requested_model: Option<PublicModelId>,
    /// Provider 从客户端请求与续接配置中解析的推理强度
    pub reasoning_effort: Option<String>,
    pub reasoning_preset: Option<String>,
    /// 思考模式与预算是独立请求事实，不用于推断 reasoning_effort 或 reasoning_preset。
    pub thinking_mode: Option<String>,
    pub thinking_budget_tokens: Option<u64>,
    /// 已采集配置为 Some(true)，即使全部缺省；None 保留历史或其他协议未采集的语义。
    pub reasoning_observed: Option<bool>,
    pub request_kind: Option<String>,
    pub subagent_kind: Option<String>,
    pub compact: bool,
    pub continuation: ContinuationRequestObservation,
}

/// 仅用于恢复事件关联的客户端作用域不透明请求事实
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuationRequestObservation {
    pub affinity_hash: Option<String>,
    pub previous_response_id_hash: Option<String>,
    pub requested: bool,
}

impl ProviderRequest {
    /// 绑定 operation 与请求计划中冻结的 Provider 候选
    #[must_use]
    pub const fn new(operation: Operation, candidate: ProviderCandidate) -> Self {
        Self {
            operation,
            candidate,
        }
    }

    /// 返回业务 operation
    #[must_use]
    pub const fn operation(&self) -> &Operation {
        &self.operation
    }

    /// 返回冻结 Provider 候选
    #[must_use]
    pub const fn candidate(&self) -> &ProviderCandidate {
        &self.candidate
    }
}

impl fmt::Debug for ProviderRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRequest")
            .field("operation", &self.operation)
            .field("provider", self.candidate.provider())
            .field("upstream_model", &self.candidate.upstream_model())
            .finish()
    }
}

/// Provider 热路径唯一接口
///
/// 每次 `execute` 只能选择一个 credential 并准备一次可见上游调用
/// 实现不得
/// 在内部轮换 credential 或隐藏业务 retry；失败后由 Attempt Coordinator 使用
/// 新的 attempt 再次调用
#[async_trait]
pub trait Provider: Send + Sync {
    /// 声明可用于互联共享的直连认证方式，不执行网络查询
    fn is_direct_account(&self, _authentication_kind: &str) -> bool {
        false
    }

    /// Provider 解释自己的隐私规则，Core 仅冻结编译结果
    fn compile_privacy_policy(
        &self,
        _policy: &crate::settings::privacy::CodexPrivacyPolicy,
    ) -> Result<
        Arc<dyn crate::settings::privacy::CompiledPrivacyPolicy>,
        crate::settings::privacy::PrivacyError,
    > {
        Err(crate::settings::privacy::PrivacyError {
            rule_index: 0,
            reason: "Provider 不支持隐私策略",
        })
    }

    /// 从已冻结的配置解析请求身份；只读取本地发布资料，不执行网络请求
    fn resolve_request_profile(
        &self,
        configuration: &crate::account::OpaqueProviderData,
    ) -> Result<crate::account::OpaqueProviderData, ProviderError> {
        Ok(configuration.clone())
    }

    /// 没有持久选择时返回 Provider 的已准备默认画像；结果仍由 Core 按请求冻结
    fn default_request_profile(
        &self,
    ) -> Result<Option<crate::account::OpaqueProviderData>, ProviderError> {
        Ok(None)
    }

    /// 返回实例生命周期内稳定的注册名称
    fn name(&self) -> &str;

    /// 返回当前进程已经成功发布的目录代次
    fn catalog_generation(&self) -> ProviderCatalogGeneration;

    /// 发现型目录不用于提前拒绝上游可能支持的新模型或别名
    fn model_catalog_is_exhaustive(&self) -> bool {
        true
    }

    /// 解释 Provider 差异化观测字段；不参与路由和传输
    fn request_observation(
        &self,
        _operation: &Operation,
        _client_api_key_id: &ClientApiKeyId,
    ) -> ProviderRequestObservation {
        ProviderRequestObservation::default()
    }

    /// 查询当前 Provider 的实时模型目录，并由 Provider 自己编译能力事实
    ///
    /// # Errors
    ///
    /// 目录 transport、认证或 Provider 协议失败时返回稳定错误
    async fn query_model_capabilities(
        &self,
    ) -> Result<Vec<ProviderModelCapabilities>, ProviderError>;

    /// 读取当前客户端协议的原生目录，必须限定到认证时冻结的账号范围
    /// `None` 表示不提供该协议的原生目录；读取失败不能伪装成不支持
    async fn query_client_model_catalog(
        &self,
        _client_api_key_id: &ClientApiKeyId,
        _scope: &crate::account::scope::FrozenAccountScope,
        _protocol: &str,
        _client_version: &str,
        _peer_route: Option<&http::HeaderValue>,
    ) -> Result<
        Option<Vec<crate::routing::ProviderModelDescriptor>>,
        crate::routing::ProviderCatalogUnavailable,
    > {
        Ok(None)
    }

    /// 选择一个未被排除的资源并返回 cold [`ProviderStream`]
    ///
    /// 返回成功、返回错误或准备 future 被取消前，均不得发送本次请求的上游握手或业务
    /// 载荷，也不得启动可独立完成这些发送的后台任务
    /// 发送只在返回的流被 poll 后发生，
    /// 保证 Core 能在真实出站前校验账号范围并登记 attempt
    ///
    /// # Errors
    ///
    /// 没有可用资源、请求无效或准备失败时返回 `NotSent` 错误；
    /// 可能已发送的失败必须通过 stream 返回，不得降级发送事实
    async fn execute(
        self: Arc<Self>,
        request: ProviderRequest,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError>;

    /// 返回 Provider 的 Live sideband 能力；不支持的 Provider 返回 `None`。
    /// 实现必须只暴露已钉住账号的受限拨号，不得泄露凭据或放宽账号范围。
    fn live_gateway(&self) -> Option<Arc<dyn crate::live::LiveGateway>> {
        None
    }
}

/// Provider Registry 构建错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryError {
    /// Provider 名称无效
    #[error("invalid provider registry name")]
    InvalidName,
    /// Provider 重复注册
    #[error("provider `{provider}` is already registered")]
    Duplicate {
        /// Provider 名称
        provider: String,
    },
}

/// 唯一保存 `Arc<dyn Provider>` 的异构注册表
#[derive(Default)]
pub struct ProviderRegistryBuilder {
    providers: BTreeMap<ProviderKind, Arc<dyn Provider>>,
}

impl ProviderRegistryBuilder {
    /// 创建空 builder
    #[must_use]
    pub const fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    /// 注册 Provider
    ///
    /// # Errors
    ///
    /// 名称无效或重复时返回错误
    pub fn register(&mut self, provider: Arc<dyn Provider>) -> Result<(), RegistryError> {
        let kind = ProviderKind::new(provider.name()).map_err(|_| RegistryError::InvalidName)?;
        if self.providers.contains_key(&kind) {
            return Err(RegistryError::Duplicate {
                provider: kind.as_str().to_owned(),
            });
        }
        self.providers.insert(kind, provider);
        Ok(())
    }

    /// 冻结注册表
    #[must_use]
    pub fn build(self) -> ProviderRegistry {
        ProviderRegistry {
            providers: Arc::new(self.providers),
        }
    }
}

/// 固定内置 Provider 的不可变注册表
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    pub(super) providers: Arc<BTreeMap<ProviderKind, Arc<dyn Provider>>>,
}

impl ProviderRegistry {
    /// 从一组异构 Provider 构造冻结注册表
    pub fn new(
        providers: impl IntoIterator<Item = Arc<dyn Provider>>,
    ) -> Result<Self, RegistryError> {
        let mut builder = ProviderRegistryBuilder::new();
        for provider in providers {
            builder.register(provider)?;
        }
        Ok(builder.build())
    }

    /// 创建 builder
    #[must_use]
    pub const fn builder() -> ProviderRegistryBuilder {
        ProviderRegistryBuilder::new()
    }

    /// 按 Provider 名称查询 adapter
    #[must_use]
    pub fn get(&self, provider: &ProviderKind) -> Option<&Arc<dyn Provider>> {
        self.providers.get(provider)
    }

    #[must_use]
    pub fn request_observation(
        &self,
        provider: &ProviderKind,
        operation: &Operation,
        client_api_key_id: &ClientApiKeyId,
    ) -> ProviderRequestObservation {
        self.providers
            .get(provider)
            .map_or_else(ProviderRequestObservation::default, |registered| {
                registered.request_observation(operation, client_api_key_id)
            })
    }

    /// 判断 Provider 是否已注册
    #[must_use]
    pub fn contains(&self, provider: &ProviderKind) -> bool {
        self.providers.contains_key(provider)
    }

    /// 返回注册数量
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// 判断注册表是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// 按注册顺序遍历 adapter；仅用于能力发现类查询。
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Provider>> {
        self.providers.values()
    }
}

impl ProviderCatalogPort for ProviderRegistry {
    fn is_direct_account(&self, provider: &ProviderKind, authentication_kind: &str) -> bool {
        self.providers
            .get(provider)
            .is_some_and(|provider| provider.is_direct_account(authentication_kind))
    }

    fn model_catalog_is_exhaustive(&self, provider: &ProviderKind) -> bool {
        self.providers
            .get(provider)
            .is_none_or(|provider| provider.model_catalog_is_exhaustive())
    }

    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration> {
        self.providers
            .iter()
            .map(|(kind, provider)| (kind.clone(), provider.catalog_generation()))
            .collect()
    }

    fn query_model_capabilities(
        &self,
        provider_kind: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>> {
        let provider = self.providers.get(provider_kind);
        Box::pin(async move {
            provider
                .ok_or(ProviderCatalogUnavailable)?
                .query_model_capabilities()
                .await
                .map_err(|_| ProviderCatalogUnavailable)
        })
    }
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("providers", &self.providers.keys())
            .finish()
    }
}

impl crate::settings::privacy::PrivacyPolicyCompiler for ProviderRegistry {
    fn compile(
        &self,
        policy: &crate::settings::privacy::CodexPrivacyPolicy,
    ) -> Result<
        Arc<dyn crate::settings::privacy::CompiledPrivacyPolicy>,
        crate::settings::privacy::PrivacyError,
    > {
        self.providers
            .iter()
            .find(|(kind, _)| kind.as_str() == "openai")
            .ok_or(crate::settings::privacy::PrivacyError {
                rule_index: 0,
                reason: "OpenAI Provider 不可用",
            })?
            .1
            .compile_privacy_policy(policy)
    }
}
