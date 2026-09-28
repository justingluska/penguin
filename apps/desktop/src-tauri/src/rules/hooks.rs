//! Power-user actions: a local program (argv only, the match as JSON on
//! stdin) and a webhook (POST JSON, optional HMAC-SHA256 signature). Both
//! are off unless `RulesConfig.allow_hooks`, and never run for dry-run
//! rules.
//!
//! Email content never reaches an argument list or a shell: the program and
//! its arguments come from the rule as the user typed them; everything about
//! the message goes through stdin (and a few id-only env vars). Payloads use
//! the CLI's JSON v1 message shape (`agent::output::MessageOut`), schema in
//! docs/cli-schemas/ruleMatch.v1.schema.json.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use penguin_core::Message;
use rmcp::schemars::JsonSchema;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::agent::output::{self, Envelope, MessageOut, ThreadOptions};

pub const PAYLOAD_KIND: &str = "ruleMatch";
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest stderr excerpt kept in the rule log when a hook fails.
const MAX_STDERR_LOG: usize = 300;
/// Hard cap on message text sent to hooks (per message, in characters).
const MAX_BODY_CHARS: usize = 100_000;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct RuleRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct MatchedMessageOut {
    pub account_id: String,
    pub thread_id: String,
    pub message: MessageOut,
}

/// `data` of a `ruleMatch` payload.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(crate = "rmcp::schemars")]
pub struct RuleMatchOut {
    pub rule: RuleRef,
    /// "newMessage" | "labelAdded" | "schedule" | "manual"
    pub trigger: String,
    /// False: `text` is empty and `fullText` null (the rule doesn't opt in
    /// to sending message bodies).
    pub includes_body: bool,
    /// One message for new-mail and label triggers; every match for a
    /// scheduled or manual run.
    pub messages: Vec<MatchedMessageOut>,
}

pub fn payload(
    rule: RuleRef,
    trigger: &str,
    include_body: bool,
    messages: &[Message],
) -> Envelope<RuleMatchOut> {
    let opts = ThreadOptions {
        include_full_text: false,
        max_chars_per_message: Some(MAX_BODY_CHARS),
    };
    let messages = messages
        .iter()
        .map(|m| {
            let mut out = output::message_out(m, opts, false);
            if !include_body {
                out.text = String::new();
                out.quoted_chars = 0;
                out.full_text = None;
                out.truncated = false;
            }
            MatchedMessageOut {
                account_id: m.account_id.clone(),
                thread_id: m.thread_id.clone(),
                message: out,
            }
        })
        .collect();
    output::envelope(
        PAYLOAD_KIND,
        RuleMatchOut {
            rule,
            trigger: trigger.to_string(),
            includes_body: include_body,
            messages,
        },
    )
}

// ---------- HMAC-SHA256 (RFC 2104) ----------

pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `X-Penguin-Signature` value: `t=<unix secs>,v1=<hex HMAC of "<t>.<body>">`.
/// Signing the timestamp lets receivers reject replays.
pub fn signature_header(secret: &str, timestamp_secs: i64, body: &[u8]) -> String {
    let mut signed = format!("{timestamp_secs}.").into_bytes();
    signed.extend_from_slice(body);
    format!(
        "t={timestamp_secs},v1={}",
        hex(&hmac_sha256(secret.as_bytes(), &signed))
    )
}

// ---------- shell hook ----------

/// Environment for a hook: a minimal fixed base plus id-only PENGUIN_* vars
/// (never subjects, addresses or text; those are on stdin).
pub fn hook_env(vars: &[(&str, String)]) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![(
        "PATH".into(),
        "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
    )];
    for key in ["HOME", "USER", "LOGNAME", "LANG", "TMPDIR"] {
        if let Ok(v) = std::env::var(key) {
            env.push((key.into(), v));
        }
    }
    for (k, v) in vars {
        // Ids and emails only; drop anything that could smuggle a newline.
        if v.chars().all(|c| !c.is_control()) {
            env.push((k.to_string(), v.clone()));
        }
    }
    env
}

/// Run `program args…` with `stdin` and `env` (the environment is replaced,
/// not inherited). Killed after `timeout`. Ok = exit status 0.
pub async fn run_hook(
    program: &Path,
    args: &[String],
    env: &[(String, String)],
    stdin: &[u8],
    timeout: Duration,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", program.display()))?;
    let mut pipe = child.stdin.take();
    let input = stdin.to_vec();
    let run = async move {
        if let Some(mut p) = pipe.take() {
            // A hook that ignores stdin closes it early; that's fine.
            let _ = p.write_all(&input).await;
            drop(p);
        }
        child.wait_with_output().await
    };
    match tokio::time::timeout(timeout, run).await {
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
        Ok(Err(e)) => Err(format!("hook failed: {e}")),
        Ok(Ok(out)) if out.status.success() => Ok("exit 0".into()),
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let excerpt: String = stderr.trim().chars().take(MAX_STDERR_LOG).collect();
            let code = out
                .status
                .code()
                .map_or("a signal".to_string(), |c| format!("status {c}"));
            Err(if excerpt.is_empty() {
                format!("exited with {code}")
            } else {
                format!("exited with {code}: {excerpt}")
            })
        }
    }
}

// ---------- webhook ----------

