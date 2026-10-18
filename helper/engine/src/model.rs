//! Model file checks: GGUF magic, size, known digests.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Ollama layer digests of models this engine was evaluated with (Qwen3-4B Q4_K_M).
pub const KNOWN_DIGESTS: &[&str] = &["sha256:7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5"];
/// Ollama media type of the GGUF layer.
pub const MEDIA_MODEL: &str = "application/vnd.ollama.image.model";

/// Size window of a known model: 2.3 to 2.7 GB.
#[derive(Clone, Copy, Debug)]
pub struct SizeRange {
    pub min: u64,
    pub max: u64,
}

pub const KNOWN_SIZE: SizeRange = SizeRange { min: 2_300_000_000, max: 2_700_000_000 };

#[derive(Clone, Debug)]
pub struct ModelFile {
    pub path: PathBuf,
    pub size: u64,
    /// `sha256:<hex>` when known from the manifest or the blob file name.
    pub digest: Option<String>,
    /// The digest is in [`KNOWN_DIGESTS`].
    pub known: bool,
    pub mtime_secs: u64,
}

impl ModelFile {
    /// Identity used in the cache key: the digest, else size and modification time.
    pub fn identity(&self) -> String {
        match &self.digest {
            Some(d) => d.clone(),
            None => format!("file:{}:{}", self.size, self.mtime_secs),
        }
    }
}

pub fn is_known(digest: &str) -> bool {
    KNOWN_DIGESTS.iter().any(|k| k.eq_ignore_ascii_case(digest))
}

/// `...\blobs\sha256-<64 hex>` gives `sha256:<hex>`.
pub fn digest_from_blob_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let hex = name.strip_prefix("sha256-")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("sha256:{}", hex.to_ascii_lowercase()))
}

pub fn has_gguf_magic(path: &Path) -> bool {
    let mut b = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut b)).is_ok() && &b == b"GGUF"
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModelError {
    Missing,
    NotGguf,
    Size,
}

/// Checks a model file. `size` is enforced when given (manifest discovery of a known digest);
/// an explicit `--model` file only needs the GGUF magic.
pub fn check(path: &Path, size: Option<SizeRange>) -> Result<ModelFile, ModelError> {
    let meta = std::fs::metadata(path).map_err(|_| ModelError::Missing)?;
    if !meta.is_file() {
        return Err(ModelError::Missing);
    }
    if !has_gguf_magic(path) {
        return Err(ModelError::NotGguf);
    }
    if let Some(r) = size {
        if meta.len() < r.min || meta.len() > r.max {
            return Err(ModelError::Size);
        }
    }
    let digest = digest_from_blob_name(path);
    let known = digest.as_deref().is_some_and(is_known);
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(ModelFile { path: path.to_path_buf(), size: meta.len(), digest, known, mtime_secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_names() {
        let h = "7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5";
        let p = PathBuf::from(format!("x/blobs/sha256-{h}"));
        assert_eq!(digest_from_blob_name(&p).unwrap(), format!("sha256:{h}"));
        assert!(is_known(&format!("sha256:{h}")));
        assert!(digest_from_blob_name(Path::new("x/model.gguf")).is_none());
        assert!(digest_from_blob_name(Path::new("x/sha256-abc")).is_none());
    }

    #[test]
    fn magic_and_size() {
        let dir = crate::testutil::temp_dir("model");
        let good = dir.join("m.gguf");
        std::fs::write(&good, b"GGUF\x03\x00\x00\x00 rest").unwrap();
        let bad = dir.join("m.bin");
        std::fs::write(&bad, b"NOPE").unwrap();
        let m = check(&good, None).unwrap();
        assert_eq!(m.size, 13);
        assert!(!m.known && m.digest.is_none());
        assert!(m.identity().starts_with("file:13:"));
        assert_eq!(check(&bad, None).unwrap_err(), ModelError::NotGguf);
        assert_eq!(check(&dir.join("none"), None).unwrap_err(), ModelError::Missing);
        assert_eq!(check(&good, Some(SizeRange { min: 100, max: 200 })).unwrap_err(), ModelError::Size);
        assert!(check(&good, Some(SizeRange { min: 10, max: 20 })).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
