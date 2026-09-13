//! 连 Server：激活 / 获焦时开会话，失败按 [`RECONNECT_INTERVAL`] 退避重试；转发出错就断开、下一键重连。

use std::time::Instant;

use qingjian_platform::protocol::SessionId;

use super::{RECONNECT_INTERVAL, TextService_Impl};
use crate::client::EngineClient;
use crate::client::pipe::connect_default;
use crate::com::log::log;

impl TextService_Impl {
    /// 连 Server 并开会话（会话 id 用 TSF 的 client id，带上宿主 exe 名）。
    pub(super) fn connect(&self) {
        let session = SessionId(self.client_id.get() as u64);
        let app = crate::com::host_app_name();
        log(&format!(
            "Server IPC 连接开始 session={session:?} app={:?}",
            app.as_deref().unwrap_or("<unknown>")
        ));
        let connected = match connect_default() {
            Ok(stream) => {
                log(&format!("Server 命名管道已连接 session={session:?}"));
                EngineClient::open(stream, session, app).map_err(|e| e.to_string())
            }
            Err(error) => {
                log(&format!(
                    "Server 命名管道连接失败 session={session:?}: {error}"
                ));
                Err(error.to_string())
            }
        };
        match connected {
            Ok(client) => {
                *self.engine.borrow_mut() = Some(client);
                self.last_connect_failure.set(None);
                log(&format!("Server 会话已打开 session={session:?}"));
            }
            Err(error) => {
                self.last_connect_failure.set(Some(Instant::now()));
                log(&format!(
                    "连 Server 失败（qingjian-server 没起？）: {error}"
                ));
            }
        }
    }

    /// 没连上就重连一次（距上次失败不到 [`RECONNECT_INTERVAL`] 则跳过）。返回此刻是否连着。
    pub(super) fn ensure_connected(&self) -> bool {
        if self.engine.borrow().is_some() {
            log("Server IPC 状态 connected=true");
            return true;
        }
        let recently_failed = self
            .last_connect_failure
            .get()
            .is_some_and(|at| at.elapsed() < RECONNECT_INTERVAL);
        if recently_failed {
            log("Server IPC 状态 connected=false retry=backoff");
            return false;
        }
        self.connect();
        let connected = self.engine.borrow().is_some();
        log(&format!(
            "Server IPC 重连结果 connected={connected} retry=attempted"
        ));
        connected
    }

    /// 转发失败后断开，下一键重连。
    pub(super) fn disconnect(&self) {
        log(&format!(
            "Server IPC 主动断开 connected_before={} composing={} composition={}",
            self.engine.borrow().is_some(),
            self.shared.composing(),
            self.shared.has_composition()
        ));
        *self.engine.borrow_mut() = None;
        self.last_connect_failure.set(None);
        self.shared.end_composing();
    }
}
