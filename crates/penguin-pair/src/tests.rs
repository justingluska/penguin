//! Protocol tests: the handshake, and each way it must refuse.

use std::time::{Duration, Instant};

use super::*;
use crate::mac::MAX_FAILURES;

fn hello() -> PhoneHello {
    PhoneHello {
        device_name: "Ada's iPhone".into(),
        device_model: "iPhone17,1".into(),
        system_version: "iOS 26.0".into(),
        app_version: "ios-1.0".into(),
        google_client_id: Some("1-abc.apps.googleusercontent.com".into()),
        accounts: vec!["grace@example.com".into()],
    }
}

fn transfer() -> Transfer {
    Transfer {
        mac_name: "Ada's MacBook".into(),
        google_client_id: Some("1-abc.apps.googleusercontent.com".into()),
        vault: SecretString::new(
            r#"{"version":3,"accounts":{"ada@example.com":{"refresh_token":"1//rt"}}}"#.into(),
        ),
        desktop_client: Some(DesktopClient {
            client_id: "1-desk.apps.googleusercontent.com".into(),
            client_secret: Some(SecretString::new("GOCSPX-fictional".into())),
        }),
        imap_vault: Some(SecretString::new(
            r#"{"version":1,"accounts":{"sam@icloud.example":{"password":"abcd-efgh-ijkl-mnop"}}}"#
                .into(),
        )),
        microsoft_vault: Some(SecretString::new(
            r#"{"version":1,"accounts":{"bea@outlook.example":{"refreshToken":"M.C5-rt","clientId":"1b2c3d4e-0000-1111-2222-333344445555"}}}"#
                .into(),
        )),
        accounts: vec![
            AccountMeta {
                email: "ada@example.com".into(),
                display_name: Some("Ada".into()),
                nickname: None,
                color: "#4F7CFF".into(),
                order: 0,
                status: AccountStatus::Token,
                provider: AccountKind::Gmail,
                provider_config: None,
            },
            AccountMeta {
                email: "sam@icloud.example".into(),
                display_name: None,
                nickname: Some("Home".into()),
                color: "#E0685A".into(),
                order: 1,
                status: AccountStatus::Token,
                provider: AccountKind::Imap,
                provider_config: Some(serde_json::json!({
                    "auth": "appPassword",
                    "imap": {"host": "imap.mail.me.com", "port": 993, "security": "tls", "username": "sam"},
                    "smtp": {"host": "smtp.mail.me.com", "port": 587, "security": "starttls", "username": "sam@icloud.example"},
                    "host": "icloud"
                })),
            },
            AccountMeta {
                email: "bea@outlook.example".into(),
                display_name: Some("Bea".into()),
                nickname: None,
                color: "#2FA37A".into(),
                order: 2,
                status: AccountStatus::Token,
                provider: AccountKind::Microsoft,
                provider_config: Some(serde_json::json!({
                    "auth": "microsoftOAuth",
                    "clientId": "1b2c3d4e-0000-1111-2222-333344445555",
                    "host": "outlookPersonal"
                })),
            },
        ],
    }
}

fn invitation(offer: &Offer) -> Invitation {
    // Through the QR text, as the phone gets it.
    let inv = offer.invitation(50123, vec!["192.168.1.20".into()]);
    Invitation::parse(&inv.to_qr_text()).unwrap()
}

