//! Reset-credit state. Only an explicit confirmation may start a redemption.
use crate::model::ResetCredit;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResetAttempt {
    pub account_key: String,
    pub credit: ResetCredit,
    pub idempotency_key: String,
}

impl ResetAttempt {
    pub fn new(account_key: String, credit: ResetCredit) -> windows::core::Result<Self> {
        let guid = unsafe { windows::Win32::System::Com::CoCreateGuid()? };
        Ok(Self { account_key, credit, idempotency_key: format!("{guid:?}").to_lowercase() })
    }

    pub fn valid(&self) -> bool {
        self.account_key.len() == 64
            && self.account_key.bytes().all(|b| b.is_ascii_hexdigit())
            && self.credit.id.as_ref().is_some_and(|id| !id.is_empty() && id.len() <= 512)
            && self.idempotency_key.len() == 36
            && self.idempotency_key.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() }
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetOutcome {
    Reset,
    AlreadyRedeemed,
    NothingToReset,
    NoCredit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetHit {
    Open,
    Back,
    Use(usize),
    Cancel,
    Confirm,
    Retry,
}

#[derive(Debug, Default)]
pub struct ResetPanel {
    pub open: bool,
    pub scroll: f32,
    pub hovered: Option<ResetHit>,
    pub pressed: Option<ResetHit>,
    pub confirmation: Option<ResetCredit>,
    pub confirmation_account: Option<String>,
    pub message: Option<String>,
    pub busy: bool,
    pub pending: Option<ResetAttempt>,
    pub storage_blocked: bool,
    pub retrying: bool,
}

impl ResetPanel {
    pub fn may_clear_failed_attempt(&self, may_have_sent: bool) -> bool {
        !self.retrying && !may_have_sent
    }
    pub fn close(&mut self) {
        self.open = false;
        self.confirmation = None;
        self.confirmation_account = None;
        self.pressed = None;
        self.hovered = None;
    }

    pub fn max_scroll(count: usize, height: f32) -> f32 {
        (count as f32 * 60.0 - (height - 92.0)).max(0.0)
    }

    pub fn hit(&self, compact: bool, count: usize, x: f32, y: f32) -> Option<ResetHit> {
        let (w, h) = if compact { (336.0, 284.0) } else { (376.0, 352.0) };
        if !self.open {
            return (if compact {
                inside(x, y, 197.0, 198.0, 315.0, 262.0)
            } else {
                inside(x, y, 203.0, 298.0, 350.0, 335.0)
            })
            .then_some(ResetHit::Open);
        }
        if self.busy {
            return None;
        }
        if self.confirmation.is_some() {
            let top = (h - 166.0) / 2.0;
            if inside(x, y, 40.0, top + 119.0, w / 2.0 - 5.0, top + 151.0) {
                return Some(ResetHit::Cancel);
            }
            if inside(x, y, w / 2.0 + 5.0, top + 119.0, w - 40.0, top + 151.0) {
                return Some(ResetHit::Confirm);
            }
            return (!inside(x, y, 24.0, top, w - 24.0, top + 166.0)).then_some(ResetHit::Cancel);
        }
        if inside(x, y, 14.0, 8.0, 110.0, 38.0) {
            return Some(ResetHit::Back);
        }
        if self.pending.is_some() && inside(x, y, w - 150.0, 46.0, w - 28.0, 74.0) {
            return Some(ResetHit::Retry);
        }
        if inside(x, y, w - 111.0, 76.0, w - 32.0, h - 16.0) {
            let row_y = y - 76.0 + self.scroll;
            let index = (row_y / 60.0).floor() as usize;
            if index < count && (9.0..=43.0).contains(&(row_y % 60.0)) {
                return Some(ResetHit::Use(index));
            }
        }
        None
    }
}

fn inside(x: f32, y: f32, l: f32, t: f32, r: f32, b: f32) -> bool {
    x >= l && x <= r && y >= t && y <= b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_new_definitely_unsent_failures_can_be_forgotten() {
        let mut panel = ResetPanel::default();
        assert!(panel.may_clear_failed_attempt(false));
        assert!(!panel.may_clear_failed_attempt(true));
        panel.retrying = true;
        assert!(!panel.may_clear_failed_attempt(false));
        assert!(!panel.may_clear_failed_attempt(true));
    }
    #[test]
    fn attempt_uuid_and_validation() {
        let credit = ResetCredit { id: Some("fixture".into()), expires_at: None };
        let a = ResetAttempt::new("a".repeat(64), credit.clone()).unwrap();
        let b = ResetAttempt::new("a".repeat(64), credit).unwrap();
        assert!(a.valid());
        assert_ne!(a.idempotency_key, b.idempotency_key);
    }
    #[test]
    fn scroll_and_confirmation_bounds() {
        assert_eq!(ResetPanel::max_scroll(2, 284.0), 0.0);
        assert_eq!(ResetPanel::max_scroll(8, 284.0), 288.0);
        let mut panel = ResetPanel { open: true, ..Default::default() };
        assert_eq!(panel.hit(false, 2, 290.0, 100.0), Some(ResetHit::Use(0)));
        panel.confirmation = Some(ResetCredit { id: None, expires_at: None });
        assert_eq!(panel.hit(false, 2, 280.0, 228.0), Some(ResetHit::Confirm));
        panel.busy = true;
        assert_eq!(panel.hit(false, 2, 280.0, 228.0), None);
    }
}
