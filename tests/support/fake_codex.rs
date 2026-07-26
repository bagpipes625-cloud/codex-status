//! Tiny JSONL fixture server used for manual UI and process-lifecycle tests.
//! It is not part of the CodexStatus binary or release archives.

use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.contains("\"id\":0") {
            writeln!(stdout, r#"{{"id":0,"result":{{}}}}"#).unwrap();
        } else if line.contains("\"id\":1") {
            writeln!(
                stdout,
                r#"{{"id":1,"result":{{"account":{{"type":"chatgpt","planType":"pro"}}}}}}"#
            )
            .unwrap();
        } else if line.contains("\"id\":2") {
            writeln!(
                stdout,
                r#"{{"id":2,"result":{{"rateLimitsByLimitId":{{"codex":{{"limitId":"codex","primary":{{"usedPercent":27,"windowDurationMins":300,"resetsAt":{}}},"secondary":{{"usedPercent":13,"windowDurationMins":10080,"resetsAt":{}}}}}}},"rateLimitResetCredits":{{"availableCount":2}}}}}}"#,
                now + 3 * 60 * 60,
                now + 4 * 24 * 60 * 60 + 7 * 60 * 60
            )
            .unwrap();
        }
        stdout.flush().unwrap();
    }
}
