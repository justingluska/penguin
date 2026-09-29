//! Share links: keys, settings, the Keychain seam (faked), signing, error
//! wording, the cleanup scheduler, the agent gate, and PUT/GET/DELETE end
//! to end against an in-process S3 stub on 127.0.0.1. No real storage.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use penguin_provider::credentials::SecretBackend;

use super::config::{self, LinkLifetime, SecretStore, ShareLinkConfigInput, StoredConfig};
use super::keys;
use super::s3::{self, S3Error, Signer};
use super::stub::{xml, Stub};
use super::uploads::{self, Upload, Uploads};
use super::{check_caller, Caller, Share};
use crate::error::ErrorCode;

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// The Keychain, in memory, counting reads.
#[derive(Default)]
struct MemoryKeychain {
    items: Mutex<HashMap<String, String>>,
    reads: AtomicUsize,
}

/// The backend handed to a SecretStore, sharing the memory with the test.
struct Fake(Arc<MemoryKeychain>);

impl SecretBackend for Fake {
    fn get(&self, account: &str) -> penguin_provider::Result<Option<String>> {
        self.0.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> penguin_provider::Result<()> {
        self.0.set(account, secret)
    }
    fn delete(&self, account: &str) -> penguin_provider::Result<()> {
        self.0.delete(account)
    }
}

impl MemoryKeychain {
    fn get(&self, account: &str) -> penguin_provider::Result<Option<String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.items.lock().unwrap().get(account).cloned())
    }
    fn set(&self, account: &str, secret: &str) -> penguin_provider::Result<()> {
        self.items
            .lock()
            .unwrap()
            .insert(account.into(), secret.into());
        Ok(())
    }
    fn delete(&self, account: &str) -> penguin_provider::Result<()> {
        self.items.lock().unwrap().remove(account);
        Ok(())
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "penguin-share-{tag}-{}-{}",
        std::process::id(),
        keys::token().unwrap()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const KEY_ID: &str = "0123456789abcdef0123456789abcdef";
const SECRET: &str = "fictional-secret-0123456789abcdef0123456789abcdef";

fn input(endpoint: &str) -> ShareLinkConfigInput {
    ShareLinkConfigInput {
        endpoint: endpoint.into(),
        bucket: "penguin-shares".into(),
        region: String::new(),
        access_key_id: KEY_ID.into(),
        secret_access_key: Some(SECRET.into()),
        lifetime: LinkLifetime::Day,
        delete_on_expiry: true,
        allow_agents: false,
    }
}

fn share_at(tag: &str) -> (Arc<Share>, Arc<MemoryKeychain>, std::path::PathBuf) {
    let dir = temp_dir(tag);
    let keychain = Arc::new(MemoryKeychain::default());
    let share = Arc::new(Share::new(
        dir.clone(),
        &dir,
        SecretStore::new(Box::new(Fake(keychain.clone()))),
    ));
    (share, keychain, dir)
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// 2026-09-29T12:00:00Z
const NOON: u64 = 1_790_683_200;

fn query(url: &url::Url) -> HashMap<String, String> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

#[test]
fn base32_matches_rfc_4648() {
    // RFC 4648 §10 test vectors, lowercase and unpadded.
    assert_eq!(keys::base32(b""), "");
    assert_eq!(keys::base32(b"f"), "my");
    assert_eq!(keys::base32(b"fo"), "mzxq");
    assert_eq!(keys::base32(b"foo"), "mzxw6");
    assert_eq!(keys::base32(b"foob"), "mzxw6yq");
    assert_eq!(keys::base32(b"fooba"), "mzxw6ytb");
    assert_eq!(keys::base32(b"foobar"), "mzxw6ytboi");
}

#[test]
fn keys_are_unguessable_and_well_formed() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1000 {
        let key = keys::object_key("Q3 report.pdf").unwrap();
        let rest = key.strip_prefix("penguin/").unwrap();
        let (token, name) = rest.split_once('/').unwrap();
        assert_eq!(token.len(), keys::TOKEN_LEN, "128 bits in base32");
        assert!(token
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));
        assert_eq!(name, "Q3-report.pdf");
        assert!(keys::is_share_key(&key));
        assert!(seen.insert(token.to_string()), "tokens never repeat");
    }
}

