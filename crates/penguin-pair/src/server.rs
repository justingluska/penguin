//! The Mac's listener (feature `tokio`): accept connections until one
//! brings a hello that opens, then stop listening, ask the app to confirm,
//! send the transfer and wait for the phone's answer.

use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::{
    frame, MacMessage, Offer, PairError, PhoneDone, PhoneHello, PhoneMessage, Result, Transfer,
};

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// From connecting to a complete hello.
    pub hello: Duration,
    /// For the user to click Send (the phone waits meanwhile).
    pub decide: Duration,
    /// For the phone to store the accounts and answer.
    pub reply: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            hello: Duration::from_secs(10),
            decide: Duration::from_secs(5 * 60),
            reply: Duration::from_secs(60),
        }
    }
}

/// The app's answer once a phone has connected.
pub enum Decision {
    Send(Transfer),
    /// Said no (the reason is shown on the phone).
    Decline(String),
}

#[derive(Debug)]
pub enum Outcome {
    Done { phone: PhoneHello, done: PhoneDone },
    Declined { phone: PhoneHello },
}

/// Connections being read at once; more wait in the kernel's backlog.
const MAX_PENDING_HELLOS: usize = 16;

/// A listener on every interface: dual-stack `[::]` where IPv6 exists, else
/// `0.0.0.0`. The port is picked by the system.
pub async fn bind() -> std::io::Result<TcpListener> {
    match TcpListener::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))).await {
        Ok(l) => Ok(l),
        Err(_) => TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await,
    }
}

/// Run one pairing on `listener` until it finishes, fails or expires.
/// `decide` is called once, when a phone has proved it holds the QR code,
/// with what it said about itself and the code both screens show.
/// Dropping the future cancels the pairing and closes the listener.
pub async fn serve<F, Fut>(
    listener: TcpListener,
    mut offer: Offer,
    mac_name: String,
    decide: F,
    timeouts: Timeouts,
) -> Result<Outcome>
where
    F: FnOnce(PhoneHello, String) -> Fut,
    Fut: Future<Output = Decision>,
{
    let deadline =
        tokio::time::Instant::now() + offer.expires_at().saturating_duration_since(Instant::now());
    let mut hellos: JoinSet<Result<(TcpStream, Vec<u8>)>> = JoinSet::new();
    let (stream, mut session) = loop {
        tokio::select! {
            accepted = listener.accept(), if hellos.len() < MAX_PENDING_HELLOS => match accepted {
                Ok((stream, _peer)) => {
                    let _ = stream.set_nodelay(true);
                    hellos.spawn(read_hello(stream, timeouts.hello));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "pairing listener accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            Some(joined) = hellos.join_next() => {
                let Ok(Ok((stream, body))) = joined else { continue };
                match offer.accept(&body, Instant::now()) {
                    Ok(session) => break (stream, session),
                    Err(e @ (PairError::TooManyAttempts | PairError::Expired)) => return Err(e),
                    Err(e) => tracing::warn!(error = %e, "refused a pairing hello"),
                }
            },
            _ = tokio::time::sleep_until(deadline) => return Err(PairError::Expired),
        }
    };
    // Single use: nobody else gets to connect.
    drop(listener);
    hellos.abort_all();
    tracing::info!("phone paired; waiting for the user to confirm");

    let (mut reader, mut writer) = stream.into_split();
    let waiting = session.seal(&MacMessage::Waiting {
        mac_name: mac_name.clone(),
    })?;
    frame::write(&mut writer, &waiting).await?;

    let phone = session.phone.clone();
    // The phone says nothing until it has the transfer, so a read that ends
    // early means it left (or misbehaved).
    let reply = frame::read(&mut reader);
    tokio::pin!(reply);
    let decided = tokio::select! {
        d = tokio::time::timeout(timeouts.decide, decide(phone.clone(), session.code().to_owned())) => d,
        early = &mut reply => {
            return Err(match early {
                Ok(_) => PairError::Protocol("the phone spoke out of turn".into()),
                Err(e) => e,
            })
        }
    };
    let transfer = match decided {
        Ok(Decision::Send(transfer)) => transfer,
        Ok(Decision::Decline(reason)) => {
            let declined = session.seal(&MacMessage::Declined { reason })?;
            frame::write(&mut writer, &declined).await?;
            return Ok(Outcome::Declined { phone });
        }
        Err(_) => {
            let declined = session.seal(&MacMessage::Declined {
                reason: "Nobody confirmed on the Mac in time.".into(),
            })?;
            let _ = frame::write(&mut writer, &declined).await;
            return Err(PairError::TimedOut);
        }
    };
    let sealed = session.seal(&MacMessage::Transfer(transfer))?;
    frame::write(&mut writer, &sealed).await?;
    let body = tokio::time::timeout(timeouts.reply, &mut reply)
        .await
        .map_err(|_| PairError::TimedOut)??;
    let PhoneMessage::Done(done) = session.open(&body)?;
    Ok(Outcome::Done { phone, done })
}

async fn read_hello(mut stream: TcpStream, limit: Duration) -> Result<(TcpStream, Vec<u8>)> {
    let body = tokio::time::timeout(limit, frame::read(&mut stream))
        .await
        .map_err(|_| PairError::TimedOut)??;
    Ok((stream, body))
}
