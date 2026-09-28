//! Detection against a fake network. Fixture domains are `.example`; real
//! provider domains (gmail.com, …) appear only as table data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::avatars::net::BoxFut;

use super::autoconfig;
use super::detect::{parse_email, Detector, ProviderNet};
use super::ms_client;
use super::table;
use super::*;

#[derive(Clone)]
enum Answer<T> {
    Ok(T),
    Fail,
    Hang,
}

#[derive(Default)]
struct FakeNet {
    mx: HashMap<String, Answer<Vec<(u16, String)>>>,
    pages: HashMap<String, Answer<Option<String>>>,
    /// Every lookup, in order: "mx:<domain>" or the URL.
    calls: Mutex<Vec<String>>,
    /// Answer for anything not listed: Ok(None) (404) or Fail (offline).
    offline: bool,
}

impl FakeNet {
    fn mx(mut self, domain: &str, hosts: &[(u16, &str)]) -> Self {
        let v = hosts.iter().map(|(p, h)| (*p, h.to_string())).collect();
        self.mx.insert(domain.into(), Answer::Ok(v));
        self
    }
    fn mx_answer(mut self, domain: &str, a: Answer<Vec<(u16, String)>>) -> Self {
        self.mx.insert(domain.into(), a);
        self
    }
    fn page(mut self, url: &str, body: &str) -> Self {
        self.pages.insert(url.into(), Answer::Ok(Some(body.into())));
        self
    }
    fn page_answer(mut self, url: &str, a: Answer<Option<String>>) -> Self {
        self.pages.insert(url.into(), a);
        self
    }
    fn offline(mut self) -> Self {
        self.offline = true;
        self
    }
}

async fn answer<T: Clone>(a: Option<Answer<T>>, missing: Result<T, String>) -> Result<T, String> {
    match a {
        Some(Answer::Ok(v)) => Ok(v),
        Some(Answer::Fail) => Err("offline".into()),
        Some(Answer::Hang) => std::future::pending().await,
        None => missing,
    }
}

impl ProviderNet for FakeNet {
    fn mx<'a>(&'a self, domain: &'a str) -> BoxFut<'a, Result<Vec<(u16, String)>, String>> {
        self.calls.lock().unwrap().push(format!("mx:{domain}"));
        let a = self.mx.get(domain).cloned();
        let missing = if self.offline {
            Err("offline".into())
        } else {
            Ok(vec![])
        };
        Box::pin(answer(a, missing))
    }
    fn get<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<Option<String>, String>> {
        self.calls.lock().unwrap().push(url.to_string());
        let a = self.pages.get(url).cloned();
        let missing = if self.offline {
            Err("offline".into())
        } else {
            Ok(None)
        };
        Box::pin(answer(a, missing))
    }
}

fn detector(net: FakeNet) -> (Detector, Arc<FakeNet>) {
    let net = Arc::new(net);
    let d = Detector::with_timeouts(
        net.clone(),
        Duration::from_millis(200),
        Duration::from_millis(200),
    );
    (d, net)
}

fn calls(net: &FakeNet) -> Vec<String> {
    net.calls.lock().unwrap().clone()
}

fn ispdb(domain: &str) -> String {
    format!("https://autoconfig.thunderbird.net/v1.1/{domain}")
}

fn config(provider: &str, imap: &str, smtp: &str, user: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<clientConfig version="1.1">
  <emailProvider id="{provider}">
    <domain>{provider}</domain>
    <displayName>{provider} Mail</displayName>
    <incomingServer type="pop3">
      <hostname>pop.{provider}</hostname><port>995</port><socketType>SSL</socketType>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>{imap}</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <authentication>password-cleartext</authentication>
      <username>{user}</username>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>{smtp}</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#
    )
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

