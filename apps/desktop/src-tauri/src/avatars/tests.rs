//! Resolver behaviour against a fake network: order, privacy gates,
//! negative caching, single-flight.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::cache::{Cache, Lookup, SourceKind};
use super::net::{BoxFut, FetchError, Net};
use super::*;

#[derive(Default)]
struct FakeNet {
    txt: HashMap<String, Vec<String>>,
    get: HashMap<String, Result<Vec<u8>, FetchError>>,
    calls: Mutex<Vec<String>>,
}

impl FakeNet {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Net for FakeNet {
    fn txt<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<Vec<String>, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("dns {name}"));
            Ok(self.txt.get(name).cloned().unwrap_or_default())
        })
    }
    fn get<'a>(
        &'a self,
        url: &'a str,
        _max: usize,
        _bearer: Option<&'a str>,
    ) -> BoxFut<'a, Result<Vec<u8>, FetchError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("get {url}"));
            self.get
                .get(url)
                .cloned()
                .unwrap_or(Err(FetchError::NotFound))
        })
    }
}

fn png(edge: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = ::image::RgbaImage::from_pixel(edge, edge, ::image::Rgba(rgba));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ::image::ImageFormat::Png)
        .unwrap();
    out
}

const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#e11d48"/></svg>"##;

fn temp(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("penguin-avatars-res-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn all_on() -> Prefs {
    Prefs {
        show: true,
        contacts: true,
        bimi: true,
        favicons: true,
        gravatar: true,
    }
}

fn defaults() -> Prefs {
    Prefs {
        show: true,
        contacts: false,
        bimi: true,
        favicons: true,
        gravatar: false,
    }
}

fn shop_net() -> FakeNet {
    let mut net = FakeNet::default();
    net.txt.insert(
        "default._bimi.shop.example".into(),
        vec![
            "v=spf1 -all".into(),
            "v=BIMI1; l=https://cdn.shop.example/bimi.svg;".into(),
        ],
    );
    net.get.insert(
        "https://cdn.shop.example/bimi.svg".into(),
        Ok(LOGO.as_bytes().to_vec()),
    );
    net.get.insert(
        "https://shop.example/apple-touch-icon.png".into(),
        Ok(png(180, [0, 0, 255, 255])),
    );
    net
}

fn resolver(tag: &str, net: Arc<FakeNet>) -> Resolver {
    Resolver::new(Cache::new(temp(tag)), net)
}

#[tokio::test]
async fn bimi_logo_for_an_authenticated_sender() {
    let net = Arc::new(shop_net());
    let r = resolver("bimi", net.clone());
    let (info, needs) = r.lookup("news@shop.example", &defaults(), true, 1000);
    assert!(info.url.is_none() && needs);
    assert!(
        r.resolve("news@shop.example", &defaults(), true, 1000)
            .await
    );
    let (info, needs) = r.lookup("news@shop.example", &defaults(), true, 1001);
    assert_eq!(info.kind, Some(AvatarKind::Logo));
    assert!(info
        .url
        .as_deref()
        .unwrap()
        .starts_with("avatar://localhost/"));
    assert!(!needs);
    // BIMI won, so the favicon was never requested.
    assert!(!net.calls().iter().any(|c| c.contains("apple-touch-icon")));
    let _ = r.cache.clear();
}

#[tokio::test]
async fn no_brand_requests_when_the_sender_is_not_authenticated() {
    let net = Arc::new(shop_net());
    let r = resolver("unauth", net.clone());
    let changed = r.resolve("news@shop.example", &all_on(), false, 1000).await;
    assert!(!changed);
    let calls = net.calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("dns") || c.contains("shop.example/")),
        "{calls:?}"
    );
    // Only Gravatar (enabled here) was asked, by hash.
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("get https://gravatar.com/avatar/"));
    assert!(!calls[0].contains("news"));
    let (info, _) = r.lookup("news@shop.example", &all_on(), false, 1001);
    assert!(info.url.is_none());
    let _ = r.cache.clear();
}

#[tokio::test]
async fn cached_logo_is_withheld_from_an_unauthenticated_message() {
    let net = Arc::new(shop_net());
    let r = resolver("withheld", net.clone());
    r.resolve("news@shop.example", &defaults(), true, 1000)
        .await;
    assert!(r
        .lookup("news@shop.example", &defaults(), true, 1001)
        .0
        .url
        .is_some());
    // Same sender, but this message failed DMARC (a spoof): monogram.
    assert!(r
        .lookup("news@shop.example", &defaults(), false, 1001)
        .0
        .url
        .is_none());
    let _ = r.cache.clear();
}