#[test]
fn handshake_and_transfer() {
    let mut offer = Offer::new().unwrap();
    let inv = invitation(&offer);
    let (mut phone, hello_body) = PhoneSession::start(&inv, &hello()).unwrap();
    let mut mac = offer.accept(&hello_body, Instant::now()).unwrap();
    assert_eq!(mac.phone, hello());
    assert_eq!(mac.code(), phone.code());
    assert_eq!(mac.code().len(), 7, "{}", mac.code());

    let waiting = mac
        .seal(&MacMessage::Waiting {
            mac_name: "Ada's MacBook".into(),
        })
        .unwrap();
    assert!(!phone.verified());
    assert!(
        matches!(phone.open(&waiting).unwrap(), MacMessage::Waiting { mac_name } if mac_name == "Ada's MacBook")
    );
    assert!(phone.verified());

    let sealed = mac.seal(&MacMessage::Transfer(transfer())).unwrap();
    // No token, password or the desktop client's secret is readable on the
    // wire, nor which servers the accounts use.
    let wire = String::from_utf8_lossy(&sealed).into_owned();
    for secret in [
        "1//rt",
        "GOCSPX",
        "abcd-efgh-ijkl-mnop",
        "M.C5-rt",
        "imap.mail.me.com",
        "sam@icloud.example",
    ] {
        assert!(!wire.contains(secret), "{secret}");
    }
    let got = phone.open(&sealed).unwrap();
    assert_eq!(got, MacMessage::Transfer(transfer()));
    let MacMessage::Transfer(got) = got else {
        unreachable!()
    };
    let desktop = got.desktop_client.as_ref().unwrap();
    assert_eq!(
        desktop.client_secret.as_ref().unwrap().expose(),
        "GOCSPX-fictional"
    );
    assert!(!format!("{got:?}").contains("GOCSPX"));
    // The IMAP password and the Microsoft token arrive for the phone's vaults.
    assert!(got
        .imap_vault
        .as_ref()
        .unwrap()
        .expose()
        .contains("abcd-efgh-ijkl-mnop"));
    assert!(got
        .microsoft_vault
        .as_ref()
        .unwrap()
        .expose()
        .contains("M.C5-rt"));
    assert!(!format!("{got:?}").contains("abcd-efgh") && !format!("{got:?}").contains("M.C5"));
    assert_eq!(got.accounts[1].provider, AccountKind::Imap);
    assert_eq!(
        got.provider_config_of("sam@icloud.example").unwrap()["imap"]["host"],
        "imap.mail.me.com"
    );

    let done = PhoneDone {
        imported: vec!["ada@example.com".into()],
        failed: vec![],
    };
    let answer = phone.seal(&PhoneMessage::Done(done.clone())).unwrap();
    assert_eq!(mac.open(&answer).unwrap(), PhoneMessage::Done(done));
}

#[test]
fn each_pairing_has_its_own_code_and_keys() {
    let offer = Offer::new().unwrap();
    let inv = invitation(&offer);
    let (a, hello_a) = PhoneSession::start(&inv, &hello()).unwrap();
    let (b, hello_b) = PhoneSession::start(&inv, &hello()).unwrap();
    assert_ne!(hello_a[1..33], hello_b[1..33], "fresh phone keys");
    // Six digits can collide by chance (1 in a million); keys never do.
    let _ = (a.code(), b.code());
    let other = Offer::new().unwrap();
    assert_ne!(invitation(&other).to_qr_text(), inv.to_qr_text());
}

#[test]
fn wrong_secret_is_refused_and_the_offer_survives() {
    let mut offer = Offer::new().unwrap();
    let mut wrong = invitation(&offer);
    wrong.secret[0] ^= 1;
    let (_, bad) = PhoneSession::start(&wrong, &hello()).unwrap();
    assert_eq!(
        offer.accept(&bad, Instant::now()).err(),
        Some(PairError::BadKey)
    );
    // The real phone still pairs.
    let (_, good) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    assert!(offer.accept(&good, Instant::now()).is_ok());
}

#[test]
fn an_expired_offer_refuses_everyone() {
    let start = Instant::now();
    let mut offer = Offer::with_ttl(Duration::from_secs(120), start).unwrap();
    let (_, body) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    assert!(!offer.is_expired(start + Duration::from_secs(119)));
    assert_eq!(
        offer.accept(&body, start + Duration::from_secs(120)).err(),
        Some(PairError::Expired)
    );
}

