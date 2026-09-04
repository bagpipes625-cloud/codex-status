//! Primary app-server reads with bounded, read-only repair of missing information.
mod auth;
mod http;

use crate::{
    app_server::{AppServerClient, AppServerFailure, AppServerSnapshot},
    model::{self, QuotaSnapshot, TokenUsageSnapshot},
};
use auth::Credentials;
use http::{Endpoint, Error};
use serde_json::{Value, json};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct FallbackState {
    attempted: [Option<Instant>; 3],
    blocked_until: Option<Instant>,
    credits: Option<CachedCredits>,
}

struct CachedCredits {
    account_key: String,
    account_id: String,
    count: u64,
    details: Vec<model::ResetCredit>,
    observed: Instant,
}

impl FallbackState {
    fn block(&mut self, delay: Duration) {
        let until = Instant::now() + delay;
        self.blocked_until = Some(self.blocked_until.map_or(until, |old| old.max(until)));
    }

    fn retain_credits(&mut self, snapshot: &mut AppServerSnapshot, auth: &Credentials) {
        let account = &mut snapshot.quota.account;
        if account.reset_credits == Some(0) {
            self.credits = None;
            account.reset_credit_details = Some(Vec::new());
            account.reset_credit_expires_at = None;
            return;
        }
        if let (Some(count), Some(details)) = (account.reset_credits, &account.reset_credit_details)
        {
            self.credits = Some(CachedCredits {
                account_key: auth.account_key.clone(),
                account_id: auth.account_id.clone(),
                count,
                details: details.clone(),
                observed: Instant::now(),
            });
            return;
        }
        let now = chrono::Utc::now().timestamp();
        let valid = self.credits.as_ref().is_some_and(|cache| {
            cache.account_key == auth.account_key
                && cache.account_id == auth.account_id
                && account.reset_credits == Some(cache.count)
                && cache.observed.elapsed() < Duration::from_secs(300)
                && cache.details.iter().all(|c| c.expires_at.is_none_or(|expiry| expiry > now))
        });
        if !valid {
            self.credits = None;
            return;
        }
        if account.reset_credit_details.is_none()
            && let Some(cache) = &self.credits
        {
            account.reset_credit_details = Some(cache.details.clone());
            account.reset_credit_expires_at =
                cache.details.iter().filter_map(|c| c.expires_at).min();
        }
    }
    fn reserve(&mut self, requested: &[Endpoint], now: Instant) -> Vec<Endpoint> {
        if self.blocked_until.is_some_and(|until| now < until) {
            return Vec::new();
        }
        requested
            .iter()
            .copied()
            .filter(|endpoint| {
                let slot = &mut self.attempted[endpoint.index()];
                if slot.is_some_and(|last| {
                    now.saturating_duration_since(last) < Duration::from_secs(60)
                }) {
                    return false;
                }
                *slot = Some(now);
                true
            })
            .collect()
    }
}

pub fn fetch(
    client: &AppServerClient,
    state: &Mutex<FallbackState>,
    publish: impl FnMut(AppServerSnapshot),
) -> Result<AppServerSnapshot, AppServerFailure> {
    let before = Credentials::load();
    repair(client.fetch(), before, state, &Credentials::current_identity, &http::get, publish)
}