#[tokio::test]
async fn gravatar_is_never_asked_when_off() {
    let net = Arc::new(FakeNet::default());
    let r = resolver("gravoff", net.clone());
    let prefs = Prefs {
        gravatar: false,
        ..all_on()
    };
    r.resolve("person@gmail.com", &prefs, true, 1000).await;
    r.resolve("bo@shop.example", &prefs, false, 1000).await;
    assert!(
        !net.calls().iter().any(|c| c.contains("gravatar")),
        "{:?}",
        net.calls()
    );
    let _ = r.cache.clear();
}

#[tokio::test]
async fn mailbox_providers_get_no_brand_and_show_off_does_nothing() {
    let net = Arc::new(FakeNet::default());
    let r = resolver("provider", net.clone());
    r.resolve("person@gmail.com", &defaults(), true, 1000).await;
    assert!(net.calls().is_empty(), "{:?}", net.calls());
    let hidden = Prefs {
        show: false,
        ..all_on()
    };
    r.resolve("bo@shop.example", &hidden, true, 1000).await;
    assert!(net.calls().is_empty());
    assert_eq!(
        r.lookup("bo@shop.example", &hidden, true, 0),
        (
            AvatarInfo {
                email: "bo@shop.example".into(),
                kind: None,
                url: None
            },
            false
        )
    );
    let _ = r.cache.clear();
}

#[tokio::test]
async fn misses_are_cached_until_their_ttl() {
    let net = Arc::new(FakeNet::default());
    let r = resolver("negative", net.clone());
    r.resolve("x@nologo.example", &defaults(), true, 1000).await;
    let first = net.calls().len();
    // BIMI (domain, then nothing: org == domain), then 4 icon URLs.
    assert_eq!(first, 5, "{:?}", net.calls());
    // Within the TTL: nothing to do, no requests.
    let (_, needs) = r.lookup("x@nologo.example", &defaults(), true, 2000);
    assert!(!needs);
    r.resolve("x@nologo.example", &defaults(), true, 2000).await;
    assert_eq!(net.calls().len(), first);
    // A second sender at the same domain shares the domain-level misses.
    r.resolve("y@nologo.example", &defaults(), true, 2000).await;
    assert_eq!(net.calls().len(), first);
    // After the miss TTL it's looked up again.
    let later = 1000 + SourceKind::Icon.miss_ttl() + 1;
    assert!(r.lookup("x@nologo.example", &defaults(), true, later).1);
    r.resolve("x@nologo.example", &defaults(), true, later)
        .await;
    assert!(net.calls().len() > first);
    let _ = r.cache.clear();
}

#[tokio::test]
async fn transient_errors_are_retried_soon_and_keep_stale_images() {
    let mut net = FakeNet::default();
    net.get.insert(
        "https://shop.example/apple-touch-icon.png".into(),
        Ok(png(180, [0, 0, 255, 255])),
    );
    let net = Arc::new(net);
    let r = resolver("transient", net.clone());
    let prefs = Prefs {
        bimi: false,
        ..defaults()
    };
    r.resolve("bo@shop.example", &prefs, true, 1000).await;
    let url = r.lookup("bo@shop.example", &prefs, true, 1001).0.url;
    assert!(url.is_some());

    // Offline when it goes stale: the old icon stays.
    let offline = Arc::new(FakeNet {
        get: HashMap::from([
            (
                "https://shop.example/apple-touch-icon.png".to_string(),
                Err(FetchError::Transient("offline".into())),
            ),
            (
                "https://www.shop.example/apple-touch-icon.png".to_string(),
                Err(FetchError::Transient("offline".into())),
            ),
        ]),
        ..Default::default()
    });
    let r2 = Resolver::new(Cache::new(r.cache.root().to_path_buf()), offline);
    r.cache.flush().unwrap();
    let stale_at = 1000 + SourceKind::Icon.hit_ttl() + 1;
    r2.resolve("bo@shop.example", &prefs, true, stale_at).await;
    assert_eq!(
        r2.lookup("bo@shop.example", &prefs, true, stale_at).0.url,
        url
    );
    let _ = r.cache.clear();
}