#[test]
fn parses_and_normalizes_addresses() {
    let a = parse_email("  Sam.Okafor@Northwind.EXAMPLE. ").unwrap();
    assert_eq!(a.email, "Sam.Okafor@northwind.example");
    assert_eq!(a.local, "Sam.Okafor");
    assert_eq!(a.domain, "northwind.example");
    assert_eq!(
        parse_email("Sam Okafor <sam@harbor.example>")
            .unwrap()
            .email,
        "sam@harbor.example"
    );
    assert_eq!(
        parse_email("mailto:sam@harbor.example").unwrap().email,
        "sam@harbor.example"
    );
    assert_eq!(
        parse_email("sam+news@harbor.example").unwrap().local,
        "sam+news"
    );
    // International domains are looked up as punycode.
    assert_eq!(
        parse_email("ana@bücher.example").unwrap().domain,
        "xn--bcher-kva.example"
    );
}

#[test]
fn rejects_things_that_are_not_addresses() {
    for bad in [
        "",
        "sam",
        "sam@",
        "@harbor.example",
        "sam@harbor",
        "sam@@harbor.example",
        "sam smith@harbor.example",
        "sam@harbor .example",
        "sam@[192.0.2.1]",
        "sam@192.0.2.1",
        "sam@-harbor.example",
        "sam@harbor..example",
        ".sam@harbor.example",
        "sa..m@harbor.example",
        "\"sam\"@harbor.example",
        "sam@harbor.x",
    ] {
        assert!(parse_email(bad).is_err(), "{bad:?} should be rejected");
    }
}

// ---------------------------------------------------------------------------
// Built-in table: instant, no network
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consumer_domains_come_from_the_table_without_network() {
    let (d, net) = detector(FakeNet::default().offline());
    let cases = [
        (
            "sam@gmail.com",
            ProviderKind::Gmail,
            SetupKind::Google,
            AuthMethod::GoogleOAuth,
        ),
        (
            "sam@googlemail.com",
            ProviderKind::Gmail,
            SetupKind::Google,
            AuthMethod::GoogleOAuth,
        ),
        (
            "sam@hotmail.co.uk",
            ProviderKind::OutlookPersonal,
            SetupKind::Microsoft,
            AuthMethod::MicrosoftOAuth,
        ),
        (
            "sam@outlook.com",
            ProviderKind::OutlookPersonal,
            SetupKind::Microsoft,
            AuthMethod::MicrosoftOAuth,
        ),
        (
            "sam@live.com.au",
            ProviderKind::OutlookPersonal,
            SetupKind::Microsoft,
            AuthMethod::MicrosoftOAuth,
        ),
        (
            "sam@yahoo.co.uk",
            ProviderKind::Yahoo,
            SetupKind::Yahoo,
            AuthMethod::AppPassword,
        ),
        (
            "sam@ymail.com",
            ProviderKind::Yahoo,
            SetupKind::Yahoo,
            AuthMethod::AppPassword,
        ),
        (
            "sam@aol.com",
            ProviderKind::Aol,
            SetupKind::Aol,
            AuthMethod::AppPassword,
        ),
        (
            "sam@me.com",
            ProviderKind::Icloud,
            SetupKind::Icloud,
            AuthMethod::AppPassword,
        ),
        (
            "sam@fastmail.fm",
            ProviderKind::Fastmail,
            SetupKind::Fastmail,
            AuthMethod::AppPassword,
        ),
        (
            "sam@proton.me",
            ProviderKind::ImapGeneric,
            SetupKind::Proton,
            AuthMethod::ImapPassword,
        ),
    ];
    for (email, kind, setup, auth) in cases {
        let p = d.detect(email, None).await.unwrap();
        assert_eq!((p.kind, p.setup, p.auth), (kind, setup, auth), "{email}");
        assert_eq!(p.source, DetectionSource::DomainTable, "{email}");
        assert!(!p.offline);
    }
    assert!(
        calls(&net).is_empty(),
        "the table never touches the network"
    );
}