/// POST `body` (JSON). No redirects are followed, so a signed payload can't
/// be bounced to another host.
pub async fn post_webhook(
    url: &str,
    secret: Option<&str>,
    body: Vec<u8>,
    now_ms: i64,
) -> Result<String, String> {
    let url = super::model::valid_webhook_url(url)?;
    let client = reqwest::Client::builder()
        .timeout(WEBHOOK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("Penguin/{}", crate::VERSION))
        .build()
        .map_err(|e| e.to_string())?;
    let ts = now_ms / 1000;
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-Penguin-Event", PAYLOAD_KIND)
        .header("X-Penguin-Timestamp", ts.to_string());
    if let Some(secret) = secret {
        req = req.header("X-Penguin-Signature", signature_header(secret, ts, &body));
    }
    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|e| format!("couldn't reach the webhook: {}", e.without_url()))?;
    let status = resp.status();
    if status.is_success() {
        Ok(format!("HTTP {}", status.as_u16()))
    } else {
        Err(format!("webhook answered HTTP {}", status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 test cases 1, 2 and 6 (a key longer than the block size).
    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn signature_covers_timestamp_and_body() {
        let a = signature_header("s3cret", 1_700_000_000, br#"{"a":1}"#);
        assert!(a.starts_with("t=1700000000,v1="));
        let mac = hmac_sha256(b"s3cret", br#"1700000000.{"a":1}"#);
        assert_eq!(a, format!("t=1700000000,v1={}", hex(&mac)));
        assert_ne!(a, signature_header("s3cret", 1_700_000_001, br#"{"a":1}"#));
        assert_ne!(a, signature_header("s3cret", 1_700_000_000, br#"{"a":2}"#));
        assert_ne!(a, signature_header("other", 1_700_000_000, br#"{"a":1}"#));
    }

    #[test]
    fn env_is_minimal_and_drops_control_characters() {
        let env = hook_env(&[
            ("PENGUIN_ACCOUNT", "me@x.example".into()),
            ("PENGUIN_THREAD_ID", "18c\nrm -rf ~".into()),
        ]);
        assert!(env.iter().any(|(k, _)| k == "PENGUIN_ACCOUNT"));
        assert!(!env.iter().any(|(k, _)| k == "PENGUIN_THREAD_ID"));
        assert!(env.iter().any(|(k, _)| k == "PATH"));
        assert!(!env.iter().any(|(k, _)| k == "CARGO_PKG_NAME"));
    }

    /// A subject full of shell metacharacters reaches the hook only as JSON
    /// on stdin; the argv is exactly what the rule says.
    #[cfg(unix)]
    #[tokio::test]
    async fn hook_gets_argv_verbatim_and_the_match_on_stdin() {
        let dir = std::env::temp_dir().join(format!("penguin-hook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out");
        let canary = dir.join("pwned");
        let script = dir.join("hook.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$@\" > '{o}'\ncat >> '{o}'\nprintf '\\n%s' \"$PENGUIN_RULE_ID\" >> '{o}'\n",
                o = out.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();

        let evil = format!(
            "$(touch {c}) `touch {c}`; touch {c} && echo",
            c = canary.display()
        );
        let mut m = crate::rules::engine::tests::message("me@x.example", "m1", "t1");
        m.subject = evil.clone();
        m.from.name = Some(format!("\"; touch {} #", canary.display()));
        let body = serde_json::to_vec(&payload(
            RuleRef {
                id: "r_1".into(),
                name: "x".into(),
            },
            "newMessage",
            false,
            &[m],
        ))
        .unwrap();
        let args = vec!["--flag".to_string(), "two words".to_string()];
        let env = hook_env(&[("PENGUIN_RULE_ID", "r_1".into())]);
        run_hook(&script, &args, &env, &body, Duration::from_secs(10))
            .await
            .unwrap();

        let got = std::fs::read_to_string(&out).unwrap();
        let mut lines = got.lines();
        assert_eq!(lines.next(), Some("2"));
        assert_eq!(lines.next(), Some("--flag"));
        assert_eq!(lines.next(), Some("two words"));
        let json: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(json["kind"], "ruleMatch");
        assert_eq!(json["data"]["messages"][0]["message"]["subject"], evil);
        assert_eq!(json["data"]["messages"][0]["message"]["text"], "");
        assert_eq!(lines.next(), Some("r_1"));
        assert!(!canary.exists(), "the subject was executed");

        // Non-zero exit and timeouts are failures, not hangs.
        let fail = dir.join("fail.sh");
        std::fs::write(&fail, "#!/bin/sh\necho nope >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&fail, std::fs::Permissions::from_mode(0o700)).unwrap();
        let e = run_hook(&fail, &[], &env, b"{}", Duration::from_secs(10))
            .await
            .unwrap_err();
        assert_eq!(e, "exited with status 3: nope");
        let slow = dir.join("slow.sh");
        std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o700)).unwrap();
        let e = run_hook(&slow, &[], &env, b"{}", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(e.contains("timed out"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The payload is the documented, versioned contract for scripts.
    #[test]
    fn schema_matches_docs() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/cli-schemas");
        let path = dir.join(format!(
            "{PAYLOAD_KIND}.v{}.schema.json",
            output::SCHEMA_VERSION
        ));
        let schema = rmcp::schemars::schema_for!(Envelope<RuleMatchOut>);
        let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMAS").is_some() {
            std::fs::write(&path, &generated).unwrap();
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_SCHEMAS=1", path.display()));
        assert_eq!(on_disk, generated, "ruleMatch schema changed");
    }
}