fn repair(
    mut primary: Result<AppServerSnapshot, AppServerFailure>,
    before: Option<Credentials>,
    state: &Mutex<FallbackState>,
    current: &impl Fn() -> Option<Credentials>,
    query: &(impl Fn(Endpoint, &Credentials) -> Result<Value, Error> + Sync),
    mut publish: impl FnMut(AppServerSnapshot),
) -> Result<AppServerSnapshot, AppServerFailure> {
    let Some(credentials) = before else {
        return primary;
    };
    let Some(identity) = current().filter(|c| credentials.same_account(c)) else {
        return Err(account_changed());
    };
    // A normal credential rotation is not an account switch. Keep valid primary data,
    // but do not issue authenticated fallback requests with the old bearer token.
    if !credentials.same_identity(&identity) {
        return primary;
    }
    let expected = match &primary {
        Ok(s) => s.account_key.as_deref(),
        Err(e) => e.account_key.as_deref(),
    };
    if expected != Some(credentials.account_key.as_str()) {
        return primary;
    }
    let requested = missing(&primary);
    let endpoints = if let Ok(mut state) = state.lock() {
        if let Ok(snapshot) = &mut primary {
            state.retain_credits(snapshot, &credentials);
        }
        if primary.as_ref().is_ok_and(|s| !s.supplement_allowed)
            || primary.as_ref().err().is_some_and(|e| blocked_error(&e.error.to_string()))
        {
            state.block(Duration::from_secs(300));
            Vec::new()
        } else {
            state.reserve(&requested, Instant::now())
        }
    } else {
        Vec::new()
    };
    if endpoints.is_empty() {
        return primary;
    }
    if let Ok(snapshot) = &primary {
        if has_quota(snapshot) {
            publish(snapshot.clone());
        }
    }

    // At most three independently bounded GETs. No detached work or recursive retries.
    let replies = std::thread::scope(|scope| {
        let jobs: Vec<_> = endpoints
            .into_iter()
            .filter_map(|endpoint| {
                let auth = &credentials;
                std::thread::Builder::new()
                    .name("codex-status-read-fallback".into())
                    .stack_size(512 * 1024)
                    .spawn_scoped(scope, move || (endpoint, query(endpoint, auth)))
                    .ok()
            })
            .collect();
        jobs.into_iter().filter_map(|job| job.join().ok()).collect::<Vec<_>>()
    });
    let Some(identity) = current().filter(|c| credentials.same_account(c)) else {
        return Err(account_changed());
    };
    if !credentials.same_identity(&identity) {
        return primary;
    }
    let mut quota = None;
    let mut credits = None;
    let mut tokens = None;
    for (endpoint, reply) in replies {
        match reply {
            Ok(value) => match endpoint {
                Endpoint::Quota => quota = parse_quota(&value, &credentials),
                Endpoint::Credits => credits = parse_credits(&value),
                Endpoint::Tokens => tokens = parse_tokens(&value),
            },
            Err(Error::Status(401 | 403 | 429, delay)) => {
                if let Ok(mut state) = state.lock() {
                    state.block(delay);
                }
            }
            Err(_) => {}
        }
    }
    if let Some(quota) = quota {
        match &mut primary {
            Ok(snapshot) if !has_quota(snapshot) => {
                snapshot.quota.weekly = quota.weekly;
                snapshot.quota.session = quota.session;
                snapshot.quota.fetched_at = quota.fetched_at;
                if snapshot.quota.account.plan_type.is_none() {
                    snapshot.quota.account.plan_type = quota.account.plan_type;
                }
                if snapshot.quota.account.reset_credits.is_none() {
                    snapshot.quota.account.reset_credits = quota.account.reset_credits;
                }
            }
            Err(_) => {
                primary = Ok(AppServerSnapshot {
                    quota,
                    token_usage: primary.as_ref().err().and_then(|e| e.token_usage.clone()),
                    account_key: Some(credentials.account_key.clone()),
                    supplement_allowed: true,
                })
            }
            _ => {}
        }
    }
    if let Ok(snapshot) = &mut primary {
        if let Some((count, details)) = credits {
            // Count and rows form one newer credit response; do not mix its rows with
            // an older count (another device may have redeemed/received a credit).
            snapshot.quota.account.reset_credits = Some(count);
            snapshot.quota.account.reset_credit_expires_at =
                details.iter().filter_map(|c| c.expires_at).min();
            snapshot.quota.account.reset_credit_details = Some(details);
            if let Ok(mut state) = state.lock() {
                state.retain_credits(snapshot, &credentials);
            }
        }
        if snapshot.token_usage.is_none() {
            snapshot.token_usage = tokens;
        }
    }
    primary
}

fn has_quota(snapshot: &AppServerSnapshot) -> bool {
    snapshot.quota.weekly.is_some() || snapshot.quota.session.is_some()
}

fn account_changed() -> AppServerFailure {
    AppServerFailure {
        account_key: None,
        token_usage: None,
        error: crate::app_server::AppServerError::Rpc {
            method: "account/read",
            message: "Account changed during refresh; retry with the current account".into(),
        },
    }
}