#[tokio::test]
async fn presets_fill_in_usernames_per_provider() {
    let (d, _) = detector(FakeNet::default());
    let icloud = d.detect("Sam.Okafor@icloud.com", None).await.unwrap();
    let imap = icloud.imap.unwrap();
    let smtp = icloud.smtp.unwrap();
    assert_eq!(
        (imap.host.as_str(), imap.port, imap.security),
        ("imap.mail.me.com", 993, MailSecurity::Tls)
    );
    assert_eq!(
        imap.username, "Sam.Okafor",
        "iCloud IMAP wants the name without the domain"
    );
    assert_eq!(
        (smtp.host.as_str(), smtp.port, smtp.security),
        ("smtp.mail.me.com", 587, MailSecurity::Starttls)
    );
    assert_eq!(smtp.username, "Sam.Okafor@icloud.com");

    let yahoo = d.detect("sam@yahoo.com", None).await.unwrap();
    assert_eq!(yahoo.imap.as_ref().unwrap().host, "imap.mail.yahoo.com");
    assert_eq!(yahoo.smtp.as_ref().unwrap().port, 465);
    assert_eq!(yahoo.imap.unwrap().username, "sam@yahoo.com");
    assert_eq!(yahoo.display_name, "Yahoo Mail");
}

#[tokio::test]
async fn google_microsoft_and_imap_accounts_are_available() {
    let (d, _) = detector(FakeNet::default());
    for email in [
        "sam@gmail.com",
        "sam@outlook.com",
        "sam@yahoo.com",
        "sam@icloud.com",
        "sam@fastmail.com",
        "sam@proton.me",
    ] {
        assert!(d.detect(email, None).await.unwrap().available, "{email}");
    }
}

// ---------------------------------------------------------------------------
// MX
// ---------------------------------------------------------------------------

#[tokio::test]
async fn custom_domains_are_recognized_by_mx() {
    let net = FakeNet::default()
        .mx("northwind.example", &[(1, "smtp.google.com")])
        .mx(
            "oldschool.example",
            &[(10, "alt1.aspmx.l.google.com"), (1, "aspmx.l.google.com")],
        )
        .mx(
            "contoso.example",
            &[(0, "contoso-example.mail.protection.outlook.com")],
        )
        .mx("smallbiz.example", &[(1, "mta5.am0.yahoodns.net")])
        .mx("dialup.example", &[(10, "mx-aol.mail.gm0.yahoodns.net")])
        .mx(
            "family.example",
            &[(10, "mx01.mail.icloud.com"), (10, "mx02.mail.icloud.com")],
        )
        .mx("writer.example", &[(10, "in1-smtp.messagingengine.com")])
        .mx("private.example", &[(10, "mail.protonmail.ch")]);
    let (d, _) = detector(net);
    let cases = [
        (
            "a@northwind.example",
            ProviderKind::GoogleWorkspace,
            SetupKind::Google,
            "smtp.google.com",
        ),
        (
            "a@oldschool.example",
            ProviderKind::GoogleWorkspace,
            SetupKind::Google,
            "aspmx.l.google.com",
        ),
        (
            "a@contoso.example",
            ProviderKind::Microsoft365,
            SetupKind::Microsoft,
            "contoso-example.mail.protection.outlook.com",
        ),
        (
            "a@smallbiz.example",
            ProviderKind::Yahoo,
            SetupKind::Yahoo,
            "mta5.am0.yahoodns.net",
        ),
        (
            "a@dialup.example",
            ProviderKind::Aol,
            SetupKind::Aol,
            "mx-aol.mail.gm0.yahoodns.net",
        ),
        (
            "a@family.example",
            ProviderKind::Icloud,
            SetupKind::Icloud,
            "mx01.mail.icloud.com",
        ),
        (
            "a@writer.example",
            ProviderKind::Fastmail,
            SetupKind::Fastmail,
            "in1-smtp.messagingengine.com",
        ),
        (
            "a@private.example",
            ProviderKind::ImapGeneric,
            SetupKind::Proton,
            "mail.protonmail.ch",
        ),
    ];
    for (email, kind, setup, mx) in cases {
        let p = d.detect(email, None).await.unwrap();
        assert_eq!((p.kind, p.setup), (kind, setup), "{email}");
        assert_eq!(p.source, DetectionSource::Mx, "{email}");
        assert_eq!(p.mx_host.as_deref(), Some(mx), "{email}");
    }
}

