//! 连接写入在唯一 writer 中串行执行，共享取消与超时预算。
use super::{ConnectionWriteError, frame::DOWNSTREAM_CLOSE_TIMEOUT, state::PumpContext};
use axum::extract::ws::Message;
use futures::{Sink, SinkExt};
use gateway_core::lifecycle::CancellationToken;
use std::{fmt, sync::Arc};
use tokio::time::timeout;

pub(super) struct Writer<S> {
    socket: tokio::sync::Mutex<S>,
    context: Arc<PumpContext>,
}
impl<S, E> Writer<S>
where
    S: Sink<Message, Error = E> + Unpin + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    pub(super) fn new(socket: S, context: Arc<PumpContext>) -> Self {
        Self {
            socket: tokio::sync::Mutex::new(socket),
            context,
        }
    }
    pub(super) async fn write(&self, message: Message) -> Result<(), ConnectionWriteError> {
        // 预算包含排队与真实写入，取消不等待其他调用释放锁
        tokio::select! {
            biased;
            () = self.context.cancellation.cancelled() => Err(ConnectionWriteError::Closed),
            result = tokio::time::timeout(self.context.config.write_timeout, async {
                let mut socket = self.socket.lock().await;
                // 发送中断后不能继续复用可能含半写入帧的连接
                let mut in_flight = CancelOnDrop(Some(self.context.cancellation.clone()));
                let result = write_message(&mut *socket, message, &self.context).await;
                if result.is_ok() { in_flight.0.take(); }
                result
            }) => result.unwrap_or(Err(ConnectionWriteError::Timeout { timeout: self.context.config.write_timeout })),
        }
    }

    pub(super) async fn finish_peer_close(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), ConnectionWriteError> {
        // 主动 Sender 也可能在 pump 之外持锁；先独占 transport 再取消业务，
        // 有在途写或已取消的半帧时立即跳过，不能等待它释放锁后继续刷新
        let socket = self
            .socket
            .try_lock()
            .ok()
            .filter(|_| !self.context.cancellation.is_cancelled());
        self.context.cancellation.cancel();
        let Some(mut socket) = socket else {
            return Err(ConnectionWriteError::Closed);
        };
        // 收到 Close 后 transport 已排队应答；只刷新它，不能再发送一份 Close
        // 业务调用已取消，清理只受宿主取消与独立短预算约束，不沿用业务写入的长超时
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(ConnectionWriteError::Closed),
            result = timeout(DOWNSTREAM_CLOSE_TIMEOUT, async {
                socket.flush().await.map_err(|error| {
                    ConnectionWriteError::Transport { message: error.to_string() }
                })
            }) => result.unwrap_or(Err(ConnectionWriteError::Timeout {
                timeout: DOWNSTREAM_CLOSE_TIMEOUT,
            })),
        }
    }
}
async fn write_message<S, E>(
    socket: &mut S,
    message: Message,
    context: &PumpContext,
) -> Result<(), ConnectionWriteError>
where
    S: Sink<Message, Error = E> + Unpin,
    E: fmt::Display,
{
    tokio::select! {
        biased;
        () = context.cancellation.cancelled() => Err(ConnectionWriteError::Closed),
        result = timeout(context.config.write_timeout, socket.send(message)) => {
            match result {
                Ok(Ok(())) => { context.stats.record_write(context.opened_at); Ok(()) }
                Ok(Err(error)) => Err(ConnectionWriteError::Transport { message: error.to_string() }),
                Err(_) => Err(ConnectionWriteError::Timeout { timeout: context.config.write_timeout }),
            }
        }
    }
}

/// 取消半写入或已退出连接，防止继续使用状态不确定的 transport。
pub(crate) struct CancelOnDrop(pub(crate) Option<CancellationToken>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancellation) = self.0.take() {
            cancellation.cancel();
        }
    }
}
