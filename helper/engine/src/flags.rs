//! llama-server --help probe, cached per (path, size, mtime) in run\flags.json (feature names only).
//!
//! Builds differ: 0.7.0-dev rejects `--no-mmap` (replaced by `-lm`), older builds may lack
//! `--fit`, `--api-key-file` or `--no-ui`. The server command line only uses what `--help` lists.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

/// Flags the engine may pass, as written in `--help` (first spelling).
pub const KNOWN: &[&str] = &[
    "-m", "--host", "--port", "-c", "-np", "-t", "-tb", "-ngl", "-lm", "--no-mmap", "--fit", "--api-key-file", "--api-key",
    "--no-ui", "--no-webui", "--offline", "-dev", "--list-devices",
];

/// Alternative spellings mapped to the [`KNOWN`] name.
const ALIASES: &[(&str, &str)] = &[
    ("--model", "-m"),
    ("--ctx-size", "-c"),
    ("--parallel", "-np"),
    ("--threads", "-t"),
    ("--threads-batch", "-tb"),
    ("--gpu-layers", "-ngl"),
    ("--n-gpu-layers", "-ngl"),
    ("--load-mode", "-lm"),
    ("-fit", "--fit"),
    ("--device", "-dev"),
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Features {
    pub names: BTreeSet<String>,
}

impl Features {
    pub fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// `-m`, `--port` and `-c` are required.
    pub fn usable(&self) -> Result<(), &'static str> {
        for (n, why) in [("-m", "no -m"), ("--port", "no --port"), ("-c", "no -c")] {
            if !self.has(n) {
                return Err(why);
            }
        }
        Ok(())
    }

    pub fn lm_none(&self) -> bool {
        self.has("-lm")
    }

    pub fn no_ui_flag(&self) -> Option<&'static str> {
        if self.has("--no-ui") {
            Some("--no-ui")
        } else if self.has("--no-webui") {
            Some("--no-webui")
        } else {
            None
        }
    }
}

/// Every flag name in the first column of `llama-server --help`.
pub fn parse_help(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        if !line.starts_with('-') {
            continue;
        }
        // The flag column ends at the first run of 2+ spaces not preceded by a comma
        // ("-h,    --help, --usage          print usage").
        let b = line.as_bytes();
        let mut end = b.len();
        let mut i = 1;
        while i + 1 < b.len() {
            if b[i] == b' ' && b[i + 1] == b' ' && b[i - 1] != b',' && b[i - 1] != b' ' {
                end = i;
                break;
            }
            i += 1;
        }
        for tok in line[..end].split([' ', ',']) {
            let t = tok.trim();
            let mut cs = t.chars();
            let ok = cs.next() == Some('-') && {
                let rest: String = cs.collect();
                let rest = rest.trim_start_matches('-');
                rest.starts_with(|c: char| c.is_ascii_alphabetic())
                    && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            };
            if ok && t.len() <= 40 {
                out.insert(t.to_string());
            }
        }
    }
    out
}

/// The features the engine cares about, from a parsed name set.
pub fn features(names: &BTreeSet<String>) -> Features {
    let mut f = BTreeSet::new();
    for k in KNOWN {
        if names.contains(*k) {
            f.insert(k.to_string());
        }
    }
    for (alias, k) in ALIASES {
        if names.contains(*alias) {
            f.insert(k.to_string());
        }
    }
    Features { names: f }
}

/// Cache identity of an exe: FNV-1a 64 of (path, size, mtime), so flags.json holds no path.
pub fn exe_id(exe: &Path) -> Option<String> {
    let meta = std::fs::metadata(exe).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let mut b = exe.to_string_lossy().to_lowercase().into_bytes();
    b.extend(meta.len().to_le_bytes());
    b.extend(mtime.to_le_bytes());
    Some(format!("{:016x}", crate::cache::fnv1a64(&b)))
}

fn read_cache(file: &Path, id: &str) -> Option<Features> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    if v.get("version")?.as_u64()? != 1 || v.get("id")?.as_str()? != id {
        return None;
    }
    let names = v.get("features")?.as_array()?.iter().filter_map(|n| n.as_str()).filter(|n| KNOWN.contains(n)).map(String::from).collect();
    Some(Features { names })
}

fn write_cache(file: &Path, id: &str, f: &Features) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let v = serde_json::json!({"version": 1, "id": id, "features": f.names.iter().collect::<Vec<_>>()});
    let _ = std::fs::write(file, v.to_string());
}