#[tokio::test]
async fn contact_photo_wins_and_a_new_url_refetches() {
    let mut net = shop_net();
    net.get.insert(
        "https://lh3.googleusercontent.com/a/ada=s128-c".into(),
        Ok(png(100, [0, 200, 0, 255])),
    );
    net.get.insert(
        "https://lh3.googleusercontent.com/a/ada2=s128-c".into(),
        Ok(png(100, [200, 200, 0, 255])),
    );
    let net = Arc::new(net);
    let r = resolver("contact", net.clone());
    let prefs = Prefs {
        contacts: true,
        ..defaults()
    };
    let hash = sources::email_hash("ada@shop.example");
    r.set_contacts(HashMap::from([(
        hash.clone(),
        "https://lh3.googleusercontent.com/a/ada=s100".to_string(),
    )]));
    r.resolve("Ada@Shop.example", &prefs, true, 1000).await;
    let (info, _) = r.lookup("ada@shop.example", &prefs, true, 1001);
    assert_eq!(info.kind, Some(AvatarKind::Photo));
    assert!(!net.calls().iter().any(|c| c.starts_with("dns")));

    r.set_contacts(HashMap::from([(
        hash,
        "https://lh3.googleusercontent.com/a/ada2=s100".to_string(),
    )]));
    assert!(
        r.lookup("ada@shop.example", &prefs, true, 1002).1,
        "new URL → refetch"
    );
    assert!(r.resolve("ada@shop.example", &prefs, true, 1002).await);
    assert_ne!(
        r.lookup("ada@shop.example", &prefs, true, 1003).0.url,
        info.url
    );
    // Contacts off: the brand logo shows instead.
    assert_eq!(
        r.lookup("ada@shop.example", &defaults(), true, 1003).0.kind,
        None,
        "BIMI not fetched yet"
    );
    let _ = r.cache.clear();
}

