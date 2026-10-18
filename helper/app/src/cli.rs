//! Command line (design D.6, plus the test flags of amendment A1).
//!
//! ```text
//! TranslateProbeHelper.exe [--runtime DIR] [--model FILE] [--backend auto|cpu|vulkan|cuda] [--capture auto|wgc|gdi]
//!   [--translator llama|echo] [--wow-class CLASS] [--diag] [--diag-json FILE] [--log-dir DIR] [--no-tray]
//!   [--exit-after SECS] [--selftest]
//! test flags: [--no-overlay] [--assume-foreground] [--overlay-probe]
//! hidden test flag: [--instance-name NAME] (not in --help)
//! ```
//! Values are given as `--flag VALUE` or `--flag=VALUE`.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Auto,
    Cpu,
    Vulkan,
    Cuda,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        Some(match s {
            "auto" => Backend::Auto,
            "cpu" => Backend::Cpu,
            "vulkan" => Backend::Vulkan,
            "cuda" => Backend::Cuda,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Backend::Auto => "auto",
            Backend::Cpu => "cpu",
            Backend::Vulkan => "vulkan",
            Backend::Cuda => "cuda",
        }
    }

    pub fn to_engine(self) -> tp_engine::BackendPref {
        match self {
            Backend::Auto => tp_engine::BackendPref::Auto,
            Backend::Cpu => tp_engine::BackendPref::Cpu,
            Backend::Vulkan => tp_engine::BackendPref::Vulkan,
            Backend::Cuda => tp_engine::BackendPref::Cuda,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureChoice {
    Auto,
    Wgc,
    Gdi,
}

impl CaptureChoice {
    pub fn parse(s: &str) -> Option<CaptureChoice> {
        Some(match s {
            "auto" => CaptureChoice::Auto,
            "wgc" => CaptureChoice::Wgc,
            "gdi" => CaptureChoice::Gdi,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            CaptureChoice::Auto => "auto",
            CaptureChoice::Wgc => "wgc",
            CaptureChoice::Gdi => "gdi",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslatorChoice {
    Llama,
    Echo,
}

impl TranslatorChoice {
    pub fn name(self) -> &'static str {
        match self {
            TranslatorChoice::Llama => "llama",
            TranslatorChoice::Echo => "echo",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cli {
    pub runtime: Option<PathBuf>,
    pub model: Option<PathBuf>,
    /// None: use settings.backend.
    pub backend: Option<Backend>,
    /// None: use settings.capture.
    pub capture: Option<CaptureChoice>,
    pub translator: TranslatorChoice,
    /// Test window class (fakewow). None: the WoW class "waApplication Window".
    pub wow_class: Option<String>,
    pub diag: bool,
    pub diag_json: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
    pub no_tray: bool,
    pub exit_after: Option<f64>,
    pub selftest: bool,
    /// A1 test flag: cards are rendered into in-memory DIBs and reported in --diag-json, but no
    /// window is ever shown (overlay or toast). Implies --assume-foreground, and skips the
    /// WindowFromPoint covering test (the test window sits under other windows on purpose).
    pub no_overlay: bool,
    /// Test flag: treat the game window as the foreground window (the E2E window must never
    /// take focus, amendment A1), so capture runs at the foreground rate and decodes the strip.
    pub assume_foreground: bool,
    /// Test flag: show one synthetic card (small, bottom-right of the primary monitor) instead of
    /// capturing, and check click-through and focus. Use with --exit-after 5 (at most 5 s).
    pub overlay_probe: bool,
    /// Hidden test flag: the single-instance mutex becomes `Local\TranslateProbeHelper.NAME`, so a
    /// test Helper can run next to the user's own (1-32 ASCII letters, digits, '-' or '_').
    pub instance_name: Option<String>,
    pub help: bool,
    pub version: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            runtime: None,
            model: None,
            backend: None,
            capture: None,
            translator: TranslatorChoice::Llama,
            wow_class: None,
            diag: false,
            diag_json: None,
            log_dir: None,
            no_tray: false,
            exit_after: None,
            selftest: false,
            no_overlay: false,
            assume_foreground: false,
            overlay_probe: false,
            instance_name: None,
            help: false,
            version: false,
        }
    }
}

impl Cli {
    /// True when the game window is treated as foreground whatever Windows says.
    pub fn foreground_assumed(&self) -> bool {
        self.assume_foreground || self.no_overlay
    }
}

#[derive(Debug, PartialEq)]
pub enum CliError {
    Unknown(String),
    MissingValue(&'static str),
    BadValue(&'static str),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Unknown(a) => write!(f, "unknown argument: {a}"),
            CliError::MissingValue(n) => write!(f, "missing value for {n}"),
            CliError::BadValue(n) => write!(f, "bad value for {n}"),
        }
    }
}

pub const USAGE: &str = "TranslateProbeHelper.exe [--runtime DIR] [--model FILE] [--backend auto|cpu|vulkan|cuda]
  [--capture auto|wgc|gdi] [--translator llama|echo] [--wow-class CLASS] [--diag] [--diag-json FILE]
  [--log-dir DIR] [--no-tray] [--exit-after SECS] [--selftest]
Test flags: [--no-overlay] (never show a window, render into memory only; implies --assume-foreground
  and skips the covering test)
  [--assume-foreground] [--overlay-probe] (synthetic card, use with --exit-after 5)
";

const VALUE_FLAGS: &[&str] = &[
    "--runtime",
    "--model",
    "--backend",
    "--capture",
    "--translator",
    "--wow-class",
    "--diag-json",
    "--log-dir",
    "--exit-after",
    "--instance-name",
];

/// A valid `--instance-name`: 1-32 ASCII letters, digits, '-' or '_'.
pub fn valid_instance_name(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Parses the arguments after the program name.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Cli, CliError> {
    let mut cli = Cli::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let flag: &'static str = match VALUE_FLAGS.iter().find(|f| **f == name) {
            Some(f) => f,
            None => {
                if inline.is_some() {
                    return Err(CliError::Unknown(arg));
                }
                match name.as_str() {
                    "--diag" => cli.diag = true,
                    "--no-tray" => cli.no_tray = true,
                    "--selftest" => cli.selftest = true,
                    "--no-overlay" => cli.no_overlay = true,
                    "--assume-foreground" => cli.assume_foreground = true,
                    "--overlay-probe" => cli.overlay_probe = true,
                    "--help" | "-h" | "/?" => cli.help = true,
                    "--version" => cli.version = true,
                    _ => return Err(CliError::Unknown(arg)),
                }
                continue;
            }
        };
        let value = match inline {
            Some(v) => v,
            None => it.next().ok_or(CliError::MissingValue(flag))?,
        };
        if value.is_empty() {
            return Err(CliError::BadValue(flag));
        }
        match flag {
            "--runtime" => cli.runtime = Some(PathBuf::from(value)),
            "--model" => cli.model = Some(PathBuf::from(value)),
            "--backend" => cli.backend = Some(Backend::parse(&value).ok_or(CliError::BadValue(flag))?),
            "--capture" => cli.capture = Some(CaptureChoice::parse(&value).ok_or(CliError::BadValue(flag))?),
            "--translator" => {
                cli.translator = match value.as_str() {
                    "llama" => TranslatorChoice::Llama,
                    "echo" => TranslatorChoice::Echo,
                    _ => return Err(CliError::BadValue(flag)),
                }
            }
            "--wow-class" => cli.wow_class = Some(value),
            "--diag-json" => cli.diag_json = Some(PathBuf::from(value)),
            "--log-dir" => cli.log_dir = Some(PathBuf::from(value)),
            "--instance-name" => {
                if !valid_instance_name(&value) {
                    return Err(CliError::BadValue(flag));
                }
                cli.instance_name = Some(value);
            }
            "--exit-after" => {
                let secs: f64 = value.parse().map_err(|_| CliError::BadValue(flag))?;
                if !(secs.is_finite() && secs > 0.0 && secs <= 86_400.0) {
                    return Err(CliError::BadValue(flag));
                }
                cli.exit_after = Some(secs);
            }
            _ => unreachable!(),
        }
    }
    Ok(cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, CliError> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults() {
        let c = p(&[]).unwrap();
        assert_eq!(c, Cli::default());
        assert_eq!(c.translator, TranslatorChoice::Llama);
        assert!(!c.foreground_assumed());
    }

    #[test]
    fn every_flag() {
        let c = p(&[
            "--runtime", "C:\\rt", "--model=m.gguf", "--backend", "cpu", "--capture=gdi", "--translator", "echo",
            "--wow-class", "TPFakeWow", "--diag", "--diag-json", "d.jsonl", "--log-dir", "logs", "--no-tray",
            "--exit-after", "2.5", "--selftest", "--no-overlay", "--overlay-probe",
        ])
        .unwrap();
        assert_eq!(c.runtime, Some(PathBuf::from("C:\\rt")));
        assert_eq!(c.model, Some(PathBuf::from("m.gguf")));
        assert_eq!(c.backend, Some(Backend::Cpu));
        assert_eq!(c.capture, Some(CaptureChoice::Gdi));
        assert_eq!(c.translator, TranslatorChoice::Echo);
        assert_eq!(c.wow_class.as_deref(), Some("TPFakeWow"));
        assert!(c.diag && c.no_tray && c.selftest && c.no_overlay && c.overlay_probe);
        assert_eq!(c.diag_json, Some(PathBuf::from("d.jsonl")));
        assert_eq!(c.log_dir, Some(PathBuf::from("logs")));
        assert_eq!(c.exit_after, Some(2.5));
        assert!(c.foreground_assumed(), "--no-overlay implies the foreground assumption");
    }

    #[test]
    fn errors() {
        assert_eq!(p(&["--bogus"]), Err(CliError::Unknown("--bogus".into())));
        assert_eq!(p(&["--diag=1"]), Err(CliError::Unknown("--diag=1".into())));
        assert_eq!(p(&["--runtime"]), Err(CliError::MissingValue("--runtime")));
        assert_eq!(p(&["--backend", "gpu"]), Err(CliError::BadValue("--backend")));
        assert_eq!(p(&["--capture", "dda"]), Err(CliError::BadValue("--capture")));
        assert_eq!(p(&["--translator", "marian"]), Err(CliError::BadValue("--translator")));
        assert_eq!(p(&["--exit-after", "-1"]), Err(CliError::BadValue("--exit-after")));
        assert_eq!(p(&["--exit-after", "soon"]), Err(CliError::BadValue("--exit-after")));
        assert_eq!(p(&["--model="]), Err(CliError::BadValue("--model")));
        assert_eq!(p(&["positional"]), Err(CliError::Unknown("positional".into())));
    }

    #[test]
    fn names_round_trip() {
        for b in [Backend::Auto, Backend::Cpu, Backend::Vulkan, Backend::Cuda] {
            assert_eq!(Backend::parse(b.name()), Some(b));
        }
        for c in [CaptureChoice::Auto, CaptureChoice::Wgc, CaptureChoice::Gdi] {
            assert_eq!(CaptureChoice::parse(c.name()), Some(c));
        }
        assert_eq!(Backend::Vulkan.to_engine(), tp_engine::BackendPref::Vulkan);
    }

    #[test]
    fn instance_name_is_hidden_and_validated() {
        let c = p(&["--instance-name", "exit-test_1"]).unwrap();
        assert_eq!(c.instance_name.as_deref(), Some("exit-test_1"));
        assert_eq!(p(&["--instance-name=a"]).unwrap().instance_name.as_deref(), Some("a"));
        let long = "n".repeat(33);
        for bad in ["a b", "x\\y", "Local\\x", "\u{e9}", long.as_str()] {
            assert_eq!(p(&["--instance-name", bad]), Err(CliError::BadValue("--instance-name")), "{bad}");
        }
        assert!(!USAGE.contains("--instance-name"), "hidden from --help");
        assert_eq!(Cli::default().instance_name, None);
    }

    #[test]
    fn assume_foreground_alone() {
        let c = p(&["--assume-foreground"]).unwrap();
        assert!(c.assume_foreground && !c.no_overlay && c.foreground_assumed());
    }
}
