use super::*;
use crate::model::ResetCredit;
use crate::reset::{ResetAttempt, ResetHit, ResetOutcome, ResetPanel};

impl AppState {
    fn reset_credits(&self) -> &[ResetCredit] {
        self.display
            .snapshot
            .as_ref()
            .and_then(|s| s.account.reset_credit_details.as_deref())
            .unwrap_or_default()
    }

    fn reset_hit(&self, lparam: LPARAM) -> Option<ResetHit> {
        if !self.full_history_render
            || (!self.reset_panel.open && self.history_navigation.page != ui::HistoryPage::Main)
        {
            return None;
        }
        let (x, y) = pointer_coordinates(lparam);
        let dpi = unsafe { GetDpiForWindow(self.flyout) }.max(96) as f32;
        self.reset_panel.hit(
            matches!(self.display.quota_availability(), QuotaAvailability::Single(_)),
            self.reset_credits().len(),
            x as f32 * 96.0 / dpi,
            y as f32 * 96.0 / dpi,
        )
    }

    fn reset_invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }

    pub(super) fn reset_pointer_down(&mut self, lparam: LPARAM) -> bool {
        if let Some(hit) = self.reset_hit(lparam) {
            self.reset_panel.pressed = Some(hit);
            unsafe {
                let _ = SetCapture(self.flyout);
            }
            return true;
        }
        self.reset_panel.open
    }

    pub(super) fn reset_pointer_up(&mut self, lparam: LPARAM) -> bool {
        if let Some(hit) = self.reset_panel.pressed.take() {
            let activate = self.reset_hit(lparam) == Some(hit);
            unsafe {
                let _ = ReleaseCapture();
            }
            if activate {
                self.activate_reset(hit);
            }
            self.reset_invalidate();
            return true;
        }
        self.reset_panel.open
    }

    pub(super) fn reset_pointer_move(&mut self, lparam: LPARAM) -> bool {
        let hit = self.reset_hit(lparam);
        if self.reset_panel.hovered != hit {
            self.reset_panel.hovered = hit;
            self.reset_invalidate();
        }
        self.reset_panel.open
    }

    pub(super) fn reset_back(&mut self) {
        if self.reset_panel.busy {
            return;
        }
        if self.reset_panel.confirmation.take().is_none() {
            self.reset_panel.close();
        }
        self.reset_panel.message = None;
        self.reset_invalidate();
    }

    pub(super) fn scroll_reset(&mut self, delta: i32) {
        if self.reset_panel.confirmation.is_some() || self.reset_panel.busy {
            return;
        }
        let height = ui::flyout_dimensions(&self.display).height as f32;
        let max = ResetPanel::max_scroll(self.reset_credits().len(), height);
        let scroll = (self.reset_panel.scroll - delta as f32 / 120.0 * 45.0).clamp(0.0, max);
        if scroll != self.reset_panel.scroll {
            self.reset_panel.scroll = scroll;
            self.reset_panel.hovered = None;
            self.reset_invalidate();
        }
    }

    fn activate_reset(&mut self, hit: ResetHit) {
        match hit {
            ResetHit::Open => {
                self.reset_panel.open = true;
                self.reset_panel.scroll = 0.0;
                self.reset_panel.message = None;
            }
            ResetHit::Back | ResetHit::Cancel => self.reset_back(),
            ResetHit::Use(index) => {
                let credit = self.reset_credits().get(index).cloned();
                if let Some(credit) = credit.filter(|c| {
                    c.id.is_some() && c.expires_at.is_none_or(|time| time > Utc::now().timestamp())
                }) {
                    self.reset_panel.confirmation = Some(credit);
                    self.reset_panel.confirmation_account = self.active_account_key.clone();
                    self.reset_panel.message = None;
                }
            }
            ResetHit::Retry => {
                if let Some(attempt) = &self.reset_panel.pending {
                    self.reset_panel.confirmation = Some(attempt.credit.clone());
                    self.reset_panel.confirmation_account = Some(attempt.account_key.clone());
                    self.reset_panel.message = None;
                }
            }
            ResetHit::Confirm => self.start_reset(),
        }
    }

    fn start_reset(&mut self) {
        if self.reset_panel.storage_blocked {
            self.reset_panel.message = Some(
                self.locale
                    .text(
                        "Request record unreadable; reset disabled",
                        "请求记录无法读取，已禁用重置",
                    )
                    .into(),
            );
            return;
        }
        if self.reset_panel.busy {
            return;
        }
        if self.refreshing {
            self.reset_panel.message =
                Some(self.locale.text("Refreshing; please wait", "正在刷新，请稍后确认").into());
            return;
        }
        let Some(account_key) = self.active_account_key.clone() else {
            self.reset_panel.message = Some(
                self.locale.text("Refresh to verify the account", "请先刷新，确认当前账户").into(),
            );
            return;
        };
        let Some(credit) = self.reset_panel.confirmation.clone() else {
            return;
        };
        if self.reset_panel.confirmation_account.as_ref() != Some(&account_key) {
            self.reset_panel.message = Some(
                self.locale
                    .text("Account changed; cancel and refresh", "账户已切换，请取消并刷新")
                    .into(),
            );
            return;
        }
        self.reset_panel.retrying = self.reset_panel.pending.is_some();
        let attempt = if let Some(pending) = &self.reset_panel.pending {
            if pending.account_key != account_key || pending.credit.id != credit.id {
                self.reset_panel.message = Some(
                    self.locale
                        .text("Resolve the previous attempt first", "请先查询原账户上次操作的结果")
                        .into(),
                );
                return;
            }
            pending.clone()
        } else {
            if !self.reset_credits().iter().any(|c| c == &credit)
                || credit.id.is_none()
                || credit.expires_at.is_some_and(|t| t <= Utc::now().timestamp())
                || self.display.refresh_state != RefreshState::Live
            {
                self.reset_panel.message = Some(
                    self.locale
                        .text("Credit unavailable; refresh first", "券状态已变化，请先刷新")
                        .into(),
                );
                return;
            }
            let Ok(attempt) = ResetAttempt::new(account_key, credit) else {
                self.reset_panel.message = Some(
                    self.locale
                        .text("Could not prepare request", "无法创建请求，请稍后重试")
                        .into(),
                );
                return;
            };
            if self.store.save_reset_attempt(Some(&attempt)).is_err() {
                self.reset_panel.message = Some(
                    self.locale
                        .text(
                            "Could not save request; nothing sent",
                            "无法保存请求记录，未执行重置",
                        )
                        .into(),
                );
                return;
            }
            self.reset_panel.pending = Some(attempt.clone());
            attempt
        };
        self.reset_panel.busy = true;
        self.reset_panel.message = None;
        let client = self.client.clone();
        let sender = self.reset_results_tx.clone();
        let hwnd_value = self.hwnd.0 as isize;
        let worker = thread::Builder::new()
            .name("codex-status-reset".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                let result = client.consume_reset_credit(&attempt);
                if sender.send(result).is_ok() {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_value as *mut std::ffi::c_void)),
                            WM_REFRESH_COMPLETE,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            });
        if worker.is_err() {
            self.reset_panel.busy = false;
            if !self.reset_panel.retrying && self.store.save_reset_attempt(None).is_ok() {
                self.reset_panel.pending = None;
            }
            self.reset_panel.message = Some(
                self.locale.text("Could not start request; retry", "请求未能启动，请重试").into(),
            );
        }
    }

    pub(super) fn finish_reset_if_ready(&mut self) {
        let Ok(result) = self.reset_results.try_recv() else {
            return;
        };
        self.reset_panel.busy = false;
        let not_sent = result
            .as_ref()
            .is_err_and(|failure| self.reset_panel.may_clear_failed_attempt(failure.may_have_sent));
        if result.is_ok() || not_sent {
            // Retain the same key if clearing the durable record fails.
            if self.store.save_reset_attempt(None).is_ok() {
                self.reset_panel.pending = None;
            }
        }
        match result {
            Ok(ResetOutcome::Reset | ResetOutcome::AlreadyRedeemed) => {
                self.reset_panel.close();
                self.history_navigation.page = ui::HistoryPage::Main;
            }
            Ok(ResetOutcome::NothingToReset) => {
                self.reset_panel.message =
                    Some(self.locale.text("No quota needs resetting", "当前无需重置额度").into());
            }
            Ok(ResetOutcome::NoCredit) => {
                self.reset_panel.message = Some(
                    self.locale.text("This credit is unavailable", "这张重置券已不可用").into(),
                );
            }
            Err(_) => {
                // No raw server text/IDs in the panel or logs. An uncertain result
                // can only be retried explicitly with the durable original key.
                self.reset_panel.message = if not_sent {
                    Some(
                        self.locale
                            .text("Nothing sent; refresh and retry", "未执行重置，请刷新后重试")
                            .into(),
                    )
                } else {
                    Some(
                        self.locale
                            .text(
                                "Result unknown; retry with the same request",
                                "结果未确认，请重试查询上次结果",
                            )
                            .into(),
                    )
                };
            }
        }
        self.refresh_pending = false;
        self.start_refresh(true);
        self.reset_invalidate();
    }
}