pub(crate) fn blocked_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| matches!(part, "429" | "401" | "403"))
        || [
            "unauthorized",
            "forbidden",
            "too many requests",
            "connector_rate_limit",
            "fallback suppressed",
        ]
        .iter()
        .any(|word| message.contains(word))
}

fn missing(primary: &Result<AppServerSnapshot, AppServerFailure>) -> Vec<Endpoint> {
    match primary {
        Err(error) => {
            let mut missing = vec![Endpoint::Quota, Endpoint::Credits];
            if error.token_usage.is_none() {
                missing.push(Endpoint::Tokens);
            }
            missing
        }
        Ok(s) => {
            let mut endpoints = Vec::new();
            if !has_quota(s) {
                endpoints.push(Endpoint::Quota);
            }
            if s.quota.account.reset_credits.is_none()
                || (s.quota.account.reset_credits != Some(0)
                    && s.quota.account.reset_credit_details.is_none())
            {
                endpoints.push(Endpoint::Credits);
            }
            if s.token_usage.is_none() {
                endpoints.push(Endpoint::Tokens);
            }
            endpoints
        }
    }
}

fn parse_quota(value: &Value, auth: &Credentials) -> Option<QuotaSnapshot> {
    if value.get("account_id").and_then(Value::as_str).is_some_and(|id| id != auth.account_id) {
        return None;
    }
    let raw = value.get("rate_limit")?.as_object()?;
    let mut bucket = json!({"planType":value.get("plan_type")});
    for (input, output) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        let Some(window) = raw.get(input).filter(|v| !v.is_null()) else {
            continue;
        };
        let percent = window.get("used_percent")?.as_f64()?;
        let seconds = window.get("limit_window_seconds")?.as_u64()?;
        let reset = window.get("reset_at").and_then(Value::as_i64);
        if !percent.is_finite()
            || !(0.0..=100.0).contains(&percent)
            || seconds == 0
            || seconds % 60 != 0
        {
            return None;
        }
        bucket[output] =
            json!({"usedPercent":percent,"windowDurationMins":seconds/60,"resetsAt":reset});
    }
    let response = json!({"rateLimits":bucket,"rateLimitResetCredits":{
        "availableCount":value.pointer("/rate_limit_reset_credits/available_count")}});
    let quota =
        model::parse_snapshot(&json!({}), &response, chrono::Utc::now().timestamp()).ok()?;
    (quota.weekly.is_some() || quota.session.is_some()).then_some(quota)
}

fn parse_credits(value: &Value) -> Option<(u64, Vec<model::ResetCredit>)> {
    let count = value.get("available_count")?.as_u64()?;
    let rows = value.get("credits")?.as_array()?;
    let converted: Option<Vec<_>> = rows
        .iter()
        .take(256)
        .map(|credit| {
            let expiry = match credit.get("expires_at").filter(|v| !v.is_null()) {
                None => None,
                Some(v) => {
                    Some(chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok()?.timestamp())
                }
            };
            Some(json!({"id":credit.get("id"),"status":credit.get("status"),"expiresAt":expiry}))
        })
        .collect();
    let details =
        model::parse_reset_credits(&json!({"rateLimitResetCredits":{"credits":converted?}}))?;
    Some((count, if count == 0 { Vec::new() } else { details }))
}

