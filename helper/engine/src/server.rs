//! llama-server process: hidden, below-normal priority, kill-on-close job object, API key file, stderr drain into an allowlisted ring.
//!
//! The process is created suspended, put in a job object whose last handle closing kills every
//! process in it, then resumed: the server can never outlive the helper, even after a crash or a
//! forced kill. stdout and stderr share one pipe, drained continuously by a thread.

use std::collections::VecDeque;
use std::ffi::{c_void, OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
};
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList,
    ResumeThread, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, BELOW_NORMAL_PRIORITY_CLASS,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES,
    STARTUPINFOEXW,
};

use crate::events::ServerLine;

/// Ring of allowlisted server lines.
pub const RING_LINES: usize = 50;

/// A Win32 handle we own and may use from any thread.
struct Owned(HANDLE);
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Job object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
pub struct Job(Owned);

impl Job {
    pub fn new() -> std::io::Result<Job> {
        unsafe {
            let h = CreateJobObjectW(None, PCWSTR::null()).map_err(std::io::Error::other)?;
            let job = Job(Owned(h));
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of_val(&info) as u32,
            )
            .map_err(std::io::Error::other)?;
            Ok(job)
        }
    }

    /// Kills every process in the job.
    pub fn terminate(&self) {
        unsafe {
            let _ = TerminateJobObject(self.0 .0, 1);
        }
    }
}

/// A process started by [`spawn_in_job`].
pub struct Child {
    process: Owned,
    pub pid: u32,
    job: Arc<Job>,
}

impl Child {
    /// Exit code once the process has exited.
    pub fn try_wait(&self) -> Option<u32> {
        self.wait_timeout(Duration::ZERO)
    }

    pub fn wait_timeout(&self, d: Duration) -> Option<u32> {
        unsafe {
            if WaitForSingleObject(self.process.0, d.as_millis().min(u32::MAX as u128 - 1) as u32) != WAIT_OBJECT_0 {
                return None;
            }
            let mut code = 0u32;
            GetExitCodeProcess(self.process.0, &mut code).ok()?;
            Some(code)
        }
    }

    /// Kills the whole job and waits up to 2 s for the process to end.
    pub fn kill(&self) {
        self.job.terminate();
        unsafe {
            let _ = TerminateProcess(self.process.0, 1);
        }
        let _ = self.wait_timeout(Duration::from_secs(2));
    }

