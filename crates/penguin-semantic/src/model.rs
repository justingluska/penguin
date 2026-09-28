//! Which embedding model Penguin uses, pinned to the byte.
//!
//! A [`ModelSpec`] names a Hugging Face repository at a fixed commit, the
//! exact files (size + sha256), how to prompt and pool the model, and the
//! vector size kept. The app downloads the files once into its data
//! directory (`models/<key>/`), checks every byte against these hashes, and
//! only then loads them; nothing is fetched at build time or bundled in the
//! app. Changing anything here changes [`ModelSpec::model_id`], and the
//! index re-embeds everything with the new model.
//!
//! The choice and the evidence behind it are in docs/SEMANTIC.md.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::chunk::CHUNKER_VERSION;

/// How token vectors become one vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    /// Average over non-padding tokens.
    Mean,
    /// The first ([CLS]) token.
    Cls,
    /// A graph output that is already pooled (e.g. `sentence_embedding`).
    Output(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    /// Path inside the repository (and inside the local model directory).
    pub path: &'static str,
    pub size: u64,
    /// Lowercase hex sha256 (Hugging Face's LFS object id for LFS files).
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// Short stable name, also the local directory name.
    pub key: &'static str,
    /// Human-readable name for Settings and debug info.
    pub display: &'static str,
    pub license: &'static str,
    pub repo: &'static str,
    /// Full commit sha: downloads resolve exactly this revision.
    pub revision: &'static str,
    pub files: &'static [ModelFile],
    pub onnx_file: &'static str,
    pub tokenizer_file: &'static str,
    pub query_prefix: &'static str,
    pub passage_prefix: &'static str,
    pub pooling: Pooling,
    /// Dimensions kept (Matryoshka models may be cut below their native size).
    pub dims: usize,
    /// Tokens per passage the model sees (longer input is truncated).
    pub max_tokens: usize,
}

impl ModelSpec {
    /// Stored with every vector: model, revision, output size, prompt/pool
    /// rules and the chunker version. Any change re-embeds.
    pub fn model_id(&self) -> String {
        format!(
            "{}@{}:{}d:t{}:c{}",
            self.key,
            &self.revision[..12.min(self.revision.len())],
            self.dims,
            self.max_tokens,
            CHUNKER_VERSION
        )
    }

    /// `https://huggingface.co/<repo>/resolve/<revision>/<path>`.
    pub fn url(&self, file: &ModelFile) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, file.path
        )
    }

    /// Total bytes to download.
    pub fn download_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Where file `f` lives under the model directory.
    pub fn local_path(&self, dir: &Path, f: &ModelFile) -> PathBuf {
        dir.join(f.path)
    }
}

/// multilingual-e5-small, int8 (Xenova's ONNX export of
/// intfloat/multilingual-e5-small; weights MIT). The permissive, faster
/// alternative: about a quarter of EmbeddingGemma's compute per passage and
/// clearly weaker retrieval (docs/SEMANTIC.md). Switch by pointing
/// [`DEFAULT`] here; every vector is re-embedded on the next start.
pub const E5_SMALL_INT8: ModelSpec = ModelSpec {
    key: "multilingual-e5-small-int8",
    display: "multilingual-e5-small (int8)",
    license: "MIT",
    repo: "Xenova/multilingual-e5-small",
    revision: "761b726dd34fb83930e26aab4e9ac3899aa1fa78",
    files: &[
        ModelFile {
            path: "onnx/model_quantized.onnx",
            size: 118_308_185,
            sha256: "f80102d3f2a1229f387d3c81909990d8945513e347b0eab049f7de3c6f98c193",
        },
        ModelFile {
            path: "tokenizer.json",
            size: 17_082_730,
            sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
        },
    ],
    onnx_file: "onnx/model_quantized.onnx",
    tokenizer_file: "tokenizer.json",
    query_prefix: "query: ",
    passage_prefix: "passage: ",
    pooling: Pooling::Mean,
    dims: 384,
    max_tokens: 256,
};