#[tokio::test]
async fn a_recognized_mx_skips_autoconfig() {
    let (d, net) = detector(FakeNet::default().mx("northwind.example", &[(1, "smtp.google.com")]));
    d.detect("a@northwind.example", None).await.unwrap();
    assert_eq!(calls(&net), vec!["mx:northwind.example"]);
}

#[test]
fn mx_patterns_match_on_label_boundaries() {
    assert_eq!(
        table::mx_known("smtp.google.com."),
        Some(table::Known::GoogleWorkspace)
    );
    assert_eq!(
        table::mx_known("aspmx2.googlemail.com"),
        Some(table::Known::GoogleWorkspace)
    );
    assert_eq!(table::mx_known("notgoogle.com"), None);
    assert_eq!(table::mx_known("google.com.evil.example"), None);
    assert_eq!(
        table::mx_known("x.mail.protection.outlook.com.evil.example"),
        None
    );
    assert_eq!(
        table::mx_known("hotmail-com.olc.protection.outlook.com"),
        Some(table::Known::OutlookPersonal)
    );
    assert!(table::is_under("a.b.example", "b.example"));
    assert!(!table::is_under("ab.example", "b.example"));
}

#[test]
fn base_domains_for_the_ispdb() {
    assert_eq!(
        table::base_domain("mx1.mail.provider.example"),
        "provider.example"
    );
    assert_eq!(table::base_domain("provider.example."), "provider.example");
    assert_eq!(table::base_domain("mx.host.co.uk"), "host.co.uk");
    assert_eq!(table::base_domain("in.mail.host.com.au"), "host.com.au");
    assert_eq!(table::base_domain("example"), "example");
}

// ---------------------------------------------------------------------------
// Autoconfig and the ISPDB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn falls_back_to_the_ispdb_for_the_mx_operator() {
    let net = FakeNet::default()
        .mx(
            "harbor.example",
            &[(10, "mx2.mailhost.example"), (5, "mx1.mailhost.example")],
        )
        .page_answer(
            "https://autoconfig.harbor.example/mail/config-v1.1.xml",
            Answer::Fail,
        )
        .page(
            &ispdb("mailhost.example"),
            &config(
                "mailhost.example",
                "imap.mailhost.example",
                "smtp.mailhost.example",
                "%EMAILADDRESS%",
            ),
        );
    let (d, net) = detector(net);
    let p = d.detect("Dana@harbor.example", None).await.unwrap();
    assert_eq!(
        (p.kind, p.setup, p.auth),
        (
            ProviderKind::ImapGeneric,
            SetupKind::Imap,
            AuthMethod::ImapPassword
        )
    );
    assert_eq!(p.source, DetectionSource::Ispdb);
    assert_eq!(p.display_name, "mailhost.example Mail");
    assert_eq!(
        p.mx_host.as_deref(),
        Some("mx1.mailhost.example"),
        "lowest preference first"
    );
    assert_eq!(
        p.imap,
        Some(ServerSettings {
            host: "imap.mailhost.example".into(),
            port: 993,
            security: MailSecurity::Tls,
            username: "Dana@harbor.example".into()
        })
    );
    assert_eq!(p.smtp.unwrap().security, MailSecurity::Starttls);
    assert!(p.available, "IMAP servers from autoconfig connect");
    // Thunderbird's order, and only the domain is ever sent.
    let urls = calls(&net);
    assert_eq!(
        urls,
        vec![
            "mx:harbor.example".to_string(),
            "https://autoconfig.harbor.example/mail/config-v1.1.xml".into(),
            "https://harbor.example/.well-known/autoconfig/mail/config-v1.1.xml".into(),
            ispdb("harbor.example"),
            ispdb("mailhost.example"),
        ]
    );
    assert!(urls.iter().all(|u| !u.to_lowercase().contains("dana")));
}

