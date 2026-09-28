//! DNS lookups through the system resolver (libresolv), so the query goes
//! wherever the Mac's DNS already goes; no DNS-over-HTTPS provider learns who
//! writes to you. TXT for BIMI (avatars), MX for provider detection when an
//! account is added (src/providers/). One query at a time: the classic
//! `res_query` API keeps global state.

use std::sync::Mutex;

static RESOLVER: Mutex<()> = Mutex::new(());

const T_TXT: u16 = 16;
const T_MX: u16 = 15;
const C_IN: i32 = 1;

fn plausible_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// TXT records for `name`, each record's strings joined. `Ok(vec![])` for
/// NXDOMAIN/no data; `Err` when the resolver itself failed. Blocking.
pub fn txt(name: &str) -> Result<Vec<String>, String> {
    if !plausible_name(name) {
        return Ok(vec![]);
    }
    let answer = query(name, T_TXT)?;
    Ok(answer.map(|a| parse_txt(&a)).unwrap_or_default())
}

/// MX records for `name` as (preference, lowercased exchange host without
/// the trailing dot), lowest preference first. `Ok(vec![])` for NXDOMAIN, no
/// data or a null MX; `Err` when the resolver itself failed (offline,
/// timeout, SERVFAIL). Blocking.
pub fn mx(name: &str) -> Result<Vec<(u16, String)>, String> {
    if !plausible_name(name) {
        return Ok(vec![]);
    }
    let answer = query(name, T_MX)?;
    let mut records = answer.map(|a| parse_mx(&a)).unwrap_or_default();
    records.sort();
    Ok(records)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn query(name: &str, typ: u16) -> Result<Option<Vec<u8>>, String> {
    use std::ffi::{c_char, c_int, c_uchar, CString};

    #[link(name = "resolv")]
    extern "C" {
        // Apple's libresolv exports the BIND 9 names.
        #[cfg_attr(target_os = "macos", link_name = "res_9_query")]
        // glibc 2.34+ exports res_query itself; __res_query is kept only for old binaries.
        #[cfg_attr(target_os = "linux", link_name = "res_query")]
        fn res_query(
            dname: *const c_char,
            class: c_int,
            typ: c_int,
            answer: *mut c_uchar,
            anslen: c_int,
        ) -> c_int;
    }
    #[cfg(target_os = "macos")]
    extern "C" {
        static h_errno: c_int;
    }
    #[cfg(target_os = "linux")]
    extern "C" {
        fn __h_errno_location() -> *mut c_int;
    }
    const HOST_NOT_FOUND: c_int = 1;
    const NO_DATA: c_int = 4;

    let cname = CString::new(name).map_err(|_| "bad name".to_string())?;
    let mut buf = vec![0u8; 8192];
    let _guard = RESOLVER.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: `cname` is NUL-terminated and outlives the call; `buf` is a
    // writable buffer of the length we pass; calls are serialized above.
    let n = unsafe {
        res_query(
            cname.as_ptr(),
            C_IN,
            typ as c_int,
            buf.as_mut_ptr(),
            buf.len() as c_int,
        )
    };
    if n < 0 {
        // SAFETY: read right after the failed call, still under the lock.
        #[cfg(target_os = "macos")]
        let err = unsafe { h_errno };
        #[cfg(target_os = "linux")]
        let err = unsafe { *__h_errno_location() };
        // NXDOMAIN / no TXT record is a real answer; anything else
        // (SERVFAIL, timeout, offline) says nothing about the domain.
        return match err {
            HOST_NOT_FOUND | NO_DATA => Ok(None),
            other => Err(format!("DNS lookup failed (h_errno {other})")),
        };
    }
    buf.truncate((n as usize).min(buf.len()));
    Ok(Some(buf))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn query(_name: &str, _typ: u16) -> Result<Option<Vec<u8>>, String> {
    Err("DNS lookups are not supported on this platform".into())
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

/// Skip a (possibly compressed) domain name; returns the offset after it.
fn skip_name(b: &[u8], mut i: usize) -> Option<usize> {
    for _ in 0..128 {
        let len = *b.get(i)? as usize;
        if len == 0 {
            return Some(i + 1);
        }
        if len & 0xc0 == 0xc0 {
            b.get(i + 1)?;
            return Some(i + 2);
        }
        i += 1 + len;
    }
    None
}

/// Read a (possibly compressed) domain name at `i` as dotted lowercase text.
/// Compression pointers must point backwards, so a pointer loop ends.
fn read_name(b: &[u8], mut i: usize) -> Option<String> {
    let mut labels: Vec<String> = Vec::new();
    let mut total = 0usize;
    for _ in 0..128 {
        let len = *b.get(i)? as usize;
        if len == 0 {
            return Some(labels.join("."));
        }
        if len & 0xc0 == 0xc0 {
            let target = ((len & 0x3f) << 8) | *b.get(i + 1)? as usize;
            if target >= i {
                return None;
            }
            i = target;
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        let label = b.get(i + 1..i + 1 + len)?;
        total += len + 1;
        if total > 255 {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        i += 1 + len;
    }
    None
}

/// Walk the answer section, calling `f(type, rdata_start, rdata_end)` per
/// record. Stops quietly at the first malformed record.
fn each_answer(b: &[u8], mut f: impl FnMut(u16, usize, usize)) {
    let (Some(qd), Some(an)) = (u16_at(b, 4), u16_at(b, 6)) else {
        return;
    };
    let mut i = 12;
    for _ in 0..qd {
        let Some(next) = skip_name(b, i) else {
            return;
        };
        i = next + 4;
    }
    for _ in 0..an {
        let Some(next) = skip_name(b, i) else {
            return;
        };
        i = next;
        let (Some(typ), Some(rdlen)) = (u16_at(b, i), u16_at(b, i + 8)) else {
            return;
        };
        let start = i + 10;
        let end = start + rdlen as usize;
        if end > b.len() {
            return;
        }
        f(typ, start, end);
        i = end;
    }
}

/// TXT rdata from a DNS response. Malformed input yields what parsed cleanly.
pub fn parse_txt(b: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    each_answer(b, |typ, start, end| {
        if typ != T_TXT {
            return;
        }
        let mut text = Vec::new();
        let mut j = start;
        while j < end {
            let len = b[j] as usize;
            let s = (j + 1).min(end);
            let e = (j + 1 + len).min(end);
            text.extend_from_slice(&b[s..e]);
            j = j + 1 + len;
        }
        out.push(String::from_utf8_lossy(&text).into_owned());
    });
    out
}

/// MX rdata from a DNS response as (preference, exchange). A null MX
/// ("." per RFC 7505: the domain takes no mail) is left out. Malformed
/// input yields what parsed cleanly.
pub fn parse_mx(b: &[u8]) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    each_answer(b, |typ, start, end| {
        if typ != T_MX || end < start + 3 {
            return;
        }
        let (Some(pref), Some(host)) = (u16_at(b, start), read_name(b, start + 2)) else {
            return;
        };
        if !host.is_empty() {
            out.push((pref, host));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(n: &str) -> Vec<u8> {
        let mut v = Vec::new();
        for label in n.split('.') {
            v.push(label.len() as u8);
            v.extend_from_slice(label.as_bytes());
        }
        v.push(0);
        v
    }

    /// A response with one question and the given (type, rdata) answers,
    /// answer names compressed to the question.
    fn response(q: &str, answers: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut b = vec![0x12, 0x34, 0x81, 0x80, 0, 1];
        b.extend_from_slice(&(answers.len() as u16).to_be_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.extend(name(q));
        b.extend_from_slice(&[0, 16, 0, 1]);
        for (typ, rdata) in answers {
            b.extend_from_slice(&[0xc0, 12]);
            b.extend_from_slice(&typ.to_be_bytes());
            b.extend_from_slice(&[0, 1, 0, 0, 0x0e, 0x10]);
            b.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            b.extend_from_slice(rdata);
        }
        b
    }

    fn strings(parts: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        for p in parts {
            v.push(p.len() as u8);
            v.extend_from_slice(p.as_bytes());
        }
        v
    }

    #[test]
    fn parses_multi_string_txt_and_skips_other_types() {
        let resp = response(
            "default._bimi.shop.example",
            &[
                (5, name("elsewhere.example")),
                (
                    16,
                    strings(&["v=BIMI1; l=https://shop.example/", "logo.svg; a="]),
                ),
            ],
        );
        assert_eq!(
            parse_txt(&resp),
            vec!["v=BIMI1; l=https://shop.example/logo.svg; a="]
        );
    }

    #[test]
    fn truncated_or_garbage_responses_do_not_panic() {
        let resp = response("a.example", &[(16, strings(&["hello"]))]);
        for n in 0..resp.len() {
            let _ = parse_txt(&resp[..n]);
        }
        let _ = parse_txt(&[0xff; 40]);
        // A compression loop must terminate.
        let mut looped = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        looped.extend_from_slice(&[0xc0, 12]);
        assert!(parse_txt(&looped).is_empty());
    }

    #[test]
    fn refuses_odd_names_without_querying() {
        assert_eq!(txt("bad name.example").unwrap(), Vec::<String>::new());
        assert_eq!(txt("").unwrap(), Vec::<String>::new());
        assert_eq!(mx("bad name.example").unwrap(), Vec::<(u16, String)>::new());
    }

    fn mx_rdata(pref: u16, exchange: &[u8]) -> Vec<u8> {
        let mut v = pref.to_be_bytes().to_vec();
        v.extend_from_slice(exchange);
        v
    }

    #[test]
    fn parses_mx_with_plain_and_compressed_exchanges() {
        // "in2" + pointer to the question name (offset 12) = in2.mail.example.
        let mut compressed = vec![3, b'i', b'n', b'2'];
        compressed.extend_from_slice(&[0xc0, 12]);
        let resp = response(
            "mail.example",
            &[
                (15, mx_rdata(20, &compressed)),
                (15, mx_rdata(10, &name("MX1.Provider.example"))),
                (16, strings(&["not an mx"])),
            ],
        );
        assert_eq!(
            parse_mx(&resp),
            vec![
                (20, "in2.mail.example".to_string()),
                (10, "mx1.provider.example".to_string())
            ]
        );
    }

    #[test]
    fn null_mx_and_bad_pointers_yield_nothing() {
        // RFC 7505 null MX: preference 0, exchange ".".
        let null = response("nomail.example", &[(15, mx_rdata(0, &[0]))]);
        assert!(parse_mx(&null).is_empty());
        // A pointer forward (or to itself) is refused instead of looping.
        let resp = response("a.example", &[(15, mx_rdata(10, &[0xc0, 0xff]))]);
        assert!(parse_mx(&resp).is_empty());
        let good = response("a.example", &[(15, mx_rdata(5, &name("mx.a.example")))]);
        for n in 0..good.len() {
            let _ = parse_mx(&good[..n]);
        }
        let _ = parse_mx(&[0xff; 64]);
    }
}