#[test]
fn replays_are_refused() {
    let mut offer = Offer::new().unwrap();
    let (mut phone, body) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    let mut mac = offer.accept(&body, Instant::now()).unwrap();
    // The same hello again (a recording), or any other: the offer is spent.
    assert_eq!(
        offer.accept(&body, Instant::now()).err(),
        Some(PairError::AlreadyUsed)
    );
    let (_, another) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    assert_eq!(
        offer.accept(&another, Instant::now()).err(),
        Some(PairError::AlreadyUsed)
    );

    // A Mac frame replayed to the phone, or a phone frame to the Mac.
    let waiting = mac
        .seal(&MacMessage::Waiting {
            mac_name: "Mac".into(),
        })
        .unwrap();
    phone.open(&waiting).unwrap();
    assert_eq!(phone.open(&waiting).err(), Some(PairError::Tampered));
    let done = phone
        .seal(&PhoneMessage::Done(PhoneDone::default()))
        .unwrap();
    mac.open(&done).unwrap();
    assert_eq!(mac.open(&done).err(), Some(PairError::Tampered));
}

#[test]
fn reordered_frames_are_refused() {
    let mut offer = Offer::new().unwrap();
    let (mut phone, body) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    let mut mac = offer.accept(&body, Instant::now()).unwrap();
    let first = mac
        .seal(&MacMessage::Waiting {
            mac_name: "Mac".into(),
        })
        .unwrap();
    let second = mac.seal(&MacMessage::Transfer(transfer())).unwrap();
    phone.open(&first).unwrap();
    let third = mac
        .seal(&MacMessage::Declined { reason: "x".into() })
        .unwrap();
    // Skipping the transfer doesn't work.
    assert_eq!(phone.open(&third).err(), Some(PairError::Tampered));
    assert!(phone.open(&second).is_ok());
}

#[test]
fn tampered_frames_are_refused() {
    let mut offer = Offer::new().unwrap();
    let (mut phone, body) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();

    // Any changed byte of the hello: wrong key or a failed tag.
    for i in [1, 20, 40, body.len() - 1] {
        let mut bent = body.clone();
        bent[i] ^= 0x40;
        assert_eq!(
            offer.accept(&bent, Instant::now()).err(),
            Some(PairError::BadKey),
            "byte {i}"
        );
    }
    let mut mac = offer.accept(&body, Instant::now()).unwrap();
    let waiting = mac
        .seal(&MacMessage::Waiting {
            mac_name: "Mac".into(),
        })
        .unwrap();
    phone.open(&waiting).unwrap();
    let mut sealed = mac.seal(&MacMessage::Transfer(transfer())).unwrap();
    sealed[5] ^= 1;
    assert_eq!(phone.open(&sealed).err(), Some(PairError::Tampered));
    let mut cut = mac.seal(&MacMessage::Transfer(transfer())).unwrap();
    cut.truncate(cut.len() - 1);
    assert!(phone.open(&cut).is_err());
}

/// Someone on the network who saw the QR code (so knows the secret and the
/// Mac's public key) but doesn't have the Mac's private key.
#[test]
fn a_man_in_the_middle_cant_read_or_answer() {
    let mut real = Offer::new().unwrap();
    let inv = invitation(&real);
    let (mut phone, body) = PhoneSession::start(&inv, &hello()).unwrap();

    // Posing as the Mac: same secret, its own key. The phone's hello
    // doesn't open for it…
    let mut impostor = Offer::with_secret(*inv.secret);
    assert_eq!(
        impostor.accept(&body, Instant::now()).err(),
        Some(PairError::BadKey)
    );
    // …and whatever it sends the phone doesn't open either: the phone's
    // keys are bound to the Mac key from the QR code.
    let impostor_inv = impostor.invitation(1, vec!["10.0.0.9".into()]);
    let (_, its_own_hello) = PhoneSession::start(&impostor_inv, &hello()).unwrap();
    let mut impostor_session = impostor.accept(&its_own_hello, Instant::now()).unwrap();
    let forged = impostor_session
        .seal(&MacMessage::Transfer(transfer()))
        .unwrap();
    assert_eq!(phone.open(&forged).err(), Some(PairError::BadKey));

    // Swapping in its own public key on the way to the Mac: refused.
    let (attacker, attacker_hello) = PhoneSession::start(&inv, &hello()).unwrap();
    let mut spliced = body.clone();
    spliced[1..33].copy_from_slice(&attacker_hello[1..33]);
    assert_eq!(
        real.accept(&spliced, Instant::now()).err(),
        Some(PairError::BadKey)
    );
    drop(attacker);

    // Relaying unchanged works, and stays unreadable to the relay.
    let mut mac = real.accept(&body, Instant::now()).unwrap();
    let sealed = mac.seal(&MacMessage::Transfer(transfer())).unwrap();
    assert!(phone.open(&sealed).is_ok());
}

