//! Runtime discovery (--runtime, settings, <exe dir>\runtime, Ollama install) and model discovery through the Ollama manifests.

use std::path::{Path, PathBuf};

use crate::model::{self, ModelFile, SizeRange};

pub const SERVER_EXE: &str = "llama-server.exe";
pub const SERVER_IMPL: &str = "libllama-server-impl.dll";

#[derive(Clone, Debug)]
pub struct Runtime {
    pub dir: PathBuf,
    pub exe: PathBuf,
    /// `vulkan\ggml-vulkan.dll` exists.
    pub vulkan: bool,
    /// `cuda_v12\` (preferred, wider driver support) or else `cuda_v13\`.
    pub cuda: Option<PathBuf>,
}

impl Runtime {
    pub fn vulkan_dir(&self) -> PathBuf {
        self.dir.join("vulkan")
    }
}

pub fn runtime_ok(dir: &Path) -> bool {
    dir.join(SERVER_EXE).is_file() && dir.join(SERVER_IMPL).is_file()
}

/// Search order: the configured dir (`--runtime`, else `settings.runtimeDir`), `<exe dir>\runtime`,
/// then `%LOCALAPPDATA%\Programs\Ollama\lib\ollama`.
pub fn runtime_candidates(configured: Option<&Path>, exe_dir: Option<&Path>, local_appdata: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = configured {
        if !p.as_os_str().is_empty() {
            v.push(p.to_path_buf());
        }
    }
    if let Some(d) = exe_dir {
        v.push(d.join("runtime"));
    }
    if let Some(l) = local_appdata {
        v.push(l.join("Programs").join("Ollama").join("lib").join("ollama"));
    }
    v
}

pub fn inspect_runtime(dir: &Path) -> Option<Runtime> {
    if !runtime_ok(dir) {
        return None;
    }
    let cuda = ["cuda_v12", "cuda_v13"].iter().map(|n| dir.join(n)).find(|d| d.is_dir());
    Some(Runtime {
        dir: dir.to_path_buf(),
        exe: dir.join(SERVER_EXE),
        vulkan: dir.join("vulkan").join("ggml-vulkan.dll").is_file(),
        cuda,
    })
}

pub fn find_runtime(candidates: &[PathBuf]) -> Option<Runtime> {
    candidates.iter().find_map(|d| inspect_runtime(d))
}

/// `%OLLAMA_MODELS%` when set, else `%USERPROFILE%\.ollama\models`.
pub fn ollama_models_dir() -> Option<PathBuf> {
    if let Some(m) = std::env::var_os("OLLAMA_MODELS").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(m));
    }
    std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join(".ollama").join("models"))
}