    pub fn killer(&self) -> Arc<dyn Fn() + Send + Sync> {
        let job = self.job.clone();
        Arc::new(move || job.terminate())
    }
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Quotes one argument for CommandLineToArgvW / the MSVC runtime.
pub fn quote_arg(arg: &OsStr) -> Vec<u16> {
    let a: Vec<u16> = arg.encode_wide().collect();
    let needs = a.is_empty() || a.iter().any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
    if !needs {
        return a;
    }
    let mut out = vec![b'"' as u16];
    let mut backslashes = 0usize;
    for &c in &a {
        if c == b'\\' as u16 {
            backslashes += 1;
        } else if c == b'"' as u16 {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
            out.push(c);
            backslashes = 0;
        } else {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            out.push(c);
            backslashes = 0;
        }
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    out.push(b'"' as u16);
    out
}

fn command_line(exe: &Path, args: &[OsString]) -> Vec<u16> {
    let mut cl = quote_arg(exe.as_os_str());
    for a in args {
        cl.push(b' ' as u16);
        cl.extend(quote_arg(a));
    }
    cl.push(0);
    cl
}

/// The current environment with overrides, as a sorted UTF-16 block.
fn env_block(overrides: &[(String, String)]) -> Vec<u16> {
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !overrides.iter().any(|(o, _)| OsStr::new(o).eq_ignore_ascii_case(k)))
        .collect();
    for (k, v) in overrides {
        vars.push((k.into(), v.into()));
    }
    vars.sort_by_key(|(k, _)| k.to_ascii_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(k.encode_wide());
        block.push(b'=' as u16);
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn inheritable(h: &impl AsRawHandle) -> std::io::Result<HANDLE> {
    let raw = HANDLE(h.as_raw_handle());
    unsafe { SetHandleInformation(raw, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT).map_err(std::io::Error::other)? };
    Ok(raw)
}

/// Starts `exe` hidden, suspended, at below-normal priority; puts it in a new kill-on-close job;
/// then resumes it. stdin is NUL; stdout and stderr go to the returned pipe. Only the two std
/// handles are inherited (PROC_THREAD_ATTRIBUTE_HANDLE_LIST).
pub fn spawn_in_job(exe: &Path, args: &[OsString], cwd: &Path, env: &[(String, String)]) -> std::io::Result<(Child, std::io::PipeReader)> {
    let job = Arc::new(Job::new()?);
    let (reader, writer) = std::io::pipe()?;
    let writer = OwnedHandle::from(writer);
    let nul = OwnedHandle::from(std::fs::File::open("NUL")?);
    let h_out = inheritable(&writer)?;
    let h_in = inheritable(&nul)?;
    let handles = [h_in, h_out];

    let mut cmd = command_line(exe, args);
    let app = wide(exe.as_os_str());
    let dir = wide(cwd.as_os_str());
    let env_w = (!env.is_empty()).then(|| env_block(env));

    unsafe {
        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        let mut attr_buf = vec![0u8; size.max(64)];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut c_void);
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size).map_err(std::io::Error::other)?;
        let res = (|| -> std::io::Result<PROCESS_INFORMATION> {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr() as *const c_void),
                std::mem::size_of_val(&handles),
                None,
                None,
            )
            .map_err(std::io::Error::other)?;
            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = h_in;
            si.StartupInfo.hStdOutput = h_out;
            si.StartupInfo.hStdError = h_out;
            si.lpAttributeList = list;
            let mut flags = CREATE_NO_WINDOW | CREATE_SUSPENDED | BELOW_NORMAL_PRIORITY_CLASS | EXTENDED_STARTUPINFO_PRESENT;
            if env_w.is_some() {
                flags |= CREATE_UNICODE_ENVIRONMENT;
            }
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessW(
                PCWSTR(app.as_ptr()),
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                true,
                flags,
                env_w.as_ref().map(|e| e.as_ptr() as *const c_void),
                PCWSTR(dir.as_ptr()),
                &si.StartupInfo,
                &mut pi,
            )
            .map_err(std::io::Error::other)?;
            Ok(pi)
        })();
        DeleteProcThreadAttributeList(list);
        let pi = res?;
        let thread = Owned(pi.hThread);
        let process = Owned(pi.hProcess);
        if let Err(e) = AssignProcessToJobObject(job.0 .0, process.0) {
            let _ = TerminateProcess(process.0, 1);
            return Err(std::io::Error::other(e));
        }
        ResumeThread(thread.0);
        drop(thread);
        // Our copies of the child's ends: closing them lets the reader see EOF at child exit.
        drop(writer);
        drop(nul);
        Ok((Child { process, pid: pi.dwProcessId, job }, reader))
    }
}

/// Runs a short command (`--help`, `--list-devices`) and returns its combined output (at most
/// 512 KB) and exit code. Killed after `timeout`.
pub fn run_capture(exe: &Path, args: &[&str], cwd: &Path, timeout: Duration) -> Result<(String, Option<u32>), &'static str> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let (child, reader) = spawn_in_job(exe, &args, cwd, &[]).map_err(|_| "spawn failed")?;
    let t = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.take(512 * 1024).read_to_end(&mut buf);
        buf
    });
    let code = child.wait_timeout(timeout);
    if code.is_none() {
        child.kill();
    }
    let out = t.join().unwrap_or_default();
    Ok((String::from_utf8_lossy(&out).into_owned(), code))
}

