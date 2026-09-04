//! Network-free JSONL fixture; excluded from release assets.
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
fn main() {
    let now = chrono::Utc::now().timestamp();
    let directory = std::env::var_os("CODEX_STATUS_FIXTURE_DIR")
        .map(std::path::PathBuf::from)
        .expect("Explicit fixture directory required");
    std::fs::create_dir_all(&directory).unwrap();
    for line in io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id") else {
            continue;
        };
        let method = request["method"].as_str().unwrap_or_default();
        let spent = directory.join("spent.json");
        let consumed = spent.exists();
        if consumed && method == "account/rateLimits/read" {
            std::fs::write(directory.join("post-reset-refresh.json"), request.to_string()).unwrap();
        }
        let result = match method {
            "initialize" => json!({}),
            "account/read" => {
                json!({"account":{"type":"chatgpt","planType":"pro","email":"fixture@example.invalid"}})
            }
            "account/rateLimits/read" => json!({
                "rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":if consumed {0}else{27},"windowDurationMins":300,"resetsAt":now+10800},
                    "secondary":{"usedPercent":13,"windowDurationMins":10080,"resetsAt":now+432000}}},
                "rateLimitResetCredits":{"availableCount":if consumed {1}else{2},"credits":
                    if consumed {json!([{"id":"fixture-2","status":"available","expiresAt":now+172800}])}
                    else {json!([{"id":"fixture-1","status":"available","expiresAt":now+86400},{"id":"fixture-2","status":"available","expiresAt":now+172800}])}}
            }),
            "account/usage/read" => {
                json!({"dailyUsageBuckets":[{"startDate":chrono::Local::now().format("%Y-%m-%d").to_string(),"tokens":480000000}]})
            }
            "account/rateLimitResetCredit/consume" => {
                assert_eq!(request["params"]["creditId"], "fixture-1");
                if consumed {
                    let old: Value =
                        serde_json::from_slice(&std::fs::read(&spent).unwrap()).unwrap();
                    assert_eq!(
                        old["params"]["idempotencyKey"],
                        request["params"]["idempotencyKey"]
                    );
                    json!({"outcome":"alreadyRedeemed"})
                } else {
                    std::fs::write(&spent, request.to_string()).unwrap();
                    json!({"outcome":"reset"})
                }
            }
            _ => continue,
        };
        println!("{}", json!({"id":id,"result":result}));
        io::stdout().flush().unwrap();
    }
}