/// Walks `<models>\manifests` for a manifest whose model layer has a known digest, and returns
/// the matching blob when it passes the checks.
pub fn scan_manifests(models_dir: &Path, known: &[&str], size: SizeRange) -> Option<ModelFile> {
    let mut stack = vec![(models_dir.join("manifests"), 0u32)];
    let mut files = Vec::new();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() && depth < 8 => stack.push((p, depth + 1)),
                Ok(t) if t.is_file() => files.push(p),
                _ => {}
            }
        }
    }
    files.sort();
    for f in files {
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        if meta.len() > 64 * 1024 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let Some(layers) = v.get("layers").and_then(|l| l.as_array()) else { continue };
        for layer in layers {
            if layer.get("mediaType").and_then(|m| m.as_str()) != Some(model::MEDIA_MODEL) {
                continue;
            }
            let Some(digest) = layer.get("digest").and_then(|d| d.as_str()) else { continue };
            if !known.iter().any(|k| k.eq_ignore_ascii_case(digest)) {
                continue;
            }
            let blob = models_dir.join("blobs").join(digest.replacen(':', "-", 1));
            if let Ok(mut m) = model::check(&blob, Some(size)) {
                m.digest = Some(digest.to_ascii_lowercase());
                m.known = true;
                return Some(m);
            }
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub runtime: Option<Runtime>,
    pub model: Option<ModelFile>,
    /// An explicit runtime or model path was given but did not pass the checks.
    pub explicit_rejected: bool,
}

impl Discovery {
    pub fn installed(&self) -> bool {
        self.runtime.is_some() && self.model.is_some()
    }
}

/// Model search: the configured file (`--model`, else `settings.model`; unknown digests accepted,
/// GGUF magic required), else the Ollama manifests.
pub fn find_model(configured: Option<&Path>, models_dir: Option<&Path>) -> (Option<ModelFile>, bool) {
    let mut rejected = false;
    if let Some(p) = configured.filter(|p| !p.as_os_str().is_empty()) {
        match model::check(p, None) {
            Ok(m) => return (Some(m), false),
            Err(_) => rejected = true,
        }
    }
    let found = models_dir.and_then(|d| scan_manifests(d, model::KNOWN_DIGESTS, model::KNOWN_SIZE));
    (found, rejected)
}

/// Full discovery with the real search places.
pub fn discover(configured_runtime: Option<&Path>, configured_model: Option<&Path>) -> Discovery {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let cands = runtime_candidates(configured_runtime, exe_dir.as_deref(), local.as_deref());
    let runtime = find_runtime(&cands);
    let runtime_rejected = configured_runtime.is_some_and(|p| !p.as_os_str().is_empty() && !runtime_ok(p));
    let (model, model_rejected) = find_model(configured_model, ollama_models_dir().as_deref());
    Discovery { runtime, model, explicit_rejected: runtime_rejected || model_rejected }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const DIGEST: &str = "sha256:7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5";

    fn fixture_manifest() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/manifests/hf.co/Qwen/Qwen3-4B-GGUF/Q4_K_M")).unwrap()
    }

    fn write(p: &Path, b: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    const TINY: SizeRange = SizeRange { min: 4, max: 1000 };

    #[test]
    fn manifest_discovery() {
        let models = temp_dir("models");
        // An unrelated manifest first (alphabetically), with an unknown model digest.
        write(
            &models.join("manifests/aaa/other/latest"),
            br#"{"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","size":5}]}"#,
        );
        write(&models.join("manifests/hf.co/Qwen/Qwen3-4B-GGUF/Q4_K_M"), fixture_manifest().as_bytes());
        write(&models.join("manifests/broken/x"), b"{not json");
        // Blob missing: nothing found.
        assert!(scan_manifests(&models, model::KNOWN_DIGESTS, TINY).is_none());
        let blob = models.join(format!("blobs/{}", DIGEST.replace(':', "-")));
        write(&blob, b"GGUF0123456789");
        let m = scan_manifests(&models, model::KNOWN_DIGESTS, TINY).unwrap();
        assert_eq!(m.path, blob);
        assert_eq!(m.digest.as_deref(), Some(DIGEST));
        assert!(m.known);
        // The real size window rejects the tiny blob.
        assert!(scan_manifests(&models, model::KNOWN_DIGESTS, model::KNOWN_SIZE).is_none());
        // Not GGUF: rejected.
        write(&blob, b"XXXX0123456789");
        assert!(scan_manifests(&models, model::KNOWN_DIGESTS, TINY).is_none());
        // Unknown digest list: nothing.
        write(&blob, b"GGUF0123456789");
        assert!(scan_manifests(&models, &["sha256:00"], TINY).is_none());
        let _ = std::fs::remove_dir_all(&models);
    }

    #[test]
    fn configured_model_wins_and_unknown_digest_is_accepted() {
        let dir = temp_dir("cfgmodel");
        let f = dir.join("my-model.gguf");
        write(&f, b"GGUF....");
        let (m, rejected) = find_model(Some(&f), None);
        let m = m.unwrap();
        assert!(!rejected && !m.known && m.digest.is_none());
        let (m, rejected) = find_model(Some(&dir.join("missing.gguf")), None);
        assert!(m.is_none() && rejected);
        let (m, rejected) = find_model(Some(Path::new("")), None);
        assert!(m.is_none() && !rejected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_search_order() {
        let root = temp_dir("runtime");
        let a = root.join("a");
        let b = root.join("exe").join("runtime");
        let c = root.join("local").join("Programs/Ollama/lib/ollama");
        let cands = runtime_candidates(Some(&a), Some(&root.join("exe")), Some(&root.join("local")));
        assert_eq!(cands, vec![a.clone(), b.clone(), c.clone()]);
        assert!(find_runtime(&cands).is_none());
        // Only the exe is not enough.
        write(&c.join(SERVER_EXE), b"x");
        assert!(find_runtime(&cands).is_none());
        write(&c.join(SERVER_IMPL), b"x");
        let r = find_runtime(&cands).unwrap();
        assert_eq!(r.dir, c);
        assert!(!r.vulkan && r.cuda.is_none());
        write(&b.join(SERVER_EXE), b"x");
        write(&b.join(SERVER_IMPL), b"x");
        write(&b.join("vulkan/ggml-vulkan.dll"), b"x");
        std::fs::create_dir_all(b.join("cuda_v13")).unwrap();
        let r = find_runtime(&cands).unwrap();
        assert_eq!(r.dir, b);
        assert!(r.vulkan);
        assert_eq!(r.cuda.as_deref(), Some(b.join("cuda_v13").as_path()));
        std::fs::create_dir_all(b.join("cuda_v12")).unwrap();
        assert_eq!(find_runtime(&cands).unwrap().cuda.as_deref(), Some(b.join("cuda_v12").as_path()));
        // An empty configured path is ignored.
        assert_eq!(runtime_candidates(Some(Path::new("")), None, None).len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