#[test]
fn too_many_wrong_hellos_withdraw_the_offer() {
    let mut offer = Offer::new().unwrap();
    let mut wrong = invitation(&offer);
    wrong.secret[31] ^= 0xff;
    for attempt in 1..=MAX_FAILURES {
        let (_, bad) = PhoneSession::start(&wrong, &hello()).unwrap();
        let expected = if attempt == MAX_FAILURES {
            PairError::TooManyAttempts
        } else {
            PairError::BadKey
        };
        assert_eq!(offer.accept(&bad, Instant::now()).err(), Some(expected));
    }
    let (_, good) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    assert_eq!(
        offer.accept(&good, Instant::now()).err(),
        Some(PairError::TooManyAttempts)
    );
}

#[test]
fn junk_and_low_order_keys_are_refused() {
    let mut offer = Offer::new().unwrap();
    // Too short, wrong version, all-zero (low-order) phone key.
    assert!(offer.accept(b"", Instant::now()).is_err());
    let mut newer = vec![2u8; 80];
    newer[0] = PROTOCOL_VERSION + 1;
    assert_eq!(
        offer.accept(&newer, Instant::now()).err(),
        Some(PairError::UnsupportedVersion)
    );
    // A hello from a version 1 or 2 phone: refused as older, never opened.
    for version in [1, 2] {
        let mut older = vec![2u8; 80];
        older[0] = version;
        assert_eq!(
            offer.accept(&older, Instant::now()).err(),
            Some(PairError::OlderVersion),
            "version {version}"
        );
    }
    let mut zero = vec![0u8; 80];
    zero[0] = PROTOCOL_VERSION;
    assert_eq!(
        offer.accept(&zero, Instant::now()).err(),
        Some(PairError::BadKey)
    );
    // A forged QR code with a low-order Mac key.
    let mut forged = invitation(&offer);
    forged.mac_public = [0; 32];
    assert_eq!(
        PhoneSession::start(&forged, &hello()).err(),
        Some(PairError::NotACode)
    );
    // The real phone still gets in.
    let (_, good) = PhoneSession::start(&invitation(&offer), &hello()).unwrap();
    assert!(offer.accept(&good, Instant::now()).is_ok());
}

// ---- over TCP, with the Mac's listener --------------------------------------

#[cfg(feature = "tokio")]
mod over_tcp {
    use super::*;
    use crate::server::{bind, serve, Decision, Outcome, Timeouts};
    use tokio::net::TcpStream;

    async fn phone_connects(inv: &Invitation) -> (PhoneSession, TcpStream) {
        let (session, body) = PhoneSession::start(inv, &hello()).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", inv.port)).await.unwrap();
        frame::write(&mut stream, &body).await.unwrap();
        (session, stream)
    }