/// Probes `llama-server.exe --help` once per (path, size, mtime); the result is cached in
/// `<data_dir>\run\flags.json`. Returns the features and whether they came from the cache.
pub fn probe(exe: &Path, cwd: &Path, data_dir: &Path) -> Result<(Features, bool), &'static str> {
    let id = exe_id(exe).ok_or("server exe missing")?;
    let file = data_dir.join("run").join("flags.json");
    if let Some(f) = read_cache(&file, &id) {
        return Ok((f, true));
    }
    let (text, _code) = crate::server::run_capture(exe, &["--help"], cwd, Duration::from_secs(15))?;
    let f = features(&parse_help(&text));
    f.usable()?;
    write_cache(&file, &id, &f);
    Ok((f, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP_070: &str = include_str!("../tests/fixtures/help_0_7_0.txt");
    const HELP_030: &str = include_str!("../tests/fixtures/help_0_3_0.txt");

    #[test]
    fn parse_lines() {
        let n = parse_help("-h,    --help, --usage                  print usage and exit\n\
             -fit,  --fit [on|off]                   whether to adjust\n\
             --ui,  --webui, --no-ui, --no-webui     whether to enable\n\
             -dev,  --device <dev1,dev2,..>          comma-separated -x list\n\
             -fitt, --fit-target MiB0,MiB1,MiB2,...\n\
             \x20                                       use -bogus here\n\
             ----- common params -----\n");
        for f in ["-h", "--help", "--usage", "-fit", "--fit", "--ui", "--webui", "--no-ui", "--no-webui", "-dev", "--device", "-fitt", "--fit-target"] {
            assert!(n.contains(f), "{f}");
        }
        assert!(!n.contains("-x") && !n.contains("-bogus") && !n.contains("-----"));
    }

    #[test]
    fn fixture_0_7_0() {
        let f = features(&parse_help(HELP_070));
        assert!(f.usable().is_ok());
        for k in ["-m", "--host", "--port", "-c", "-np", "-t", "-tb", "-ngl", "-lm", "--fit", "--api-key-file", "--api-key", "--no-ui", "--offline", "-dev", "--list-devices"] {
            assert!(f.has(k), "{k}");
        }
        // 0.7.0-dev rejects --no-mmap (measured): it must not be detected.
        assert!(!f.has("--no-mmap"));
        assert!(f.lm_none());
        assert_eq!(f.no_ui_flag(), Some("--no-ui"));
    }

    #[test]
    fn fixture_0_3_0() {
        let f = features(&parse_help(HELP_030));
        assert!(f.usable().is_ok());
        for k in ["-m", "--port", "-c", "-lm", "--fit", "--api-key-file", "--api-key", "--no-ui", "--offline", "-dev", "--no-mmap"] {
            assert!(f.has(k), "{k}");
        }
        assert!(f.lm_none(), "-lm wins over the deprecated --no-mmap");
    }

    #[test]
    fn missing_required_flags() {
        let f = features(&parse_help("-m,    --model FNAME    model\n--port PORT    port\n"));
        assert_eq!(f.usable(), Err("no -c"));
        let f = features(&parse_help("--ctx-size N    ctx\n--port PORT    port\n"));
        assert_eq!(f.usable(), Err("no -m"));
        assert!(features(&parse_help("")).usable().is_err());
    }

    #[test]
    fn cache_round_trip_holds_names_only() {
        let dir = crate::testutil::temp_dir("flags");
        let exe = dir.join("llama-server.exe");
        std::fs::write(&exe, b"stub").unwrap();
        let id = exe_id(&exe).unwrap();
        let f = features(&parse_help(HELP_070));
        let file = dir.join("run").join("flags.json");
        write_cache(&file, &id, &f);
        assert_eq!(read_cache(&file, &id), Some(f.clone()));
        assert_eq!(read_cache(&file, "other"), None);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(":\\") && !text.contains("llama-server"), "{text}");
        // probe() serves from the cache without running anything (the stub is not an exe).
        let (g, cached) = probe(&exe, &dir, &dir).unwrap();
        assert!(cached);
        assert_eq!(g, f);
        // A changed exe (size) invalidates the cache: the stub cannot run, so the probe fails.
        std::fs::write(&exe, b"stub2").unwrap();
        assert!(probe(&exe, &dir, &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
