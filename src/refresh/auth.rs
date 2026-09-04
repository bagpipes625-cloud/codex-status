use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::PathBuf};

// Never Debug/Serialize credentials or retain them in application state.
pub(super) struct Credentials {
    pub token: String,
    pub account_id: String,
    pub account_key: String,
    fingerprint: [u8; 32],
}

impl Credentials {
    pub fn load() -> Option<Self> {
        Self::load_at(chrono::Utc::now().timestamp())
    }

    pub fn current_identity() -> Option<Self> {
        Self::load_at(0)
    }

    fn load_at(now: i64) -> Option<Self> {
        // An explicit executable can be a test fixture or use a different identity store.
        if std::env::var_os("CODEX_STATUS_CODEX").is_some() {
            return None;
        }
        let root = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".codex")))?;
        let mut bytes = Vec::new();
        File::open(root.join("auth.json")).ok()?.take(65_537).read_to_end(&mut bytes).ok()?;
        if bytes.len() > 65_536 {
            return None;
        }
        Self::parse(&serde_json::from_slice::<Value>(&bytes).ok()?, now)
    }

    pub(super) fn parse(value: &Value, now: i64) -> Option<Self> {
        let tokens = value.get("tokens")?;
        let token = tokens.get("access_token")?.as_str()?;
        let account_id = tokens.get("account_id")?.as_str()?;
        if !header_value(token, 16_384) || !header_value(account_id, 512) {
            return None;
        }
        let access = claims(token)?;
        let identity = claims(tokens.get("id_token")?.as_str()?)?;
        if access.get("exp")?.as_i64()? <= now + 30 {
            return None;
        }
        if access.get("https://api.openai.com/auth")?.get("chatgpt_account_id")?.as_str()?
            != account_id
        {
            return None;
        }
        // Claims are used only for local binding; the HTTPS service authenticates the token.
        let email = identity.get("email")?.as_str()?;
        let account_key =
            crate::app_server::account_key(&json!({"account":{"type":"chatgpt","email":email}}))?;
        let fingerprint = Sha256::digest(format!("{account_id}\0{token}").as_bytes()).into();
        Some(Self { token: token.into(), account_id: account_id.into(), account_key, fingerprint })
    }

    pub fn same_account(&self, other: &Self) -> bool {
        self.account_key == other.account_key && self.account_id == other.account_id
    }

    pub fn same_identity(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint && self.account_key == other.account_key
    }
}

fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    if payload.len() > 24_000 {
        return None;
    }
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

fn header_value(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && value.bytes().all(|b| (33..=126).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn jwt(v: Value) -> String {
        format!("x.{}.x", URL_SAFE_NO_PAD.encode(v.to_string()))
    }
    fn fixture() -> Value {
        json!({"tokens":{"account_id":"acct", "access_token":jwt(json!({"exp":2000,
            "https://api.openai.com/auth":{"chatgpt_account_id":"acct"}})),
            "id_token":jwt(json!({"email":"test@example.invalid"}))}})
    }
    #[test]
    fn credentials_bind_account_and_expiry_without_exposing_secrets() {
        let v = fixture();
        assert!(Credentials::parse(&v, 1000).is_some());
        assert!(Credentials::parse(&v, 1990).is_none());
        let mut changed = v.clone();
        changed["tokens"]["account_id"] = json!("other");
        assert!(Credentials::parse(&changed, 1000).is_none());
        changed["tokens"]["account_id"] = json!("acct\r\nInjected: yes");
        assert!(Credentials::parse(&changed, 1000).is_none());
        assert!(Credentials::parse(&json!({}), 1000).is_none());
    }
}