    async fn listen() -> (tokio::net::TcpListener, u16) {
        let listener = bind().await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[tokio::test]
    async fn pairs_end_to_end() {
        let (listener, port) = listen().await;
        let offer = Offer::new().unwrap();
        let inv = offer.invitation(port, vec!["127.0.0.1".into()]);
        let (code_tx, code_rx) = tokio::sync::oneshot::channel();
        let mac = tokio::spawn(serve(
            listener,
            offer,
            "Ada's MacBook".into(),
            move |phone: PhoneHello, code: String| async move {
                assert_eq!(phone.device_name, "Ada's iPhone");
                code_tx.send(code).unwrap();
                Decision::Send(transfer())
            },
            Timeouts::default(),
        ));

        // A stranger without the code first: refused, the pairing goes on.
        let mut stranger_inv = inv.clone();
        stranger_inv.secret[0] ^= 1;
        let (_, mut stranger) = phone_connects(&stranger_inv).await;
        assert!(frame::read(&mut stranger).await.is_err());

        let (mut phone, mut stream) = phone_connects(&inv).await;
        let waiting = phone
            .open(&frame::read(&mut stream).await.unwrap())
            .unwrap();
        assert!(matches!(waiting, MacMessage::Waiting { .. }));
        let MacMessage::Transfer(got) = phone
            .open(&frame::read(&mut stream).await.unwrap())
            .unwrap()
        else {
            panic!("expected the transfer")
        };
        assert_eq!(got, transfer());
        assert_eq!(code_rx.await.unwrap(), phone.code());

        // Single use: the listener is gone once a phone has paired.
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());

        let done = PhoneDone {
            imported: vec!["ada@example.com".into()],
            failed: vec![],
        };
        let answer = phone.seal(&PhoneMessage::Done(done.clone())).unwrap();
        frame::write(&mut stream, &answer).await.unwrap();
        match mac.await.unwrap().unwrap() {
            Outcome::Done { phone, done: got } => {
                assert_eq!(phone.device_name, "Ada's iPhone");
                assert_eq!(got, done);
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn declining_tells_the_phone() {
        let (listener, port) = listen().await;
        let offer = Offer::new().unwrap();
        let inv = offer.invitation(port, vec!["127.0.0.1".into()]);
        let mac = tokio::spawn(serve(
            listener,
            offer,
            "Mac".into(),
            |_: PhoneHello, _: String| async { Decision::Decline("Not this phone.".into()) },
            Timeouts::default(),
        ));
        let (mut phone, mut stream) = phone_connects(&inv).await;
        phone
            .open(&frame::read(&mut stream).await.unwrap())
            .unwrap();
        assert_eq!(
            phone
                .open(&frame::read(&mut stream).await.unwrap())
                .unwrap(),
            MacMessage::Declined {
                reason: "Not this phone.".into()
            }
        );
        assert!(matches!(
            mac.await.unwrap().unwrap(),
            Outcome::Declined { .. }
        ));
    }

    #[tokio::test]
    async fn a_phone_that_leaves_while_the_mac_decides_ends_it() {
        let (listener, port) = listen().await;
        let offer = Offer::new().unwrap();
        let inv = offer.invitation(port, vec!["127.0.0.1".into()]);
        let mac = tokio::spawn(serve(
            listener,
            offer,
            "Mac".into(),
            |_: PhoneHello, _: String| std::future::pending::<Decision>(),
            Timeouts::default(),
        ));
        let (mut phone, mut stream) = phone_connects(&inv).await;
        phone
            .open(&frame::read(&mut stream).await.unwrap())
            .unwrap();
        drop(stream);
        assert_eq!(mac.await.unwrap().err(), Some(PairError::Disconnected));
    }

    #[tokio::test(start_paused = true)]
    async fn nobody_scanning_expires() {
        let (listener, _) = listen().await;
        let offer = Offer::with_ttl(Duration::from_secs(120), Instant::now()).unwrap();
        let mac = serve(
            listener,
            offer,
            "Mac".into(),
            |_: PhoneHello, _: String| async { Decision::Decline(String::new()) },
            Timeouts::default(),
        );
        assert_eq!(mac.await.err(), Some(PairError::Expired));
    }
}
