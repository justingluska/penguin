//! Account photos (account.rs) against a fake network. The resolver's own
//! tests live with it in crates/penguin-avatars.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Mutex;

use super::cache::{Cache, Lookup, SourceKind};
use super::net::{BoxFut, FetchError, Net};
use super::*;

#[derive(Default)]
struct FakeNet {
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
            Ok(Vec::new())
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

fn temp(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("penguin-avatars-acct-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

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