/// 32 hex characters from the system RNG.
pub fn new_api_key() -> String {
    let mut b = [0u8; 16];
    let st = unsafe { BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if st.0 != 0 {
        // Never expected. std seeds RandomState from the OS RNG, so this is still unpredictable.
        use std::hash::BuildHasher;
        let s = std::collections::hash_map::RandomState::new();
        let hi = s.hash_one(std::process::id());
        let lo = s.hash_one(Instant::now());
        b = ((hi as u128) << 64 | lo as u128).to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A free TCP port on 127.0.0.1 (bind to port 0, read it, close).
pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// What the worker asks a spawner to start.
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub exe: PathBuf,
    pub cwd: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    pub port: u16,
    /// Deleted when the server exits or is killed.
    pub key_file: Option<PathBuf>,
}

/// Context size, slots and the "all layers" value used for full offload.
pub const CTX: u32 = 1024;
pub const GPU_LAYERS_ALL: u32 = 999;

/// The server command line (design D.4 "Start"): only flags listed by `--help` are passed.
/// CPU: cwd = runtime root, `CUDA_VISIBLE_DEVICES=-1`, `-ngl 0`. Vulkan: cwd = `vulkan\`,
/// `-ngl 999`, `-dev VulkanN`. CUDA: cwd = the cuda folder. The working directory selects the
/// ggml GPU backend and lets Windows find its DLLs (measured).
#[allow(clippy::too_many_arguments)]
pub fn launch_spec(
    rt: &crate::discover::Runtime,
    model: &Path,
    f: &crate::flags::Features,
    choice: crate::policy::Choice,
    vulkan_dev: Option<&str>,
    port: u16,
    key: &str,
    key_file: Option<&Path>,
) -> Option<LaunchSpec> {
    use crate::policy::Choice;
    let (threads, ngl, cwd, gpu) = match choice {
        Choice::Cpu { threads } => (threads, 0, rt.dir.clone(), false),
        Choice::Vulkan { threads } => (threads, GPU_LAYERS_ALL, rt.vulkan_dir(), true),
        Choice::Cuda { threads } => (threads, GPU_LAYERS_ALL, rt.cuda.clone()?, true),
        Choice::LowMemory => return None,
    };
    let mut a: Vec<String> = vec!["-m".into(), model.to_string_lossy().into_owned(), "--host".into(), "127.0.0.1".into(), "--port".into(), port.to_string()];
    let mut key_file_used = None;
    match key_file {
        Some(kf) if f.has("--api-key-file") => {
            a.extend(["--api-key-file".into(), kf.to_string_lossy().into_owned()]);
            key_file_used = Some(kf.to_path_buf());
        }
        _ if f.has("--api-key") => a.extend(["--api-key".into(), key.to_string()]),
        _ => {}
    }
    a.extend(["-c".into(), CTX.to_string()]);
    if f.has("-np") {
        a.extend(["-np".into(), "1".into()]);
    }
    if f.has("-t") {
        a.extend(["-t".into(), threads.to_string()]);
    }
    if f.has("-tb") {
        a.extend(["-tb".into(), threads.to_string()]);
    }
    if f.has("-ngl") {
        a.extend(["-ngl".into(), ngl.to_string()]);
    }
    if let (true, Some(dev), true) = (matches!(choice, Choice::Vulkan { .. }), vulkan_dev, f.has("-dev")) {
        a.extend(["-dev".into(), dev.to_string()]);
    }
    if f.has("--fit") {
        a.extend(["--fit".into(), "off".into()]);
    }
    if f.lm_none() {
        a.extend(["-lm".into(), "none".into()]);
    } else if f.has("--no-mmap") {
        a.push("--no-mmap".into());
    }
    if let Some(no_ui) = f.no_ui_flag() {
        a.push(no_ui.into());
    }
    if f.has("--offline") {
        a.push("--offline".into());
    }
    let env = if gpu { vec![] } else { vec![("CUDA_VISIBLE_DEVICES".to_string(), "-1".to_string())] };
    Some(LaunchSpec { exe: rt.exe.clone(), cwd, args: a.into_iter().map(OsString::from).collect(), env, port, key_file: key_file_used })
}

/// A running llama-server (real or fake).
pub trait ServerProc: Send {
    fn port(&self) -> u16;
    /// Exit code once the process has exited.
    fn exit_code(&mut self) -> Option<i32>;
    /// Kills it (idempotent) and waits up to 2 s.
    fn kill(&mut self);
    /// Kills it from any thread, without waiting (used by shutdown).
    fn killer(&self) -> Arc<dyn Fn() + Send + Sync>;
    /// Allowlisted output lines so far.
    fn ring(&self) -> Vec<ServerLine>;
}

pub trait Spawner: Send {
    fn spawn(&mut self, spec: &LaunchSpec) -> Result<Box<dyn ServerProc>, &'static str>;
}

pub struct RealSpawner;

impl Spawner for RealSpawner {
    fn spawn(&mut self, spec: &LaunchSpec) -> Result<Box<dyn ServerProc>, &'static str> {
        let (child, reader) = spawn_in_job(&spec.exe, &spec.args, &spec.cwd, &spec.env).map_err(|_| "spawn failed")?;
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let r2 = ring.clone();
        let drain = std::thread::Builder::new()
            .name("tp-llama-drain".into())
            .spawn(move || drain_lines(reader, r2))
            .ok();
        Ok(Box::new(LlamaProc { child, port: spec.port, key_file: spec.key_file.clone(), ring, drain, exited: None }))
    }
}

fn drain_lines(reader: impl Read, ring: Arc<Mutex<VecDeque<ServerLine>>>) {
    let mut r = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if let Some(line) = ServerLine::filter(&String::from_utf8_lossy(&buf)) {
                    let mut g = ring.lock().unwrap_or_else(|e| e.into_inner());
                    if g.len() >= RING_LINES {
                        g.pop_front();
                    }
                    g.push_back(line);
                }
            }
        }
    }
}