#[test]
fn key_names_are_safe_for_urls_and_shells() {
    assert_eq!(keys::key_name("screenshot.png"), "screenshot.png");
    assert_eq!(keys::key_name("My  Photo (1).JPG"), "My-Photo-_1_.JPG");
    assert_eq!(keys::key_name("../../etc/passwd"), "passwd");
    assert_eq!(keys::key_name("C:\\Users\\sam\\a b.txt"), "a-b.txt");
    assert_eq!(
        keys::key_name("Überweisung März.pdf"),
        "berweisung-M_rz.pdf"
    );
    assert_eq!(keys::key_name(".hidden"), "hidden");
    assert_eq!(keys::key_name("日本語"), "file");
    assert_eq!(keys::key_name(""), "file");
    assert_eq!(keys::key_name("   "), "file");
    assert_eq!(keys::key_name("a?b&c=d#e"), "a_b_c_d_e");
    let long = format!("{}.pdf", "x".repeat(300));
    let cut = keys::key_name(&long);
    assert_eq!(cut.len(), 100);
    assert!(cut.ends_with(".pdf"));
    for name in ["x", "report.pdf", "Q3 report.pdf", "日本語.png", &long] {
        let key = format!("penguin/{}/{}", "a".repeat(26), keys::key_name(name));
        assert!(keys::is_share_key(&key), "{key}");
    }
}

#[test]
fn only_penguins_own_keys_are_share_keys() {
    let token = "abcdefghijklmnopqrstuvwxyz";
    assert!(keys::is_share_key(&format!("penguin/{token}/a.pdf")));
    for bad in [
        "penguin/abc/a.pdf".to_string(),
        format!("other/{token}/a.pdf"),
        format!("penguin/{token}/"),
        format!("penguin/{token}/../x"),
        format!("penguin/{token}/a/b"),
        format!("penguin/{}/a.pdf", "A".repeat(26)),
        format!("penguin/{token}/.env"),
        "penguin/test/abc.txt".to_string(),
        String::new(),
    ] {
        assert!(!keys::is_share_key(&bad), "{bad}");
    }
}

