//! Just enough S3 on 127.0.0.1 for tests: the share tests (share/tests.rs),
//! the agent share-link tests (agent/sharing.rs) and the CLI end-to-end test
//! (tests/cli.rs includes this file by path). Never real storage.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// An S3 error document with `code`.
pub fn xml(code: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>m</Message></Error>")
}

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Just enough S3 on 127.0.0.1: PUT stores, a signed GET returns, an
/// unsigned GET is refused, DELETE removes. `refuse` makes every request
/// answer with that S3 error code instead.
pub struct Stub {
    pub endpoint: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    pub objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub refuse: Arc<Mutex<Option<(u16, &'static str)>>>,
}

impl Stub {
    pub async fn start() -> Stub {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let objects = Arc::new(Mutex::new(HashMap::new()));
        let refuse: Arc<Mutex<Option<(u16, &'static str)>>> = Arc::new(Mutex::new(None));
        let (s, o, r) = (seen.clone(), objects.clone(), refuse.clone());
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let (s, o, r) = (s.clone(), o.clone(), r.clone());
                tokio::spawn(async move {
                    let (read, mut write) = sock.into_split();
                    let mut read = BufReader::new(read);
                    loop {
                        let mut line = String::new();
                        if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let mut parts = line.split_whitespace();
                        let method = parts.next().unwrap_or_default().to_string();
                        let target =
                            url::Url::parse(&format!("http://stub{}", parts.next().unwrap_or("/")))
                                .unwrap();
                        let mut headers = HashMap::new();
                        loop {
                            let mut h = String::new();
                            read.read_line(&mut h).await.unwrap();
                            let h = h.trim_end();
                            if h.is_empty() {
                                break;
                            }
                            let (k, v) = h.split_once(':').unwrap();
                            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                        }
                        let len: usize = headers
                            .get("content-length")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        let mut body = vec![0; len];
                        read.read_exact(&mut body).await.unwrap();
                        let query: HashMap<String, String> = target
                            .query_pairs()
                            .map(|(k, v)| (k.into_owned(), v.into_owned()))
                            .collect();
                        let path = target.path().to_string();
                        s.lock().unwrap().push(Seen {
                            method: method.clone(),
                            path: path.clone(),
                            query: query.clone(),
                            headers,
                            body: body.clone(),
                        });
                        let signed = query.contains_key("X-Amz-Signature");
                        let refused = *r.lock().unwrap();
                        let (status, out): (u16, Vec<u8>) = match (refused, method.as_str()) {
                            (Some((status, code)), _) => (status, xml(code).into_bytes()),
                            (None, _) if !signed => (403, xml("AccessDenied").into_bytes()),
                            (None, "PUT") => {
                                o.lock().unwrap().insert(path, body);
                                (200, Vec::new())
                            }
                            (None, "GET") => match o.lock().unwrap().get(&path) {
                                Some(b) => (200, b.clone()),
                                None => (404, xml("NoSuchKey").into_bytes()),
                            },
                            (None, "DELETE") => {
                                o.lock().unwrap().remove(&path);
                                (204, Vec::new())
                            }
                            _ => (405, Vec::new()),
                        };
                        let head = format!(
                            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nDate: {}\r\n\r\n",
                            out.len(),
                            chrono::Utc::now().to_rfc2822().replace("+0000", "GMT")
                        );
                        if write.write_all(head.as_bytes()).await.is_err()
                            || write.write_all(&out).await.is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        Stub {
            endpoint: format!("http://127.0.0.1:{port}"),
            seen,
            objects,
            refuse,
        }
    }
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}