#[tokio::test]
async fn the_domains_own_autoconfig_wins_over_the_ispdb() {
    let net = FakeNet::default()
        .mx("cedar.example", &[(10, "mx.cedar.example")])
        .page(
            "https://cedar.example/.well-known/autoconfig/mail/config-v1.1.xml",
            &config(
                "cedar.example",
                "imap.%EMAILDOMAIN%",
                "smtp.%EMAILDOMAIN%",
                "%EMAILLOCALPART%",
            ),
        )
        .page(
            &ispdb("cedar.example"),
            &config(
                "cedar.example",
                "imap.other.example",
                "smtp.other.example",
                "%EMAILADDRESS%",
            ),
        );
    let (d, _) = detector(net);
    let p = d.detect("mike@cedar.example", None).await.unwrap();
    assert_eq!(p.source, DetectionSource::Autoconfig);
    let imap = p.imap.unwrap();
    assert_eq!(
        imap.host, "imap.cedar.example",
        "%EMAILDOMAIN% in host names is filled in"
    );
    assert_eq!(imap.username, "mike");
}

#[tokio::test]
async fn an_ispdb_answer_that_points_at_a_known_provider_uses_its_flow() {
    let net = FakeNet::default()
        .mx("school.example", &[(10, "mx.relay.example")])
        .page(
            &ispdb("school.example"),
            &config(
                "school.example",
                "imap.gmail.com",
                "smtp.gmail.com",
                "%EMAILADDRESS%",
            ),
        );
    let (d, _) = detector(net);
    let p = d.detect("kid@school.example", None).await.unwrap();
    assert_eq!(
        (p.kind, p.setup, p.source),
        (
            ProviderKind::GoogleWorkspace,
            SetupKind::Google,
            DetectionSource::Ispdb
        )
    );
    assert!(p.available);
}

#[tokio::test]
async fn unknown_domains_say_so_and_keep_what_was_learned() {
    let (d, _) = detector(FakeNet::default().mx("tiny.example", &[(10, "mail.tiny.example")]));
    let p = d.detect("a@tiny.example", None).await.unwrap();
    assert_eq!(
        (p.kind, p.setup, p.source),
        (
            ProviderKind::Unknown,
            SetupKind::Unsupported,
            DetectionSource::Unknown
        )
    );
    assert_eq!(p.display_name, "tiny.example");
    assert_eq!(p.mx_host.as_deref(), Some("mail.tiny.example"));
    assert!(p.imap.is_none() && p.smtp.is_none());
    assert!(!p.offline && !p.available);

    // No MX at all (a typo, or a domain that takes no mail).
    let p = d.detect("a@nomail.example", None).await.unwrap();
    assert_eq!(p.kind, ProviderKind::Unknown);
    assert_eq!(p.mx_host, None);
}

#[tokio::test]
async fn security_gateways_are_named_and_not_looked_up() {
    let net = FakeNet::default().mx(
        "bigcorp.example",
        &[
            (10, "mxa-001.gslb.pphosted.com"),
            (10, "mxb-001.gslb.pphosted.com"),
        ],
    );
    let (d, net) = detector(net);
    let p = d.detect("a@bigcorp.example", None).await.unwrap();
    assert_eq!(p.kind, ProviderKind::Unknown);
    assert_eq!(p.gateway.as_deref(), Some("Proofpoint"));
    assert!(!calls(&net).contains(&ispdb("pphosted.com")));
}

// ---------------------------------------------------------------------------
// Offline, timeouts, cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn offline_is_reported_and_not_cached() {
    let (d, net) = detector(FakeNet::default().offline());
    let p = d.detect("a@harbor.example", None).await.unwrap();
    assert!(p.offline);
    assert_eq!(p.setup, SetupKind::Unsupported);
    let before = calls(&net).len();
    d.detect("a@harbor.example", None).await.unwrap();
    assert!(
        calls(&net).len() > before,
        "an offline answer is retried next time"
    );
}

#[tokio::test]
async fn dns_failing_while_the_web_answers_is_not_offline() {
    let net = FakeNet::default()
        .mx_answer("harbor.example", Answer::Fail)
        .page(
            &ispdb("harbor.example"),
            &config(
                "harbor.example",
                "imap.harbor.example",
                "smtp.harbor.example",
                "%EMAILADDRESS%",
            ),
        );
    let (d, _) = detector(net);
    let p = d.detect("a@harbor.example", None).await.unwrap();
    assert!(!p.offline);
    assert_eq!(p.source, DetectionSource::Ispdb);
}