struct LlamaProc {
    child: Child,
    port: u16,
    key_file: Option<PathBuf>,
    ring: Arc<Mutex<VecDeque<ServerLine>>>,
    drain: Option<JoinHandle<()>>,
    exited: Option<i32>,
}

impl LlamaProc {
    fn cleanup(&mut self) {
        if let Some(k) = self.key_file.take() {
            let _ = std::fs::remove_file(k);
        }
        if let Some(d) = self.drain.take() {
            // The pipe closes with the process; never block shutdown on it.
            if d.is_finished() {
                let _ = d.join();
            }
        }
    }
}

impl ServerProc for LlamaProc {
    fn port(&self) -> u16 {
        self.port
    }

    fn exit_code(&mut self) -> Option<i32> {
        if self.exited.is_none() {
            self.exited = self.child.try_wait().map(|c| c as i32);
            if self.exited.is_some() {
                self.cleanup();
            }
        }
        self.exited
    }

    fn kill(&mut self) {
        self.child.kill();
        self.exited.get_or_insert(-1);
        self.cleanup();
    }

    fn killer(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.child.killer()
    }

    fn ring(&self) -> Vec<ServerLine> {
        let g = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().cloned().collect()
    }
}

impl Drop for LlamaProc {
    fn drop(&mut self) {
        if self.exited.is_none() {
            self.child.kill();
        }
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> String {
        String::from_utf16(&quote_arg(OsStr::new(s))).unwrap()
    }

    #[test]
    fn argument_quoting() {
        assert_eq!(q("abc"), "abc");
        assert_eq!(q("a b"), "\"a b\"");
        assert_eq!(q(""), "\"\"");
        assert_eq!(q(r"C:\Program Files\x\"), r#""C:\Program Files\x\\""#);
        assert_eq!(q(r#"a"b"#), r#""a\"b""#);
        assert_eq!(q(r#"a\"b c"#), r#""a\\\"b c""#);
        assert_eq!(q(r"C:\no\space"), r"C:\no\space");
    }

    fn features(help: &str) -> crate::flags::Features {
        crate::flags::features(&crate::flags::parse_help(help))
    }

    fn rt() -> crate::discover::Runtime {
        crate::discover::Runtime {
            dir: PathBuf::from(r"R:\rt"),
            exe: PathBuf::from(r"R:\rt\llama-server.exe"),
            vulkan: true,
            cuda: Some(PathBuf::from(r"R:\rt\cuda_v12")),
        }
    }

    fn sargs(s: &LaunchSpec) -> Vec<String> {
        s.args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn launch_specs() {
        use crate::policy::Choice;
        let f070 = features(include_str!("../tests/fixtures/help_0_7_0.txt"));
        let model = Path::new(r"M:\m.gguf");
        let kf = Path::new(r"D:\run\key-1.txt");
        let cpu = launch_spec(&rt(), model, &f070, Choice::Cpu { threads: 3 }, None, 5000, "k", Some(kf)).unwrap();
        assert_eq!(
            sargs(&cpu).join(" "),
            r"-m M:\m.gguf --host 127.0.0.1 --port 5000 --api-key-file D:\run\key-1.txt -c 1024 -np 1 -t 3 -tb 3 -ngl 0 --fit off -lm none --no-ui --offline"
        );
        assert_eq!(cpu.cwd, PathBuf::from(r"R:\rt"));
        assert_eq!(cpu.env, vec![("CUDA_VISIBLE_DEVICES".to_string(), "-1".to_string())]);
        assert_eq!(cpu.key_file.as_deref(), Some(kf));
        let vk = launch_spec(&rt(), model, &f070, Choice::Vulkan { threads: 4 }, Some("Vulkan0"), 5000, "k", Some(kf)).unwrap();
        let a = sargs(&vk).join(" ");
        assert!(a.contains("-ngl 999 -dev Vulkan0 --fit off"), "{a}");
        assert_eq!(vk.cwd, PathBuf::from(r"R:\rt\vulkan"));
        assert!(vk.env.is_empty());
        let cu = launch_spec(&rt(), model, &f070, Choice::Cuda { threads: 4 }, Some("Vulkan0"), 5000, "k", Some(kf)).unwrap();
        assert_eq!(cu.cwd, PathBuf::from(r"R:\rt\cuda_v12"));
        assert!(!sargs(&cu).contains(&"-dev".to_string()), "-dev is for Vulkan only");
        assert!(launch_spec(&rt(), model, &f070, Choice::LowMemory, None, 5000, "k", Some(kf)).is_none());
        // An old build without -lm, --fit, --api-key-file or --no-ui.
        let old = features("-m,    --model F   x\n--port P   x\n-c,    --ctx-size N   x\n--no-mmap   x\n--api-key K   x\n-ngl N   x\n");
        let o = launch_spec(&rt(), model, &old, Choice::Cpu { threads: 2 }, None, 5000, "key32", Some(kf)).unwrap();
        assert_eq!(sargs(&o).join(" "), r"-m M:\m.gguf --host 127.0.0.1 --port 5000 --api-key key32 -c 1024 -ngl 0 --no-mmap");
        assert!(o.key_file.is_none(), "key passed inline, no key file to delete");
        let f030 = features(include_str!("../tests/fixtures/help_0_3_0.txt"));
        let a = sargs(&launch_spec(&rt(), model, &f030, Choice::Cpu { threads: 4 }, None, 1, "k", Some(kf)).unwrap()).join(" ");
        assert!(a.contains("-lm none") && !a.contains("--no-mmap") && a.contains("--fit off") && a.contains("--no-ui"), "{a}");
    }

    #[test]
    fn api_key_and_port() {
        let a = new_api_key();
        let b = new_api_key();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(free_port().unwrap() > 0);
    }

    #[test]
    fn env_block_overrides() {
        let b = env_block(&[("CUDA_VISIBLE_DEVICES".into(), "-1".into())]);
        let s = String::from_utf16_lossy(&b);
        assert!(s.contains("CUDA_VISIBLE_DEVICES=-1\0"));
        assert!(s.ends_with("\0\0"));
        assert_eq!(s.matches("CUDA_VISIBLE_DEVICES=").count(), 1);
    }

    /// Captures a short command's output through the job/pipe path (cmd's echo builtin).
    #[test]
    fn capture_output_of_child() {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let cmd = sys.join("System32").join("cmd.exe");
        let (out, code) = run_capture(&cmd, &["/d", "/c", "echo tp-engine-check & echo error: second 1>&2"], &sys, Duration::from_secs(10)).unwrap();
        assert_eq!(code, Some(0));
        assert!(out.contains("tp-engine-check"), "{out}");
        assert!(out.contains("error: second"), "stderr shares the pipe: {out}");
    }

    #[test]
    fn spawner_ring_and_key_file() {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let dir = crate::testutil::temp_dir("ring");
        let key = dir.join("key-test.txt");
        std::fs::write(&key, "k").unwrap();
        let spec = LaunchSpec {
            exe: sys.join("System32").join("cmd.exe"),
            cwd: sys.clone(),
            args: ["/d", "/c", "echo load_backend: ok & echo slot prompt text & echo model loaded"].iter().map(OsString::from).collect(),
            env: vec![("CUDA_VISIBLE_DEVICES".into(), "-1".into())],
            port: 1,
            key_file: Some(key.clone()),
        };
        let mut p = RealSpawner.spawn(&spec).unwrap();
        let t = Instant::now();
        while p.exit_code().is_none() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(p.exit_code(), Some(0));
        std::thread::sleep(Duration::from_millis(200));
        let ring: Vec<String> = p.ring().iter().map(|l| l.as_str().to_string()).collect();
        assert_eq!(ring, vec!["load_backend: ok".to_string(), "model loaded".to_string()]);
        assert!(!key.exists(), "key file deleted at exit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Design P3: a child helper process starts `ping -n 30 127.0.0.1` in the job, then exits
    /// with `process::exit`; ping must be gone within 2 s.
    #[test]
    fn job_kills_grandchild_when_parent_exits() {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", "server::tests::job_child_entry", "--ignored", "--nocapture", "--test-threads=1"])
            .env("TP_JOB_CHILD", "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let pid: u32 = text
            .lines()
            .find_map(|l| l.split("PING_PID=").nth(1))
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .unwrap_or_else(|| panic!("no pid in child output: {text}"));
        let t = Instant::now();
        let gone = unsafe {
            match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                Err(_) => true, // already gone
                Ok(h) => {
                    let h = Owned(h);
                    WaitForSingleObject(h.0, 2000) == WAIT_OBJECT_0
                }
            }
        };
        assert!(gone, "ping {pid} still alive after {:?}", t.elapsed());
    }

    #[test]
    #[ignore = "entry point for job_kills_grandchild_when_parent_exits"]
    fn job_child_entry() {
        if std::env::var_os("TP_JOB_CHILD").is_none() {
            return;
        }
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let ping = sys.join("System32").join("PING.EXE");
        let args: Vec<OsString> = ["-n", "30", "127.0.0.1"].iter().map(OsString::from).collect();
        let (child, _reader) = spawn_in_job(&ping, &args, &sys, &[]).unwrap();
        assert!(child.try_wait().is_none(), "ping should be running");
        println!("PING_PID={}", child.pid);
        use std::io::Write;
        std::io::stdout().flush().unwrap();
        // Exit without dropping the job: the OS closes the last job handle, which kills ping.
        std::process::exit(0);
    }
}