/// EmbeddingGemma-300m, 4-bit (onnx-community export of
/// google/embeddinggemma-300m: block-quantized `MatMulNBits` weights and a
/// `GatherBlockQuantized` embedding table). Weights under the Gemma Terms
/// of Use (https://ai.google.dev/gemma/terms); the upstream repository is
/// gated, this mirror is not. Not the int8 export: it stores weight-only
/// QDQ (`DequantizeLinear` → `MatMul`/`Gather`), so ONNX Runtime
/// dequantizes the 262k×768 embedding table on every run (≈600 ms per query
/// measured); and not fp16 (EmbeddingGemma activations don't support it).
pub const EMBEDDINGGEMMA_Q4: ModelSpec = ModelSpec {
    key: "embeddinggemma-300m-q4",
    display: "EmbeddingGemma 300M (4-bit)",
    license: "Gemma Terms of Use",
    repo: "onnx-community/embeddinggemma-300m-ONNX",
    revision: "5090578d9565bb06545b4552f76e6bc2c93e4a66",
    files: &[
        ModelFile {
            path: "onnx/model_q4.onnx",
            size: 519_322,
            sha256: "ad1dfee81a70f7944b9b9d1cc6e48075b832881cf33fab2f2b248be78f3f0043",
        },
        ModelFile {
            path: "onnx/model_q4.onnx_data",
            size: 196_725_760,
            sha256: "599962c3143b040de2dd05e5975be3e9091dd067cacc6a8f7186e3203bab9e02",
        },
        ModelFile {
            path: "tokenizer.json",
            size: 20_323_312,
            sha256: "4dda02faaf32bc91031dc8c88457ac272b00c1016cc679757d1c441b248b9c47",
        },
    ],
    onnx_file: "onnx/model_q4.onnx",
    tokenizer_file: "tokenizer.json",
    // Google's retrieval prompts (model card, "Prompt instructions").
    query_prefix: "task: search result | query: ",
    passage_prefix: "title: none | text: ",
    pooling: Pooling::Output("sentence_embedding"),
    // Matryoshka: the first dimensions of 768, renormalized.
    dims: 256,
    max_tokens: 256,
};

/// The model Penguin uses. See docs/SEMANTIC.md for why.
pub const DEFAULT: &ModelSpec = &EMBEDDINGGEMMA_Q4;

/// Every spec the app knows (for tests and the benchmark).
pub const ALL: &[&ModelSpec] = &[&EMBEDDINGGEMMA_Q4, &E5_SMALL_INT8];

/// Why a model directory can't be used yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Missing,
    WrongSize { expected: u64, actual: u64 },
    WrongHash,
}

/// Check every file of `spec` under `dir`: size first (cheap), then the
/// sha256 of the content. Returns the files that are not right.
pub fn verify(spec: &ModelSpec, dir: &Path) -> Vec<(&'static ModelFile, FileState)> {
    let mut bad = Vec::new();
    for f in spec.files {
        let p = spec.local_path(dir, f);
        match std::fs::metadata(&p) {
            Err(_) => bad.push((f, FileState::Missing)),
            Ok(m) if m.len() != f.size => bad.push((
                f,
                FileState::WrongSize {
                    expected: f.size,
                    actual: m.len(),
                },
            )),
            Ok(_) => match sha256_file(&p) {
                Ok(h) if h == f.sha256 => {}
                _ => bad.push((f, FileState::WrongHash)),
            },
        }
    }
    bad
}

/// The file in a model directory recording what [`verify_cached`] last
/// passed.
pub const STAMP_FILE: &str = ".verified";

/// Files changed less than this long ago aren't stamped: some filesystems
/// keep change times at a coarse tick, so a write landing in the same tick
/// as the check would leave the metadata unchanged (git's "racy git"
/// problem, git-scm.com/docs/racy-git). They're hashed again next time.
const STAMP_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(2);

/// [`verify`], without re-reading files that passed before and haven't
/// changed since. After a full pass, a stamp in the directory records each
/// file's size, inode, device, and modification and change times (and the
/// expected hash); while all of them still match, the files aren't hashed
/// again. Any write, replacement, `touch` or chmod changes a file's change
/// time (which can't be set from user space), so it is hashed again.
///
/// The app checks the model on every launch: hashing its 217 MB each time
/// cost a full read of the files and about a second of CPU on the build VM
/// at startup, even when the model then wasn't loaded (it loads lazily).
/// Non-unix targets always hash.
pub fn verify_cached(spec: &ModelSpec, dir: &Path) -> Vec<(&'static ModelFile, FileState)> {
    verify_cached_at(spec, dir, std::time::SystemTime::now())
}

fn verify_cached_at(
    spec: &ModelSpec,
    dir: &Path,
    now: std::time::SystemTime,
) -> Vec<(&'static ModelFile, FileState)> {
    let stamp_path = dir.join(STAMP_FILE);
    let before = stamp(spec, dir);
    if let Some(expected) = &before {
        if std::fs::read_to_string(&stamp_path).is_ok_and(|s| &s == expected) {
            return Vec::new();
        }
    }
    let bad = verify(spec, dir);
    // Stamp only what was hashed: the files didn't change while being read,
    // and are old enough for their times to be trusted.
    let settled = |t: Option<std::time::SystemTime>| {
        t.and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= STAMP_MIN_AGE)
    };
    let trusted = spec.files.iter().all(|f| {
        std::fs::metadata(spec.local_path(dir, f))
            .is_ok_and(|m| settled(m.modified().ok()) && settled(changed(&m)))
    });
    match before {
        Some(b) if bad.is_empty() && trusted && stamp(spec, dir).as_ref() == Some(&b) => {
            let tmp = dir.join(format!("{STAMP_FILE}.part"));
            if std::fs::write(&tmp, &b).is_ok() {
                let _ = std::fs::rename(&tmp, &stamp_path);
            }
        }
        _ => {
            let _ = std::fs::remove_file(&stamp_path);
        }
    }
    bad
}