#[tokio::test]
async fn slow_lookups_time_out() {
    let mut net = FakeNet::default().mx_answer("slow.example", Answer::Hang);
    for (_, url) in autoconfig::urls("slow.example", None) {
        net = net.page_answer(&url, Answer::Hang);
    }
    let (d, _) = detector(net);
    let started = Instant::now();
    let p = d.detect("a@slow.example", None).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert!(p.offline);
}

#[tokio::test]
async fn answers_are_cached_per_domain() {
    let net = FakeNet::default()
        .mx("harbor.example", &[(10, "mx.harbor.example")])
        .page(
            &ispdb("harbor.example"),
            &config(
                "harbor.example",
                "imap.harbor.example",
                "smtp.harbor.example",
                "%EMAILLOCALPART%",
            ),
        );
    let (d, net) = detector(net);
    let first = d.detect("dana@harbor.example", None).await.unwrap();
    let n = calls(&net).len();
    let second = d.detect("ravi@harbor.example", None).await.unwrap();
    assert_eq!(
        calls(&net).len(),
        n,
        "the second address on a domain makes no lookups"
    );
    assert_eq!(first.imap.unwrap().username, "dana");
    assert_eq!(second.imap.unwrap().username, "ravi");
}

// ---------------------------------------------------------------------------
// Manual choice
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_manual_choice_needs_no_network() {
    let (d, net) = detector(FakeNet::default().offline());
    let p = d
        .detect("sam@hotmail.com", Some(SetupKind::Microsoft))
        .await
        .unwrap();
    assert_eq!(
        (p.kind, p.source),
        (ProviderKind::OutlookPersonal, DetectionSource::Manual)
    );
    let p = d
        .detect("sam@contoso.example", Some(SetupKind::Microsoft))
        .await
        .unwrap();
    assert_eq!(p.kind, ProviderKind::Microsoft365);
    let p = d
        .detect("sam@northwind.example", Some(SetupKind::Google))
        .await
        .unwrap();
    assert_eq!(
        (p.kind, p.auth),
        (ProviderKind::GoogleWorkspace, AuthMethod::GoogleOAuth)
    );
    let p = d
        .detect("sam@custom.example", Some(SetupKind::Icloud))
        .await
        .unwrap();
    assert_eq!((p.kind, p.setup), (ProviderKind::Icloud, SetupKind::Icloud));
    let p = d
        .detect("sam@custom.example", Some(SetupKind::Imap))
        .await
        .unwrap();
    assert_eq!(
        (p.kind, p.setup, p.auth),
        (
            ProviderKind::ImapGeneric,
            SetupKind::Imap,
            AuthMethod::ImapPassword
        )
    );
    assert!(p.imap.is_none(), "nothing known: the form starts empty");
    assert_eq!(p.display_name, "Other mail server");
    assert!(d
        .detect("sam@custom.example", Some(SetupKind::Unsupported))
        .await
        .is_err());
    assert!(d
        .detect("not an address", Some(SetupKind::Google))
        .await
        .is_err());
    assert!(calls(&net).is_empty());
}

#[tokio::test]
async fn choosing_imap_keeps_detected_servers() {
    let net = FakeNet::default()
        .mx("harbor.example", &[(10, "mx.harbor.example")])
        .page(
            &ispdb("harbor.example"),
            &config(
                "harbor.example",
                "imap.harbor.example",
                "smtp.harbor.example",
                "%EMAILADDRESS%",
            ),
        );
    let (d, _) = detector(net);
    d.detect("a@harbor.example", None).await.unwrap();
    let p = d
        .detect("a@harbor.example", Some(SetupKind::Imap))
        .await
        .unwrap();
    assert_eq!(p.source, DetectionSource::Manual);
    assert_eq!(p.imap.unwrap().host, "imap.harbor.example");
}