#[tokio::test]
async fn concurrent_resolves_for_one_domain_fetch_once() {
    let net = Arc::new(shop_net());
    let r = Arc::new(resolver("single", net.clone()));
    let mut tasks = Vec::new();
    for who in ["a", "b", "c", "d"] {
        let r = r.clone();
        tasks.push(tokio::spawn(async move {
            r.resolve(&format!("{who}@shop.example"), &defaults(), true, 1000)
                .await
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let dns = net.calls().iter().filter(|c| c.starts_with("dns")).count();
    assert_eq!(dns, 1, "{:?}", net.calls());
    let _ = r.cache.clear();
}

#[test]
fn lookup_queues_unknown_senders_once() {
    let net = Arc::new(FakeNet::default());
    let avatars = Avatars::new(resolver("queue", net));
    let reqs = vec![
        AvatarRequest {
            email: "Bo@Shop.example".into(),
            authenticated: None,
        },
        AvatarRequest {
            email: "bo@shop.example".into(),
            authenticated: None,
        },
    ];
    let infos = avatars.lookup(&reqs, &defaults(), 1000);
    assert_eq!(infos[0].email, "Bo@Shop.example");
    let jobs = avatars.take(10);
    assert_eq!(jobs.len(), 1);
    // Still in flight: not queued again.
    avatars.lookup(&reqs, &defaults(), 1000);
    assert!(avatars.take(10).is_empty());
    avatars.done(&jobs[0]);
    avatars.set_evidence(HashMap::new(), &["bo@shop.example".into()], 1000);
    assert!(!avatars.authenticated_for(&jobs[0], 1000));
    let _ = avatars.clear();
}

#[test]
fn cache_state_is_consistent_after_clear() {
    let net = Arc::new(FakeNet::default());
    let r = resolver("clear", net);
    r.cache.put_miss("icon-x", 0, 100, None);
    r.cache.clear().unwrap();
    assert_eq!(r.cache.lookup("icon-x", 1), Lookup::Missing);
}

// ---------- account photos (account.rs) ----------

const ME_PHOTO: &str = "https://lh3.googleusercontent.com/a/me";

fn google_net(people_me: &str, photo: Result<Vec<u8>, FetchError>) -> FakeNet {
    let mut net = FakeNet::default();
    net.get.insert(
        crate::me::PEOPLE_ME.into(),
        Ok(people_me.as_bytes().to_vec()),
    );
    net.get.insert(format!("{ME_PHOTO}=s128-c"), photo);
    net
}

#[tokio::test]
async fn account_photo_is_fetched_at_avatar_size_and_cached_for_a_day() {
    let net = google_net(
        &format!(r#"{{"photos":[{{"url":"{ME_PHOTO}=s100","metadata":{{"primary":true}}}}]}}"#),
        Ok(png(200, [200, 120, 40, 255])),
    );
    let cache = Cache::new(temp("acct-hit"));
    let email = "Sam@Northwind.example";
    assert_eq!(account::cached(&cache, email, 1000), Lookup::Missing);
    let fetched = account::fetch_photo(&net, "token").await;
    assert!(net.calls().contains(&format!("get {ME_PHOTO}=s128-c")));
    let hash = account::settle(&cache, email, fetched, None, 1000)
        .unwrap()
        .unwrap();
    let img = ::image::load_from_memory(&std::fs::read(cache.image_path(&hash).unwrap()).unwrap())
        .unwrap();
    assert_eq!((img.width(), img.height()), (image::SIZE, image::SIZE));
    // Same account in any case: fresh for a day, then stale (still shown).
    assert_eq!(
        account::cached(&cache, "sam@northwind.example", 1000 + 3600),
        Lookup::Fresh(Some(hash.clone()))
    );
    let later = 1000 + SourceKind::Account.hit_ttl() + 1;
    assert_eq!(
        account::cached(&cache, email, later),
        Lookup::Stale(Some(hash))
    );
    let _ = cache.clear();
}

#[tokio::test]
async fn account_with_only_a_letter_avatar_is_a_cached_miss() {
    let net = google_net(
        &format!(
            r#"{{"photos":[{{"url":"{ME_PHOTO}=s100","default":true,"metadata":{{"primary":true}}}}]}}"#
        ),
        Err(FetchError::NotFound),
    );
    let cache = Cache::new(temp("acct-miss"));
    let fetched = account::fetch_photo(&net, "token").await.unwrap();
    assert_eq!(fetched, None);
    assert_eq!(
        account::settle(&cache, "a@b.example", Ok(fetched), None, 1000).unwrap(),
        None
    );
    assert_eq!(
        account::cached(&cache, "a@b.example", 1001),
        Lookup::Fresh(None)
    );
    let _ = cache.clear();
}

#[tokio::test]
async fn account_photo_refresh_failure_keeps_the_old_photo() {
    let cache = Cache::new(temp("acct-offline"));
    let first = account::settle(
        &cache,
        "a@b.example",
        Ok(Some(png(64, [1, 2, 3, 255]))),
        None,
        1000,
    )
    .unwrap();
    let net = google_net(
        &format!(r#"{{"photos":[{{"url":"{ME_PHOTO}=s100"}}]}}"#),
        Err(FetchError::Transient("offline".into())),
    );
    let later = 1000 + SourceKind::Account.hit_ttl() + 1;
    let Lookup::Stale(stale) = account::cached(&cache, "a@b.example", later) else {
        panic!("expected stale")
    };
    let failed = account::fetch_photo(&net, "token").await;
    assert!(failed.is_err());
    assert_eq!(
        account::settle(&cache, "a@b.example", failed, stale, later).unwrap(),
        first
    );
    // Left stale, so the next session asks Google again.
    assert!(matches!(
        account::cached(&cache, "a@b.example", later),
        Lookup::Stale(Some(_))
    ));
    // No photo yet and offline: an error, and the UI keeps its monogram.
    let failed = account::fetch_photo(&net, "token").await;
    assert!(account::settle(&cache, "c@d.example", failed, None, later).is_err());
    let _ = cache.clear();
}

#[test]
fn account_photo_whose_file_was_wiped_counts_as_missing() {
    let cache = Cache::new(temp("acct-wiped"));
    let hash = account::settle(
        &cache,
        "a@b.example",
        Ok(Some(png(64, [9, 9, 9, 255]))),
        None,
        1000,
    )
    .unwrap()
    .unwrap();
    std::fs::remove_file(cache.image_path(&hash).unwrap()).unwrap();
    assert_eq!(
        account::cached(&cache, "a@b.example", 1001),
        Lookup::Missing
    );
    let _ = cache.clear();
}

/// Real DNS + HTTPS; run by hand: `cargo test -p penguin-desktop live_sources -- --ignored`.
#[tokio::test]
#[ignore]
async fn live_sources() {
    let net = super::net::HttpNet::new();
    assert!(super::dns::txt("nonexistent-label-xyz.example")
        .unwrap()
        .is_empty());
    let bimi = sources::bimi(&net, "linkedin.com").await;
    eprintln!(
        "bimi linkedin.com: {:?}",
        matches!(bimi, sources::Outcome::Image(_))
    );
    let icon = sources::company_icon(&net, "github.com").await;
    eprintln!(
        "icon github.com: {:?}",
        matches!(icon, sources::Outcome::Image(_))
    );
    assert!(matches!(icon, sources::Outcome::Image(_)));
}