#[test]
fn content_headers_keep_the_name_and_never_render_html() {
    assert_eq!(keys::content_type("image/PNG"), "image/png");
    assert_eq!(keys::content_type("application/pdf"), "application/pdf");
    assert_eq!(
        keys::content_type("text/html; charset=utf-8"),
        "application/octet-stream"
    );
    assert_eq!(keys::content_type(""), "application/octet-stream");
    assert_eq!(keys::content_type("nonsense"), "application/octet-stream");
    assert_eq!(
        keys::content_disposition("Überweisung \"März\".pdf", "application/pdf"),
        "attachment; filename=\"_berweisung _M_rz_.pdf\"; filename*=UTF-8''%C3%9Cberweisung%20%22M%C3%A4rz%22.pdf"
    );
    assert!(keys::content_disposition("a.png", "image/png").starts_with("inline; "));
    assert!(keys::content_disposition("a.html", "text/html").starts_with("attachment; "));
    assert!(keys::content_disposition("a.svg", "image/svg+xml").starts_with("attachment; "));
    // No header injection through a name.
    assert!(!keys::content_disposition("a\r\nX-Evil: 1.txt", "text/plain").contains('\n'));
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[test]
fn storage_settings_are_checked() {
    let ok = config::validate_target(
        "https://abc123.r2.cloudflarestorage.com",
        "penguin-shares",
        "",
        KEY_ID,
    )
    .unwrap();
    assert_eq!(ok.endpoint_str(), "https://abc123.r2.cloudflarestorage.com");
    assert_eq!(ok.region, "auto", "region defaults to auto");
    assert!(config::validate_target(
        "https://abc123.r2.cloudflarestorage.com/",
        "b.c-d",
        "us-east-1",
        KEY_ID
    )
    .is_ok());
    // A local MinIO over plain http, loopback only.
    assert!(config::validate_target("http://127.0.0.1:9000", "bucket", "", KEY_ID).is_ok());
    assert!(config::validate_target("http://localhost:9000", "bucket", "", KEY_ID).is_ok());
    let bad = |e: &str, b: &str, r: &str, k: &str| config::validate_target(e, b, r, k).unwrap_err();
    assert!(bad("", "bucket", "", KEY_ID).contains("endpoint"));
    assert!(bad("http://abc.r2.cloudflarestorage.com", "bucket", "", KEY_ID).contains("https://"));
    assert!(bad("ftp://abc.example", "bucket", "", KEY_ID).contains("https://"));
    assert!(bad("abc.r2.cloudflarestorage.com", "bucket", "", KEY_ID).contains("web address"));
    assert!(bad(
        "https://abc.r2.cloudflarestorage.com/penguin-shares",
        "bucket",
        "",
        KEY_ID
    )
    .contains("Bucket"));
    assert!(bad("https://key:secret@abc.example", "bucket", "", KEY_ID).contains("key"));
    assert!(bad("https://abc.example?x=1", "bucket", "", KEY_ID).contains("nothing after"));
    for b in [
        "",
        "ab",
        "Upper",
        "-dash",
        "dash-",
        "a..b",
        "under_score",
        &"x".repeat(64),
    ] {
        assert!(
            config::validate_target("https://abc.example", b, "", KEY_ID).is_err(),
            "{b}"
        );
    }
    assert!(bad("https://abc.example", "bucket", "US EAST", KEY_ID).contains("region"));
    assert!(bad("https://abc.example", "bucket", "", "").contains("access key ID"));
    assert!(bad("https://abc.example", "bucket", "", "has space").contains("access key ID"));
    assert!(config::validate_secret(SECRET).is_ok());
    assert!(config::validate_secret("with space").is_err());
    assert!(config::validate_secret(&"s".repeat(300)).is_err());
}

#[test]
fn a_config_is_set_up_only_with_every_field_and_a_saved_secret() {
    let mut c = StoredConfig::default();
    assert!(!c.configured());
    assert_eq!(c.lifetime, LinkLifetime::Day);
    assert!(c.delete_on_expiry, "cleanup on by default");
    assert!(!c.allow_agents, "agents off by default");
    c.endpoint = "https://abc.r2.cloudflarestorage.com".into();
    c.bucket = "penguin-shares".into();
    c.access_key_id = KEY_ID.into();
    assert!(!c.configured(), "no secret yet");
    c.secret_saved = true;
    assert!(c.configured());
    let view = c.view();
    assert!(view.configured && view.has_secret);
    // The view is what the UI gets: no secret in it.
    assert!(!serde_json::to_string(&view).unwrap().contains(SECRET));
}

#[test]
fn the_form_parses_and_its_debug_redacts_the_secret() {
    let form: ShareLinkConfigInput = serde_json::from_value(serde_json::json!({
        "endpoint": "https://abc.r2.cloudflarestorage.com",
        "bucket": "penguin-shares",
        "accessKeyId": KEY_ID,
        "secretAccessKey": SECRET,
        "lifetime": "7d",
    }))
    .unwrap();
    assert_eq!(form.lifetime, LinkLifetime::Week);
    assert!(form.delete_on_expiry);
    assert!(!form.allow_agents);
    assert_eq!(form.typed_secret(), Some(SECRET));
    assert!(!format!("{form:?}").contains(SECRET));
    let blank: ShareLinkConfigInput = serde_json::from_value(serde_json::json!({
        "endpoint": "e", "bucket": "b", "accessKeyId": "k", "secretAccessKey": "  ", "lifetime": "1h"
    }))
    .unwrap();
    assert_eq!(blank.typed_secret(), None);
    assert_eq!(blank.lifetime.seconds(), 3600);
    assert!(
        serde_json::from_value::<ShareLinkConfigInput>(serde_json::json!({
            "endpoint": "e", "bucket": "b", "accessKeyId": "k", "lifetime": "30d"
        }))
        .is_err(),
        "no lifetime past S3's 7-day maximum"
    );
    assert!(
        serde_json::from_value::<ShareLinkConfigInput>(serde_json::json!({
            "endpoint": "e", "bucket": "b", "accessKeyId": "k", "secret": "x"
        }))
        .is_err(),
        "unknown fields are refused"
    );
}

#[test]
fn saving_keeps_the_secret_in_the_keychain_only() {
    let (share, keychain, dir) = share_at("save");
    // No secret typed and none saved.
    let mut form = input("https://abc.r2.cloudflarestorage.com");
    form.secret_access_key = None;
    assert!(share
        .set_config(&form)
        .unwrap_err()
        .message
        .contains("secret"));
    // Saved with a secret.
    let view = share
        .set_config(&input("https://abc.r2.cloudflarestorage.com/"))
        .unwrap();
    assert!(view.configured);
    assert_eq!(view.endpoint, "https://abc.r2.cloudflarestorage.com");
    assert_eq!(
        keychain
            .items
            .lock()
            .unwrap()
            .get(config::KEYCHAIN_ACCOUNT)
            .map(String::as_str),
        Some(SECRET)
    );
    let file = std::fs::read_to_string(dir.join("share-links.json")).unwrap();
    assert!(!file.contains(SECRET), "never in the settings file");
    assert!(file.contains("\"secretSaved\": true"));
    // Changing another field keeps the saved secret.
    form.lifetime = LinkLifetime::Hour;
    let view = share.set_config(&form).unwrap();
    assert_eq!(view.lifetime, LinkLifetime::Hour);
    assert!(view.has_secret);
    // It survives a restart.
    let again = Share::new(
        dir.clone(),
        &dir,
        SecretStore::new(Box::new(Fake(keychain.clone()))),
    );
    assert_eq!(again.config().lifetime, LinkLifetime::Hour);
    assert!(again.config().configured());
    // Remove: back to defaults, Keychain item gone.
    let view = share.clear_config().unwrap();
    assert!(!view.configured && !view.has_secret && view.endpoint.is_empty());
    assert!(keychain.items.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_keychain_is_read_once() {
    let keychain = Arc::new(MemoryKeychain::default());
    keychain.set(config::KEYCHAIN_ACCOUNT, SECRET).unwrap();
    let store = SecretStore::new(Box::new(Fake(keychain.clone())));
    assert_eq!(store.get().unwrap().as_deref(), Some(SECRET));
    assert_eq!(store.get().unwrap().as_deref(), Some(SECRET));
    assert_eq!(keychain.reads.load(Ordering::SeqCst), 1);
    store.clear().unwrap();
    assert_eq!(store.get().unwrap(), None);
    assert_eq!(
        keychain.reads.load(Ordering::SeqCst),
        1,
        "a clear is remembered too"
    );
}

#[test]
fn an_unreadable_settings_file_means_not_set_up() {
    let dir = temp_dir("bad-config");
    std::fs::write(dir.join("share-links.json"), b"{nope").unwrap();
    assert_eq!(config::load(&dir), StoredConfig::default());
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

fn target(endpoint: &str, bucket: &str, region: &str) -> config::Target {
    config::validate_target(endpoint, bucket, region, KEY_ID).unwrap()
}

#[test]
fn presigned_get_matches_the_aws_example() {
    // "Example: GET Object" from the AWS SigV4 query-string auth docs.
    let t = config::validate_target(
        "https://s3.amazonaws.com",
        "examplebucket",
        "us-east-1",
        "AKIAIOSFODNN7EXAMPLE",
    )
    .unwrap();
    let signer = Signer::new(&t, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY").unwrap();
    let url = signer.get_url("test.txt", Duration::from_secs(86400), at(1_369_353_600));
    assert_eq!(url.host_str(), Some("examplebucket.s3.amazonaws.com"));
    assert_eq!(url.path(), "/test.txt");
    let q = query(&url);
    assert_eq!(
        q["X-Amz-Signature"],
        "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
    );
}

#[test]
fn presigned_links_have_the_shape_and_lifetime_asked_for() {
    let t = target(
        "https://abc123.r2.cloudflarestorage.com",
        "penguin-shares",
        "",
    );
    let signer = Signer::new(&t, SECRET).unwrap();
    let key = "penguin/abcdefghijklmnopqrstuvwxyz/Q3-report.pdf";
    for (lifetime, secs) in [
        (LinkLifetime::Hour, "3600"),
        (LinkLifetime::Day, "86400"),
        (LinkLifetime::Week, "604800"),
    ] {
        let url = signer.get_url(key, Duration::from_secs(lifetime.seconds()), at(NOON));
        assert_eq!(url.scheme(), "https");
        // Path style on R2: the bucket is the first path segment.
        assert_eq!(url.host_str(), Some("abc123.r2.cloudflarestorage.com"));
        assert_eq!(url.path(), format!("/penguin-shares/{key}"));
        let q = query(&url);
        assert_eq!(q["X-Amz-Algorithm"], "AWS4-HMAC-SHA256");
        assert_eq!(
            q["X-Amz-Credential"],
            format!("{KEY_ID}/20260929/auto/s3/aws4_request")
        );
        assert_eq!(q["X-Amz-Date"], "20260929T120000Z");
        assert_eq!(q["X-Amz-Expires"], secs);
        assert_eq!(q["X-Amz-SignedHeaders"], "host");
        let sig = &q["X-Amz-Signature"];
        assert!(sig.len() == 64 && sig.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(
            !url.as_str().contains(SECRET),
            "the secret never appears in a link"
        );
    }
    assert_eq!(LinkLifetime::Week.seconds(), 604_800, "S3's maximum");
    // Different times sign differently.
    let a = signer.get_url(key, Duration::from_secs(3600), at(NOON));
    let b = signer.get_url(key, Duration::from_secs(3600), at(NOON + 1));
    assert_ne!(query(&a)["X-Amz-Signature"], query(&b)["X-Amz-Signature"]);
}

#[test]
fn uploads_sign_their_type_and_name_headers() {
    let t = target(
        "https://abc123.r2.cloudflarestorage.com",
        "penguin-shares",
        "",
    );
    let signer = Signer::new(&t, SECRET).unwrap();
    let url = signer.put_url(
        "penguin/x/a.pdf",
        "application/pdf",
        "attachment; filename=\"a.pdf\"",
        at(NOON),
    );
    assert_eq!(
        query(&url)["X-Amz-SignedHeaders"],
        "content-disposition;content-type;host"
    );
    let del = signer.delete_url("penguin/x/a.pdf", at(NOON));
    assert_eq!(
        query(&del)["X-Amz-Expires"],
        "900",
        "request signatures are short-lived"
    );
    // AWS itself: virtual-hosted, unless the bucket has dots.
    let aws = Signer::new(
        &target(
            "https://s3.us-west-2.amazonaws.com",
            "my.bucket",
            "us-west-2",
        ),
        SECRET,
    )
    .unwrap();
    assert_eq!(
        aws.get_url("k", Duration::from_secs(60), at(NOON))
            .host_str(),
        Some("s3.us-west-2.amazonaws.com")
    );
}

// ---------------------------------------------------------------------------
// Errors in words
// ---------------------------------------------------------------------------

#[test]
fn storage_errors_say_what_to_fix() {
    let now = at(NOON);
    let date = "Tue, 29 Sep 2026 12:01:00 GMT";
    assert_eq!(
        s3::classify(403, &xml("InvalidAccessKeyId"), Some(date), now),
        S3Error::BadKeyId
    );
    assert_eq!(
        s3::classify(403, &xml("SignatureDoesNotMatch"), Some(date), now),
        S3Error::BadSecret
    );
    assert_eq!(
        s3::classify(404, &xml("NoSuchBucket"), Some(date), now),
        S3Error::NoBucket
    );
    assert_eq!(
        s3::classify(403, &xml("AccessDenied"), Some(date), now),
        S3Error::Denied
    );
    assert_eq!(s3::classify(403, "", None, now), S3Error::Denied);
    assert_eq!(
        s3::classify(301, &xml("PermanentRedirect"), None, now),
        S3Error::Moved
    );
    assert_eq!(
        s3::classify(400, "<Error><Code>AuthorizationQueryParametersError</Code><Region>eu-west-1</Region></Error>", Some(date), now),
        S3Error::WrongRegion(Some("eu-west-1".into()))
    );
    assert_eq!(
        s3::classify(500, &xml("InternalError"), Some(date), now),
        S3Error::Status {
            status: 500,
            code: Some("InternalError".into())
        }
    );
    // Clock skew: named by S3, or inferred from Date on an auth failure.
    assert_eq!(
        s3::classify(
            403,
            &xml("RequestTimeTooSkewed"),
            Some("Tue, 29 Sep 2026 12:20:00 GMT"),
            now
        ),
        S3Error::ClockSkew { minutes: -20 }
    );
    assert_eq!(
        s3::classify(
            403,
            &xml("SignatureDoesNotMatch"),
            Some("Tue, 29 Sep 2026 11:00:00 GMT"),
            now
        ),
        S3Error::ClockSkew { minutes: 60 }
    );
    // A far-off Date on a success-like status isn't blamed.
    assert_eq!(
        s3::classify(
            404,
            &xml("NoSuchBucket"),
            Some("Tue, 29 Sep 2026 11:00:00 GMT"),
            now
        ),
        S3Error::NoBucket
    );
    // Junk in the document is ignored.
    assert_eq!(s3::error_fields("<Code><script></Code>"), (None, None));
    // Messages name the fix, and never the key or secret.
    let msg = |e: S3Error| e.message("penguin-shares");
    assert!(msg(S3Error::BadKeyId).contains("access key ID"));
    assert!(msg(S3Error::BadSecret).contains("secret access key"));
    assert!(msg(S3Error::NoBucket).contains("no bucket named penguin-shares"));
    assert!(msg(S3Error::Denied).contains("Object Read & Write"));
    assert!(msg(S3Error::ClockSkew { minutes: 20 }).contains("20 minutes off"));
    assert!(msg(S3Error::Unreachable {
        host: "abc.example".into(),
        timeout: false
    })
    .contains("Couldn't reach abc.example"));
}

// ---------------------------------------------------------------------------
// Cleanup scheduler
// ---------------------------------------------------------------------------

const E: &str = "https://abc.r2.cloudflarestorage.com";
const B: &str = "penguin-shares";

fn upload(key: &str, expires_at: i64) -> Upload {
    Upload {
        key: key.into(),
        endpoint: E.into(),
        bucket: B.into(),
        created_at: expires_at - 3_600_000,
        expires_at,
        attempts: 0,
        next_try_at: 0,
    }
}

#[test]
fn backoff_grows_and_caps_at_a_day() {
    assert_eq!(uploads::backoff_ms(1), 5 * 60_000);
    assert_eq!(uploads::backoff_ms(2), 10 * 60_000);
    assert_eq!(uploads::backoff_ms(3), 20 * 60_000);
    assert_eq!(uploads::backoff_ms(20), 24 * 3_600_000);
    assert_eq!(uploads::backoff_ms(u32::MAX), 24 * 3_600_000);
}

#[test]
fn only_expired_uploads_of_this_storage_are_due() {
    let now = 10_000_000;
    let mut waiting = upload("penguin/a/waiting", now - 1);
    waiting.next_try_at = now + 1;
    let mut elsewhere = upload("penguin/a/elsewhere", now - 1);
    elsewhere.bucket = "other".into();
    let list = vec![
        upload("penguin/a/due", now),
        upload("penguin/a/live", now + 1),
        waiting,
        elsewhere,
    ];
    let due: Vec<_> = uploads::due(&list, now, E, B)
        .into_iter()
        .map(|u| u.key)
        .collect();
    assert_eq!(due, ["penguin/a/due"]);
}

#[tokio::test]
async fn a_sweep_deletes_retries_and_forgets() {
    let now = 100 * 24 * 3_600_000;
    let stale = upload("penguin/a/stale", now - uploads::FORGET_AFTER_MS - 1);
    let mut stale_elsewhere = stale.clone();
    stale_elsewhere.key = "penguin/a/stale-elsewhere".into();
    stale_elsewhere.bucket = "old-bucket".into();
    let list = Uploads::in_memory(vec![
        upload("penguin/a/ok", now - 1000),
        upload("penguin/a/fails", now - 1000),
        upload("penguin/a/live", now + 24 * 3_600_000),
        stale_elsewhere,
    ]);
    let tried = Mutex::new(Vec::new());
    let swept = uploads::sweep(&list, now, Some((E, B)), |u: Upload| {
        tried.lock().unwrap().push(u.key.clone());
        let ok = u.key != "penguin/a/fails";
        async move {
            if ok {
                Ok(())
            } else {
                Err(S3Error::Unreachable {
                    host: "h".into(),
                    timeout: true,
                })
            }
        }
    })
    .await;
    assert_eq!(
        swept,
        uploads::Sweep {
            deleted: 1,
            failed: 1,
            forgotten: 1
        }
    );
    assert_eq!(*tried.lock().unwrap(), ["penguin/a/ok", "penguin/a/fails"]);
    let left: Vec<_> = list.snapshot().into_iter().map(|u| u.key).collect();
    assert_eq!(left, ["penguin/a/fails", "penguin/a/live"]);
    let failed = list.get("penguin/a/fails").unwrap();
    assert_eq!(failed.attempts, 1);
    assert_eq!(failed.next_try_at, now + uploads::backoff_ms(1));
    // Not retried before its time…
    tried.lock().unwrap().clear();
    let never = |u: Upload| {
        tried.lock().unwrap().push(u.key);
        async { Ok::<(), S3Error>(()) }
    };
    uploads::sweep(&list, now + 1000, Some((E, B)), never).await;
    assert!(tried.lock().unwrap().is_empty());
    // …then retried and removed.
    uploads::sweep(&list, now + uploads::backoff_ms(1), Some((E, B)), never).await;
    assert_eq!(*tried.lock().unwrap(), ["penguin/a/fails"]);
    // With deleting off, nothing is deleted, but old records are still forgotten.
    let off = Uploads::in_memory(vec![upload("penguin/a/x", now - 1000), stale]);
    let swept = uploads::sweep(&off, now, None, |_: Upload| async {
        Err::<(), _>("never called")
    })
    .await;
    assert_eq!(
        swept,
        uploads::Sweep {
            deleted: 0,
            failed: 0,
            forgotten: 1
        }
    );
}

#[test]
fn upload_records_survive_a_restart() {
    let dir = temp_dir("records");
    let list = Uploads::load(&dir);
    list.add(upload("penguin/a/one", 5)).unwrap();
    list.add(upload("penguin/a/two", 6)).unwrap();
    list.failed("penguin/a/two", 100).unwrap();
    list.remove("penguin/a/one").unwrap();
    let again = Uploads::load(&dir);
    let snap = again.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].key, "penguin/a/two");
    assert_eq!(snap[0].attempts, 1);
    // A damaged file is set aside, not trusted or overwritten.
    std::fs::write(dir.join("share-uploads.json"), b"[[[").unwrap();
    assert!(Uploads::load(&dir).snapshot().is_empty());
    assert!(dir.join("share-uploads.json.bad").exists());
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// The agent gate
// ---------------------------------------------------------------------------

/// Agents need both: storage set up and the switch on. Each refusal is a
/// permission error (penguin-cli exit 77) naming the page to change.
#[test]
fn agents_are_refused_unless_allowed() {
    let mut c = StoredConfig::default();
    assert!(check_caller(Caller::User, &c).is_ok());
    // Nothing set up (the switch alone isn't enough).
    for allow in [false, true] {
        c.allow_agents = allow;
        let refused = check_caller(Caller::Agent, &c).unwrap_err();
        assert_eq!(refused.code, ErrorCode::PermissionDenied);
        assert!(
            refused.message.contains("aren't set up"),
            "{}",
            refused.message
        );
        assert!(refused.message.contains("Settings → Share links"));
        assert!(!super::agents_allowed(&c));
    }
    // Set up, switch off.
    c = StoredConfig {
        endpoint: "https://abc.r2.cloudflarestorage.com".into(),
        bucket: "penguin-shares".into(),
        access_key_id: KEY_ID.into(),
        secret_saved: true,
        ..StoredConfig::default()
    };
    assert!(c.configured());
    let refused = check_caller(Caller::Agent, &c).unwrap_err();
    assert_eq!(refused.code, ErrorCode::PermissionDenied);
    assert!(refused
        .message
        .contains("Let agents (CLI and MCP) create share links"));
    assert!(refused.message.contains("Settings → Share links"));
    assert!(!super::agents_allowed(&c));
    // Both.
    c.allow_agents = true;
    assert!(check_caller(Caller::Agent, &c).is_ok());
    assert!(super::agents_allowed(&c));
}

#[tokio::test]
async fn prepare_checks_setup_then_the_caller() {
    let (share, _, dir) = share_at("gate");
    let err = share.prepare_blocking(Caller::User).await.err().unwrap();
    assert_eq!(err.code, ErrorCode::NotConfigured);
    // An agent is told it isn't allowed (and what to turn on), not "set up".
    let err = share.prepare_blocking(Caller::Agent).await.err().unwrap();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    share
        .set_config(&input("https://abc.r2.cloudflarestorage.com"))
        .unwrap();
    assert!(share.prepare_blocking(Caller::User).await.is_ok());
    let err = share.prepare_blocking(Caller::Agent).await.err().unwrap();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    let mut allow = input("https://abc.r2.cloudflarestorage.com");
    allow.allow_agents = true;
    share.set_config(&allow).unwrap();
    assert!(share.prepare_blocking(Caller::Agent).await.is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// End to end against an in-process S3 stub (share/stub.rs)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn share_uploads_links_and_deletes_end_to_end() {
    let stub = Stub::start().await;
    let (share, _, dir) = share_at("e2e");
    share.set_config(&input(&stub.endpoint)).unwrap();
    let prepared = share.prepare_blocking(Caller::User).await.unwrap();
    let bytes = b"%PDF-1.7 fictional quarterly report".to_vec();
    let before = SystemTime::now();
    let link = share
        .upload(
            &prepared,
            "Q3 report.pdf",
            "application/pdf",
            bytes.clone(),
            before,
        )
        .await
        .unwrap();

    // One PUT, signed, with the type and name headers it signed.
    let put = stub.seen().into_iter().find(|s| s.method == "PUT").unwrap();
    assert_eq!(put.path, format!("/penguin-shares/{}", link.key));
    assert_eq!(put.body, bytes);
    assert_eq!(put.headers["content-type"], "application/pdf");
    assert_eq!(
        put.headers["content-disposition"],
        "attachment; filename=\"Q3 report.pdf\"; filename*=UTF-8''Q3%20report.pdf"
    );
    assert_eq!(
        put.query["X-Amz-SignedHeaders"],
        "content-disposition;content-type;host"
    );
    assert!(keys::is_share_key(&link.key));
    assert!(link.key.ends_with("/Q3-report.pdf"));
    assert_eq!(link.name, "Q3 report.pdf");
    assert_eq!(link.size, bytes.len() as u64);
    let created = super::unix_ms(before);
    assert_eq!(link.expires_at, created + 24 * 3_600_000);
    assert!(
        !format!("{link:?}").contains("X-Amz-Signature"),
        "Debug never shows the link"
    );

    // The link downloads it.
    let url = url::Url::parse(&link.url).unwrap();
    assert_eq!(query(&url)["X-Amz-Expires"], "86400");
    let got = reqwest::get(url).await.unwrap();
    assert_eq!(got.status(), 200);
    assert_eq!(got.bytes().await.unwrap().as_ref(), bytes.as_slice());

    // Recorded for cleanup.
    let rec = share.uploads.get(&link.key).unwrap();
    assert_eq!(rec.endpoint, stub.endpoint);
    assert_eq!(rec.bucket, "penguin-shares");
    assert_eq!(rec.expires_at, link.expires_at);

    // Delete now: a signed DELETE, and the record goes.
    share.delete(&link.key).await.unwrap();
    let del = stub
        .seen()
        .into_iter()
        .find(|s| s.method == "DELETE")
        .unwrap();
    assert_eq!(del.path, put.path);
    assert!(del.query.contains_key("X-Amz-Signature"));
    assert!(share.uploads.get(&link.key).is_none());
    assert!(stub.objects.lock().unwrap().is_empty());
    // Only Penguin's own recorded keys can be deleted.
    assert_eq!(
        share.delete(&link.key).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        share
            .delete("penguin-shares/anything")
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_refused_upload_says_why_and_leaves_no_record() {
    let stub = Stub::start().await;
    let (share, _, dir) = share_at("refused");
    share.set_config(&input(&stub.endpoint)).unwrap();
    let prepared = share.prepare_blocking(Caller::User).await.unwrap();
    *stub.refuse.lock().unwrap() = Some((403, "InvalidAccessKeyId"));
    let err = share
        .upload(
            &prepared,
            "a.png",
            "image/png",
            vec![1, 2, 3],
            SystemTime::now(),
        )
        .await
        .unwrap_err();
    assert!(
        err.message.contains("doesn't know this access key ID"),
        "{}",
        err.message
    );
    assert!(share.uploads.snapshot().is_empty());
    // Too big: refused before anything is sent.
    let before = stub.seen().len();
    let big = vec![0u8; (super::MAX_SHARE_BYTES + 1) as usize];
    let err = share
        .upload(&prepared, "big.bin", "", big, SystemTime::now())
        .await
        .unwrap_err();
    assert!(err.message.contains("up to 100 MB"));
    assert_eq!(stub.seen().len(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_settings_test_reports_each_step() {
    let stub = Stub::start().await;
    let (share, _, dir) = share_at("test");
    let t = config::validate_target(&stub.endpoint, "penguin-shares", "", KEY_ID).unwrap();
    let report = share.test(&t, SECRET).await;
    let steps: Vec<_> = report.steps.iter().map(|s| (s.step, s.status)).collect();
    assert_eq!(
        steps,
        [
            ("upload", "ok"),
            ("link", "ok"),
            ("private", "ok"),
            ("delete", "ok")
        ],
        "{report:?}"
    );
    assert!(report.ok);
    assert!(
        stub.objects.lock().unwrap().is_empty(),
        "the test file is gone"
    );
    let put = stub.seen().into_iter().find(|s| s.method == "PUT").unwrap();
    assert!(put.path.starts_with("/penguin-shares/penguin/test/"));

    *stub.refuse.lock().unwrap() = Some((404, "NoSuchBucket"));
    let report = share.test(&t, SECRET).await;
    assert!(!report.ok);
    assert_eq!(report.steps[0].status, "failed");
    assert!(report.steps[0]
        .message
        .contains("no bucket named penguin-shares"));
    assert!(report.steps[1..].iter().all(|s| s.status == "skipped"));

    // Nothing listening: the endpoint is wrong.
    let dead = config::validate_target("http://127.0.0.1:9", "penguin-shares", "", KEY_ID).unwrap();
    let report = share.test(&dead, SECRET).await;
    assert!(
        report.steps[0].message.contains("Couldn't reach 127.0.0.1"),
        "{report:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn cleanup_deletes_expired_uploads_from_the_storage() {
    let stub = Stub::start().await;
    let (share, _, dir) = share_at("sweep");
    share.set_config(&input(&stub.endpoint)).unwrap();
    let prepared = share.prepare_blocking(Caller::User).await.unwrap();
    let link = share
        .upload(
            &prepared,
            "a.txt",
            "text/plain",
            b"hello".to_vec(),
            SystemTime::now(),
        )
        .await
        .unwrap();
    // Not yet expired: untouched.
    assert_eq!(
        share.sweep_once(SystemTime::now()).await,
        uploads::Sweep::default()
    );
    assert_eq!(stub.objects.lock().unwrap().len(), 1);
    // A day later: deleted, record gone.
    let later = SystemTime::now() + Duration::from_secs(24 * 3600 + 60);
    let swept = share.sweep_once(later).await;
    assert_eq!(swept.deleted, 1);
    assert!(stub.objects.lock().unwrap().is_empty());
    assert!(share.uploads.get(&link.key).is_none());
    // With "Delete uploads when their links expire" off, expired files stay.
    let mut keep = input(&stub.endpoint);
    keep.delete_on_expiry = false;
    keep.secret_access_key = None;
    share.set_config(&keep).unwrap();
    let prepared = share.prepare_blocking(Caller::User).await.unwrap();
    share
        .upload(
            &prepared,
            "b.txt",
            "text/plain",
            b"hi".to_vec(),
            SystemTime::now(),
        )
        .await
        .unwrap();
    assert_eq!(share.sweep_once(later).await.deleted, 0);
    assert_eq!(stub.objects.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}