// ---------------------------------------------------------------------------
// Autoconfig parsing
// ---------------------------------------------------------------------------

#[test]
fn autoconfig_prefers_encrypted_servers_and_validates_hosts() {
    let xml = r#"<clientConfig version="1.1"><emailProvider id="x.example">
        <incomingServer type="imap"><hostname>imap.x.example</hostname><port>143</port><socketType>plain</socketType></incomingServer>
        <incomingServer type="imap"><hostname>imap.x.example</hostname><port>143</port><socketType>STARTTLS</socketType></incomingServer>
        <outgoingServer type="smtp"><hostname>bad host!</hostname><port>465</port><socketType>SSL</socketType></outgoingServer>
        <outgoingServer type="smtp"><hostname>smtp.x.example</hostname><port>0</port><socketType>SSL</socketType></outgoingServer>
    </emailProvider></clientConfig>"#;
    let ac = autoconfig::parse(xml, "x.example").unwrap();
    let imap = ac.imap.unwrap();
    assert_eq!((imap.port, imap.security), (143, MailSecurity::Starttls));
    assert_eq!(
        imap.username, "%EMAILADDRESS%",
        "a missing username means the address"
    );
    assert!(ac.smtp.is_none(), "invalid hosts and ports are dropped");
    assert_eq!(ac.display_name, None);
}

#[test]
fn autoconfig_refuses_non_configs_and_dtds() {
    assert!(autoconfig::parse("<html><body>Not found</body></html>", "x.example").is_none());
    assert!(autoconfig::parse("not xml at all", "x.example").is_none());
    assert!(autoconfig::parse(
        "<clientConfig><emailProvider id=\"x\"/></clientConfig>",
        "x.example"
    )
    .is_none());
    let bomb = r#"<?xml version="1.0"?><!DOCTYPE c [<!ENTITY a "aaaaaaaa"><!ENTITY b "&a;&a;&a;&a;">]>
        <clientConfig><emailProvider id="x"><displayName>&b;</displayName>
        <incomingServer type="imap"><hostname>imap.x.example</hostname><port>993</port><socketType>SSL</socketType></incomingServer>
        </emailProvider></clientConfig>"#;
    assert!(autoconfig::parse(bomb, "x.example").is_none());
}

#[test]
fn autoconfig_reads_a_real_shaped_ispdb_entry() {
    // Shape of autoconfig.thunderbird.net/v1.1/icloud.com (2026-09-25).
    let xml = r#"<clientConfig version="1.1">
  <emailProvider id="me.com">
    <domain>mac.com</domain><domain>me.com</domain><domain>icloud.com</domain>
    <displayName>Apple iCloud</displayName>
    <displayShortName>Apple</displayShortName>
    <incomingServer type="imap">
      <hostname>imap.mail.me.com</hostname><port>993</port><socketType>SSL</socketType>
      <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.mail.me.com</hostname><port>587</port><socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;
    let ac = autoconfig::parse(xml, "icloud.com").unwrap();
    assert_eq!(ac.display_name.as_deref(), Some("Apple iCloud"));
    assert_eq!(
        table::imap_host_known(&ac.imap.unwrap().host),
        Some(table::Known::Icloud)
    );
}

