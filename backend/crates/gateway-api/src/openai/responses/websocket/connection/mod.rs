//! 下游客户端 WebSocket 的连接合同、句柄与单 owner pump

mod frame;
mod handle;
mod pump;
mod state;
mod transport;
mod writer;

pub use frame::{
    ConnectionConfig, ConnectionEvent, ConnectionWriteError, FramePhase, PumpExitReason,
    WriteContext, WriteOutcome,
};
pub use handle::{ResponsesWebSocketConnection, spawn_connection};
pub(super) use transport::Socket;
pub(crate) use transport::Upgrade;
pub(crate) use writer::CancelOnDrop;