fn parse_tokens(value: &Value) -> Option<TokenUsageSnapshot> {
    let buckets = value.pointer("/stats/daily_usage_buckets")?.as_array()?;
    let converted: Vec<_> = buckets
        .iter()
        .map(|b| json!({"startDate":b.get("start_date"),"tokens":b.get("tokens")}))
        .collect();
    Some(model::parse_token_usage(&json!({"dailyUsageBuckets":converted})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn credentials(account: &str, nonce: u64) -> Credentials {
        let jwt = |v: Value| {
            format!(
                "x.{}.x",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
            )
        };
        Credentials::parse(&json!({"tokens":{"account_id":account,
            "access_token":jwt(json!({"exp":4000000000i64,"nonce":nonce,"https://api.openai.com/auth":{"chatgpt_account_id":account}})),
            "id_token":jwt(json!({"email":"test@example.invalid"}))}}),1000).unwrap()
    }

    fn scoped(mut snapshot: AppServerSnapshot) -> AppServerSnapshot {
        snapshot.account_key = Some(credentials("acct", 1).account_key);
        snapshot
    }

    #[test]
    fn complete_primary_never_calls_network_or_publishes_twice() {
        let result = repair(
            Ok(scoped(snapshot(Some(0), None, true))),
            Some(credentials("acct", 1)),
            &Mutex::default(),
            &|| Some(credentials("acct", 1)),
            &|_, _| panic!("unexpected HTTP"),
            |_| panic!("unexpected interim"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn quota_failure_does_not_requery_successful_tokens() {
        let usage = TokenUsageSnapshot {
            daily_buckets: vec![model::DailyTokenUsage {
                start_date: "2026-09-04".into(),
                tokens: 42,
            }],
        };
        let failure = AppServerFailure {
            error: crate::app_server::AppServerError::Timeout,
            account_key: Some(credentials("acct", 1).account_key),
            token_usage: Some(usage.clone()),
        };
        let result=repair(Err(failure),Some(credentials("acct",1)),&Mutex::default(),&||Some(credentials("acct",1)),
            &|endpoint,_|match endpoint {
                Endpoint::Quota=>Ok(json!({"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_at":2000000000}}})),
                Endpoint::Credits=>Err(Error::Network),Endpoint::Tokens=>panic!("already received tokens")
            },|_|{}).unwrap();
        assert_eq!(result.token_usage, Some(usage));
    }

    #[test]
    fn token_rotation_after_network_keeps_valid_primary_and_discards_supplement() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = repair(
            Ok(scoped(snapshot(Some(2), None, true))),
            Some(credentials("acct", 1)),
            &Mutex::default(),
            &|| {
                Some(credentials(
                    "acct",
                    if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        1
                    } else {
                        2
                    },
                ))
            },
            &|_, _| Ok(json!({"available_count":0,"credits":[]})),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.quota.account.reset_credits, Some(2));
    }

    #[test]
    fn failed_optional_request_does_not_erase_successful_primary_or_other_supplement() {
        let calls = Mutex::new(Vec::new());
        let mut interim = Vec::new();
        let primary = scoped(snapshot(Some(2), None, false));
        let initial = primary.quota.clone();
        let final_result=repair(Ok(primary),Some(credentials("acct",1)),&Mutex::default(),&||Some(credentials("acct",1)),
            &|endpoint,_| { calls.lock().unwrap().push(endpoint); match endpoint {
                Endpoint::Credits=>Ok(json!({"available_count":1,"credits":[{"id":"new","status":"available","expires_at":"2030-01-01T00:00:00Z"}]})),
                Endpoint::Tokens=>Err(Error::Network),Endpoint::Quota=>panic!("quota already valid")
            }},|s|interim.push(s)).unwrap();
        assert_eq!(interim.len(), 1);
        assert_eq!(interim[0].quota, initial);
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert_eq!(final_result.quota.weekly, initial.weekly);
        assert_eq!(final_result.quota.fetched_at, initial.fetched_at);
        assert_eq!(final_result.quota.account.reset_credits, Some(1));
        assert_eq!(
            final_result.quota.account.reset_credit_details.unwrap()[0].id.as_deref(),
            Some("new")
        );
        assert!(final_result.token_usage.is_none());
    }

    #[test]
    fn account_change_discards_late_results_but_token_rotation_keeps_primary() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = repair(
            Ok(scoped(snapshot(Some(2), None, true))),
            Some(credentials("acct", 1)),
            &Mutex::default(),
            &|| {
                Some(credentials(
                    if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        "acct"
                    } else {
                        "other"
                    },
                    1,
                ))
            },
            &|_, _| Ok(json!({"available_count":0,"credits":[]})),
            |_| {},
        );
        assert!(result.unwrap_err().account_key.is_none());
        let result = repair(
            Ok(scoped(snapshot(Some(2), None, true))),
            Some(credentials("acct", 1)),
            &Mutex::default(),
            &|| Some(credentials("acct", 2)),
            &|_, _| panic!("old bearer must not be used"),
            |_| {},
        );
        assert!(result.is_ok());
    }

    #[test]
    fn empty_quota_success_is_repaired_without_replacing_other_fields() {
        let mut primary = scoped(snapshot(Some(0), None, true));
        primary.quota.weekly = None;
        let result=repair(Ok(primary),Some(credentials("acct",1)),&Mutex::default(),&||Some(credentials("acct",1)),
            &|endpoint,_| {assert_eq!(endpoint,Endpoint::Quota);Ok(json!({"account_id":"acct","rate_limit":{"primary_window":{
                "used_percent":20,"limit_window_seconds":604800,"reset_at":2000000000}},"rate_limit_reset_credits":{"available_count":3}}))},
            |_|panic!("do not publish blank quota interim")).unwrap();
        assert_eq!(result.quota.weekly.unwrap().used_percent, 20.0);
        assert_eq!(result.quota.account.reset_credits, Some(0));
        assert!(result.token_usage.is_some());
    }

    #[test]
    fn longest_retry_after_wins_and_suppressed_primary_never_queries() {
        let mut state = FallbackState::default();
        state.block(Duration::from_secs(3600));
        let until = state.blocked_until;
        state.block(Duration::from_secs(300));
        assert_eq!(state.blocked_until, until);
        let mut primary = scoped(snapshot(Some(2), None, false));
        primary.supplement_allowed = false;
        assert!(
            repair(
                Ok(primary),
                Some(credentials("acct", 1)),
                &Mutex::new(state),
                &|| Some(credentials("acct", 1)),
                &|_, _| panic!("rate-limited"),
                |_| {}
            )
            .is_ok()
        );
    }

    #[test]
    fn credit_cache_survives_cooldown_but_not_account_count_or_expiry_changes() {
        let auth = credentials("acct", 1);
        let mut state = FallbackState::default();
        let mut good = scoped(snapshot(
            Some(2),
            Some(vec![model::ResetCredit {
                id: Some("fixture".into()),
                expires_at: Some(2000000000),
            }]),
            true,
        ));
        state.retain_credits(&mut good, &auth);
        state.reserve(&[Endpoint::Credits], Instant::now());
        let shared = Mutex::new(state);
        let result = repair(
            Ok(scoped(snapshot(Some(2), None, true))),
            Some(auth),
            &shared,
            &|| Some(credentials("acct", 1)),
            &|_, _| panic!("cooldown"),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            result.quota.account.reset_credit_details,
            good.quota.account.reset_credit_details
        );
        let mut changed = scoped(snapshot(Some(1), None, true));
        shared.lock().unwrap().retain_credits(&mut changed, &credentials("acct", 1));
        assert!(changed.quota.account.reset_credit_details.is_none());
        let mut state = shared.lock().unwrap();
        state.retain_credits(&mut good, &credentials("acct", 1));
        let mut other = scoped(snapshot(Some(2), None, true));
        state.retain_credits(&mut other, &credentials("other", 1));
        assert!(other.quota.account.reset_credit_details.is_none());
        state.retain_credits(&mut good, &credentials("acct", 1));
        state.credits.as_mut().unwrap().observed = Instant::now() - Duration::from_secs(301);
        state.retain_credits(&mut other, &credentials("acct", 1));
        assert!(other.quota.account.reset_credit_details.is_none());
    }

    fn snapshot(
        count: Option<u64>,
        details: Option<Vec<model::ResetCredit>>,
        usage: bool,
    ) -> AppServerSnapshot {
        AppServerSnapshot {
            quota: QuotaSnapshot {
                weekly: Some(model::QuotaWindow {
                    used_percent: 10.0,
                    remaining_percent: 90.0,
                    window_minutes: 10080,
                    resets_at: Some(2000000000),
                }),
                session: None,
                fetched_at: 1,
                account: model::AccountSummary {
                    reset_credits: count,
                    reset_credit_details: details,
                    ..Default::default()
                },
            },
            token_usage: usage.then(TokenUsageSnapshot::default),
            account_key: Some("fixture".into()),
            supplement_allowed: true,
        }
    }

    #[test]
    fn supplements_only_missing_fields_not_valid_empty_results() {
        assert!(missing(&Ok(snapshot(Some(0), None, true))).is_empty());
        assert!(missing(&Ok(snapshot(Some(2), Some(vec![]), true))).is_empty());
        assert_eq!(missing(&Ok(snapshot(Some(2), None, true))), vec![Endpoint::Credits]);
        assert_eq!(missing(&Ok(snapshot(Some(0), None, false))), vec![Endpoint::Tokens]);
        assert_eq!(
            missing(&Ok(snapshot(None, None, false))),
            vec![Endpoint::Credits, Endpoint::Tokens]
        );
    }

    #[test]
    fn authentication_and_rate_limit_signals_do_not_match_generic_quota_errors() {
        for text in [
            "HTTP status 429",
            "401 Unauthorized",
            "403 Forbidden",
            "connector_rate_limit",
            "Too many requests",
        ] {
            assert!(blocked_error(text), "{text}");
        }
        for text in [
            "failed to fetch codex rate limits: timed out",
            "duration=14019",
            "rate limit reset credit detail request timed out",
        ] {
            assert!(!blocked_error(text), "{text}");
        }
    }

    #[test]
    #[ignore = "Explicit opt-in only: real GET reads; never consumes credits"]
    fn read_only_live_probe() {
        assert_eq!(std::env::var("CODEX_STATUS_ALLOW_READ_PROBE").as_deref(), Ok("1"));
        let auth = Credentials::load().expect("Current supported local credentials are required");
        for endpoint in [Endpoint::Quota, Endpoint::Credits, Endpoint::Tokens] {
            let start = Instant::now();
            let reply = http::get(endpoint, &auth);
            let valid = match &reply {
                Ok(v) => match endpoint {
                    Endpoint::Quota => parse_quota(v, &auth).is_some(),
                    Endpoint::Credits => parse_credits(v).is_some(),
                    Endpoint::Tokens => parse_tokens(v).is_some(),
                },
                Err(_) => false,
            };
            eprintln!(
                "read-only {endpoint:?}: transport_ok={}, parsed={valid}, elapsed_ms={}",
                reply.is_ok(),
                start.elapsed().as_millis()
            );
            if matches!(reply, Err(Error::Status(401 | 403 | 429, _))) {
                break;
            }
        }
        assert!(
            Credentials::load().is_some_and(|current| auth.same_identity(&current)),
            "Credentials changed during probe"
        );
    }
    #[test]
    fn cooldown_and_global_backoff_prevent_manual_request_storms() {
        let mut state = FallbackState::default();
        let now = Instant::now();
        assert_eq!(state.reserve(&[Endpoint::Credits], now), vec![Endpoint::Credits]);
        assert!(state.reserve(&[Endpoint::Credits], now + Duration::from_secs(59)).is_empty());
        assert_eq!(state.reserve(&[Endpoint::Tokens], now), vec![Endpoint::Tokens]);
        state.blocked_until = Some(now + Duration::from_secs(300));
        assert!(state.reserve(&[Endpoint::Quota], now + Duration::from_secs(60)).is_empty());
    }
    #[test]
    fn empty_missing_and_malformed_credits_are_distinct() {
        assert!(parse_credits(&json!({"available_count":2,"credits":null})).is_none());
        assert_eq!(parse_credits(&json!({"available_count":0,"credits":[]})), Some((0, vec![])));
        assert!(
            parse_credits(&json!({"available_count":2,"credits":[{"expires_at":"bad"}]})).is_none()
        );
        let (count, rows) = parse_credits(&json!({"available_count":2,"credits":[
            {"id":"b","status":"available","expires_at":"2030-01-02T00:00:00Z"},
            {"id":"a","status":"available","expires_at":"2030-01-01T00:00:00Z"}]}))
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(rows[0].id.as_deref(), Some("a"));
    }
    #[test]
    fn tokens_only_mirror_official_days() {
        assert!(parse_tokens(&json!({"stats":{}})).is_none());
        let data = parse_tokens(
            &json!({"stats":{"daily_usage_buckets":[{"start_date":"2026-09-04","tokens":42}]}}),
        )
        .unwrap();
        assert_eq!(data.daily_buckets.len(), 1);
        assert_eq!(data.daily_buckets[0].tokens, 42);
    }
}