// ---------------------------------------------------------------------------
// Wire shapes (types.ts lockstep)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detected_provider_serializes_with_the_ts_names() {
    let (d, _) = detector(FakeNet::default().mx("northwind.example", &[(1, "smtp.google.com")]));
    let v = serde_json::to_value(d.detect("sam@northwind.example", None).await.unwrap()).unwrap();
    assert_eq!(v["kind"], "googleWorkspace");
    assert_eq!(v["auth"], "googleOAuth");
    assert_eq!(v["setup"], "google");
    assert_eq!(v["source"], "mx");
    assert_eq!(v["mxHost"], "smtp.google.com");
    assert_eq!(v["displayName"], "Google Workspace");
    assert_eq!(v["imap"]["security"], "tls");
    assert_eq!(v["gateway"], serde_json::Value::Null);
    assert_eq!(v["available"], true);
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    let mut keys = keys;
    keys.sort();
    assert_eq!(
        keys,
        [
            "auth",
            "available",
            "displayName",
            "domain",
            "email",
            "gateway",
            "imap",
            "kind",
            "mxHost",
            "offline",
            "setup",
            "smtp",
            "source"
        ]
    );
    for (kind, wire) in [
        (ProviderKind::OutlookPersonal, "outlookPersonal"),
        (ProviderKind::Microsoft365, "microsoft365"),
        (ProviderKind::Icloud, "icloud"),
        (ProviderKind::ImapGeneric, "imapGeneric"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), wire);
    }
    assert_eq!(
        serde_json::to_value(AuthMethod::MicrosoftOAuth).unwrap(),
        "microsoftOAuth"
    );
    assert_eq!(
        serde_json::to_value(DetectionSource::DomainTable).unwrap(),
        "domainTable"
    );
    assert_eq!(
        serde_json::from_value::<SetupKind>("icloud".into()).unwrap(),
        SetupKind::Icloud
    );
}

#[test]
fn connect_request_matches_what_the_ui_sends_and_never_prints_the_password() {
    let json = r#"{
        "email": "sam@harbor.example",
        "kind": "imapGeneric",
        "auth": "imapPassword",
        "clientId": null,
        "password": "hunter2-app-password",
        "imap": {"host": "imap.harbor.example", "port": 993, "security": "tls", "username": "sam@harbor.example"},
        "smtp": {"host": "smtp.harbor.example", "port": 587, "security": "starttls", "username": "sam@harbor.example"}
    }"#;
    let req: ConnectAccountRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.imap.as_ref().unwrap().port, 993);
    assert_eq!(req.smtp.as_ref().unwrap().security, MailSecurity::Starttls);
    assert!(!format!("{req:?}").contains("hunter2"));
    let minimal: ConnectAccountRequest =
        serde_json::from_str(r#"{"email":"sam@outlook.com","kind":"outlookPersonal","auth":"microsoftOAuth","clientId":"1b2c3d4e-0000-1111-2222-333344445555"}"#).unwrap();
    assert_eq!(minimal.password, None);
    assert!(serde_json::from_str::<ConnectAccountRequest>(
        r#"{"email":"a@b.example","kind":"gmail","auth":"googleOAuth","extra":1}"#
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// Microsoft client ID
// ---------------------------------------------------------------------------

#[test]
fn microsoft_client_ids_are_found_and_normalized() {
    let id = "1b2c3d4e-0000-1111-2222-333344445555";
    assert_eq!(ms_client::normalize_client_id(id).unwrap(), id);
    assert_eq!(
        ms_client::normalize_client_id("{1B2C3D4E-0000-1111-2222-333344445555}").unwrap(),
        id
    );
    assert_eq!(
        ms_client::normalize_client_id(
            "Application (client) ID : 1b2c3d4e-0000-1111-2222-333344445555 "
        )
        .unwrap(),
        id
    );
    for bad in [
        "",
        "penguin",
        "1b2c3d4e-0000-1111-2222-33334444555",
        "1b2c3d4e00001111222233334444555566",
        "zb2c3d4e-0000-1111-2222-333344445555",
    ] {
        assert!(ms_client::normalize_client_id(bad).is_err(), "{bad:?}");
    }
    assert!(ms_client::normalize_client_id("ünïcode 1b2c3d4e-0000-1111-2222-333344445555").is_ok());
}

#[test]
fn microsoft_client_id_round_trips_through_the_config_dir() {
    let dir = std::env::temp_dir().join(format!("penguin-ms-client-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(ms_client::load(&dir), None);
    ms_client::save(&dir, Some("1b2c3d4e-0000-1111-2222-333344445555")).unwrap();
    assert_eq!(
        ms_client::load(&dir).as_deref(),
        Some("1b2c3d4e-0000-1111-2222-333344445555")
    );
    ms_client::save(&dir, None).unwrap();
    assert_eq!(ms_client::load(&dir), None);
    ms_client::save(&dir, None).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
