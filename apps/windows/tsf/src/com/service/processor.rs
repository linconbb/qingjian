//! `ITfTextInputProcessor`：激活时挂击键 sink、登记翻译保留键、连 Server、起轮询定时器、挂 profile /
//! 转换模式回调、登记语言栏按钮；停用按相反顺序撤掉，敲了一半的拼音先原样落定。

use windows::Win32::UI::TextServices::{
    ITfKeyEventSink, ITfKeystrokeMgr, ITfTextInputProcessor_Impl, ITfThreadMgr,
};
use windows::core::{IUnknownImpl, Interface, Ref, Result};

use qingjian_platform::protocol::SessionId;

use super::{ACTIVE, TextService_Impl};
use crate::com::key::preserved;
use crate::com::log::log;
use crate::com::poll::PollTimer;
use crate::com::profile;

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        log(&format!(
            "Activate 开始 client_id={tid} version={} foreground={} connected={} composing={} composition={}",
            env!("CARGO_PKG_VERSION"),
            self.shared.foreground(),
            self.engine.borrow().is_some(),
            self.shared.composing(),
            self.shared.has_composition()
        ));
        let thread_mgr = match ptim.ok() {
            Ok(thread_mgr) => thread_mgr.clone(),
            Err(error) => {
                log(&format!(
                    "Activate 失败：ITfThreadMgr 为空 client_id={tid}: {error}"
                ));
                return Err(error);
            }
        };
        let keystroke: ITfKeystrokeMgr = match thread_mgr.cast() {
            Ok(keystroke) => keystroke,
            Err(error) => {
                log(&format!(
                    "Activate 失败：获取 ITfKeystrokeMgr client_id={tid}: {error}"
                ));
                return Err(error);
            }
        };
        let sink: ITfKeyEventSink = self.to_interface();
        match unsafe { keystroke.AdviseKeyEventSink(tid, &sink, true) } {
            Ok(()) => log(&format!(
                "AdviseKeyEventSink 成功 client_id={tid} foreground=true"
            )),
            Err(error) => {
                log(&format!(
                    "AdviseKeyEventSink 失败 client_id={tid} foreground=true: {error}"
                ));
                return Err(error);
            }
        }
        let combo = preserved::load_combo();
        match preserved::register(&keystroke, tid, combo) {
            Ok(()) => {
                self.translate_combo.set(Some(combo));
                log(&format!("翻译选中文字快捷键已登记为保留键: {combo}"));
            }
            Err(error) => log(&format!("登记翻译快捷键失败: {error}")),
        }

        self.client_id.set(tid);
        // 连不上 Server、没定时器都不致命。
        self.connect();
        match PollTimer::new(self.engine.clone(), self.shared.clone()) {
            Ok(timer) => *self.poll_timer.borrow_mut() = Some(timer),
            Err(error) => log(&format!("挂云联想轮询定时器失败: {error}")),
        }

        if self.profile_cookie.get().is_none() {
            match profile::advise(&thread_mgr, SessionId(tid as u64)) {
                Ok(cookie) => {
                    self.profile_cookie.set(Some(cookie));
                    log(&format!("输入法 profile 监听成功 cookie={cookie}"));
                }
                Err(error) => log(&format!("监听输入法切换失败: {error}")),
            }
        } else {
            log(&format!(
                "输入法 profile 监听已存在 cookie={:?}",
                self.profile_cookie.get()
            ));
        }
        *self.thread_mgr.borrow_mut() = Some(thread_mgr);
        self.mode_state.set_english(false);
        self.add_lang_bar_item();
        self.refresh_mode_indicator();
        // 放在初始写指示器之后，别被自己那次写触发。
        self.advise_conversion_sink();
        ACTIVE.with(|active| *active.borrow_mut() = Some(self.to_object()));
        log(&format!(
            "Activate 完成 client_id={tid} connected={} profile_cookie={:?} poll_timer={} foreground={} composing={} composition={}",
            self.engine.borrow().is_some(),
            self.profile_cookie.get(),
            self.poll_timer.borrow().is_some(),
            self.shared.foreground(),
            self.shared.composing(),
            self.shared.has_composition()
        ));
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        log(&format!(
            "Deactivate 开始 client_id={} connected={} foreground={} composing={} composition={} translating={}",
            self.client_id.get(),
            self.engine.borrow().is_some(),
            self.shared.foreground(),
            self.shared.composing(),
            self.shared.has_composition(),
            self.shared.translating()
        ));
        // 先撤回调，之后不再有回调碰本服务。
        ACTIVE.with(|active| active.borrow_mut().take());
        self.unadvise_conversion_sink();
        self.remove_lang_bar_item();
        self.poll_timer.borrow_mut().take();
        // 切走输入法时敲了一半的拼音原样落定，再关会话。
        self.commit_pending();
        if let Some(thread_mgr) = self.thread_mgr.borrow_mut().take()
            && let Ok(keystroke) = thread_mgr.cast::<ITfKeystrokeMgr>()
        {
            if let Some(combo) = self.translate_combo.take() {
                preserved::unregister(&keystroke, combo);
                log(&format!("保留键已反注册: {combo}"));
            }
            match unsafe { keystroke.UnadviseKeyEventSink(self.client_id.get()) } {
                Ok(()) => log(&format!(
                    "UnadviseKeyEventSink 成功 client_id={}",
                    self.client_id.get()
                )),
                Err(error) => log(&format!(
                    "UnadviseKeyEventSink 失败 client_id={}: {error}",
                    self.client_id.get()
                )),
            }
        } else {
            log("Deactivate 未找到可反注册的 ITfKeystrokeMgr");
        }
        if let Some(client) = self.engine.borrow_mut().take() {
            match client.close() {
                Ok(()) => log("Server 会话关闭消息已发送"),
                Err(error) => log(&format!("Server 会话关闭消息发送失败: {error}")),
            }
        } else {
            log("Deactivate 时 Server 已断开");
        }
        self.shared.reset();
        self.shared.take_server_stale();
        self.shared.set_foreground(false);
        log(&format!(
            "Deactivate 完成 client_id={} connected=false foreground=false composing=false composition=false",
            self.client_id.get()
        ));
        Ok(())
    }
}
