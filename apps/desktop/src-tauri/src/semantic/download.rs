//! One-time download of the embedding model from Hugging Face.
//!
//! Every file is fetched from `huggingface.co/<repo>/resolve/<commit>/<path>`
//! (the commit is pinned in `penguin_semantic::model`), written to
//! `<path>.part` while being hashed, and renamed into place only when its
//! size and sha256 match the pinned values. An interrupted download
//! resumes with an HTTP Range request. Nothing but these requests leaves
//! the Mac: no account, token or mail content is involved.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use penguin_semantic::model::{self, HashingWriter, ModelFile, ModelSpec};

/// Download whatever `verify` says is missing or wrong. `progress` gets
/// (bytes done, bytes total) across all files.
pub async fn ensure(
    spec: &'static ModelSpec,
    dir: &Path,
    progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<(), String> {
    let total = spec.download_bytes();
    let dir_owned = dir.to_path_buf();
    let bad = tauri::async_runtime::spawn_blocking(move || model::verify(spec, &dir_owned))
        .await
        .map_err(|e| e.to_string())?;
    if bad.is_empty() {
        progress(total, total);
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .user_agent(format!("Penguin/{}", crate::VERSION))
        .connect_timeout(Duration::from_secs(10))
        // Per read, not for the whole file: a slow link must not fail a
        // 100 MB download, only a stalled one.
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut done: u64 = spec.files.iter().filter(|f| !bad.iter().any(|(b, _)| b.path == f.path)).map(|f| f.size).sum();
    progress(done, total);
    for (file, _) in bad {
        fetch(&client, spec, dir, file, &mut done, total, &progress).await?;
    }
    Ok(())
}

async fn fetch(
    client: &reqwest::Client,
    spec: &'static ModelSpec,
    dir: &Path,
    file: &'static ModelFile,
    done: &mut u64,
    total: u64,
    progress: &(impl Fn(u64, u64) + Send + Sync),
) -> Result<(), String> {
    let dest = spec.local_path(dir, file);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let part = {
        let mut p = dest.clone().into_os_string();
        p.push(".part");
        std::path::PathBuf::from(p)
    };
    let mut hasher = HashingWriter::default();
    let mut have: u64 = 0;
    // Resume: hash what is already there.
    if let Ok(meta) = std::fs::metadata(&part) {
        if meta.len() < file.size {
            use std::io::Read;
            let mut f = std::fs::File::open(&part).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                have += n as u64;
            }
        } else {
            let _ = std::fs::remove_file(&part);
        }
    }
    let mut req = client.get(spec.url(file));
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req
        .send()
        .await
        .map_err(|e| format!("downloading {}: {e}", file.path))?;
    let status = resp.status();
    if have > 0 && status == reqwest::StatusCode::OK {
        // The server ignored the range: start over.
        hasher = HashingWriter::default();
        have = 0;
    } else if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
        return Err(format!("downloading {}: HTTP {status}", file.path));
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(have > 0)
        .truncate(have == 0)
        .open(&part)
        .map_err(|e| format!("writing {}: {e}", part.display()))?;
    *done += have;
    progress(*done, total);
    let mut written = have;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("downloading {}: {e}", file.path))?
    {
        written += chunk.len() as u64;
        if written > file.size {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{} is larger than expected", file.path));
        }
        hasher.update(&chunk);
        out.write_all(&chunk).map_err(|e| format!("writing {}: {e}", part.display()))?;
        *done += chunk.len() as u64;
        progress(*done, total);
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    if written != file.size {
        return Err(format!("{}: got {written} of {} bytes", file.path, file.size));
    }
    if hasher.finish() != file.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{}: checksum mismatch; the download was discarded", file.path));
    }
    std::fs::rename(&part, &dest).map_err(|e| format!("saving {}: {e}", dest.display()))?;
    Ok(())
}