/// The stamp text for the files as they are now (None if one is missing,
/// or on a platform without inodes and change times).
fn stamp(spec: &ModelSpec, dir: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut out = String::new();
        for f in spec.files {
            let m = std::fs::metadata(spec.local_path(dir, f)).ok()?;
            out.push_str(&format!(
                "{} {} {} {} {}.{} {}.{} {}\n",
                f.path,
                m.len(),
                m.dev(),
                m.ino(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
                f.sha256
            ));
        }
        Some(out)
    }
    #[cfg(not(unix))]
    {
        let _ = (spec, dir);
        None
    }
}

/// The inode change time.
fn changed(m: &std::fs::Metadata) -> Option<std::time::SystemTime> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let secs = u64::try_from(m.ctime()).ok()?;
        let nanos = u32::try_from(m.ctime_nsec()).ok()?;
        Some(std::time::UNIX_EPOCH + std::time::Duration::new(secs, nanos))
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        None
    }
}

/// Lowercase hex sha256 of a file, streamed.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Streaming sha256 for downloads (hash while writing).
#[derive(Default)]
pub struct HashingWriter {
    hasher: Sha256,
}

impl HashingWriter {
    pub fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
    }
    pub fn finish(self) -> String {
        hex(&self.hasher.finalize())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_pinned_and_consistent() {
        for spec in ALL {
            assert_eq!(spec.revision.len(), 40, "{}: full commit sha", spec.key);
            assert!(spec.files.iter().any(|f| f.path == spec.onnx_file));
            assert!(spec.files.iter().any(|f| f.path == spec.tokenizer_file));
            for f in spec.files {
                assert_eq!(f.sha256.len(), 64);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            }
            assert!(spec.dims > 0 && spec.max_tokens > 0);
            assert!(spec.model_id().contains(&spec.revision[..12]));
            assert!(spec.url(&spec.files[0]).starts_with("https://huggingface.co/"));
        }
    }

    #[test]
    fn verify_reports_missing_wrong_size_and_wrong_hash() {
        static FILES: [ModelFile; 2] = [
            ModelFile {
                path: "a.bin",
                size: 3,
                // sha256("abc")
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            },
            ModelFile { path: "b.bin", size: 3, sha256: "00" },
        ];
        static SPEC: ModelSpec = ModelSpec { files: &FILES, ..E5_SMALL_INT8 };
        let dir = std::env::temp_dir().join(format!("penguin-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = verify(&SPEC, &dir);
        assert_eq!(bad.len(), 2);
        assert!(bad.iter().all(|(_, s)| *s == FileState::Missing));
        std::fs::write(dir.join("a.bin"), b"abc").unwrap();
        std::fs::write(dir.join("b.bin"), b"abcd").unwrap();
        let bad = verify(&SPEC, &dir);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].1, FileState::WrongSize { expected: 3, actual: 4 });
        std::fs::write(dir.join("b.bin"), b"xyz").unwrap();
        let bad = verify(&SPEC, &dir);
        assert_eq!(bad[0].1, FileState::WrongHash);
        let mut w = HashingWriter::default();
        w.update(b"a");
        w.update(b"bc");
        assert_eq!(w.finish(), FILES[0].sha256);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn verify_cached_skips_unchanged_files_and_rehashes_changed_ones() {
        static FILES: [ModelFile; 1] = [ModelFile {
            path: "a.bin",
            size: 3,
            // sha256("abc")
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        }];
        static SPEC: ModelSpec = ModelSpec { files: &FILES, ..E5_SMALL_INT8 };
        let dir = std::env::temp_dir().join(format!("penguin-model-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stamp_path = dir.join(STAMP_FILE);
        let now = std::time::SystemTime::now;
        let later = || now() + std::time::Duration::from_secs(60);

        // Missing: reported, no stamp.
        assert_eq!(verify_cached_at(&SPEC, &dir, later())[0].1, FileState::Missing);
        assert!(!stamp_path.exists());

        // Just written: passes, but too fresh to stamp.
        std::fs::write(dir.join("a.bin"), b"abc").unwrap();
        assert!(verify_cached_at(&SPEC, &dir, now()).is_empty());
        assert!(!stamp_path.exists());

        // Settled: passes and is stamped; the stamp alone then passes it.
        assert!(verify_cached_at(&SPEC, &dir, later()).is_empty());
        let stamped = std::fs::read_to_string(&stamp_path).unwrap();
        assert!(stamped.starts_with("a.bin 3 "));
        assert!(verify_cached(&SPEC, &dir).is_empty());

        // Same size, other bytes: the change time moved, so the file is
        // hashed again, fails, and the stamp goes.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(dir.join("a.bin"), b"xyz").unwrap();
        assert_eq!(verify_cached_at(&SPEC, &dir, later())[0].1, FileState::WrongHash);
        assert!(!stamp_path.exists());

        // A stale stamp put back by hand doesn't match the file: still hashed.
        std::fs::write(&stamp_path, stamped).unwrap();
        assert_eq!(verify_cached_at(&SPEC, &dir, later())[0].1, FileState::WrongHash);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
