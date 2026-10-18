//! Free VRAM (NVML, else DXGI dedicated memory minus the PDH Dedicated Usage counter), RAM and physical cores.
//!
//! Never the DXGI budget or `llama-server --list-devices`: both report the per-process budget,
//! about 4.5 GB above what is really free while WoW runs (measured).

use std::path::Path;
use std::time::Duration;

use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformation, GlobalMemoryStatusEx, RelationProcessorCore, MEMORYSTATUSEX,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
};

use crate::discover::Runtime;
use crate::policy::{self, AdapterSummary};

const MB: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterInfo {
    pub name: String,
    pub luid_high: i32,
    pub luid_low: u32,
    pub vendor: u32,
    pub device: u32,
    pub subsys: u32,
    pub dedicated_mb: u32,
    pub software: bool,
}

impl AdapterInfo {
    pub fn summary(&self) -> AdapterSummary {
        AdapterSummary { dedicated_mb: self.dedicated_mb, vendor: self.vendor, software: self.software }
    }
}

/// Every DXGI adapter.
pub fn adapters() -> Vec<AdapterInfo> {
    let mut v = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return v };
        let mut i = 0;
        while let Ok(a) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            let len = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            v.push(AdapterInfo {
                name: String::from_utf16_lossy(&d.Description[..len]),
                luid_high: d.AdapterLuid.HighPart,
                luid_low: d.AdapterLuid.LowPart,
                vendor: d.VendorId,
                device: d.DeviceId,
                subsys: d.SubSysId,
                dedicated_mb: (d.DedicatedVideoMemory as u64 / MB).min(u32::MAX as u64) as u32,
                software: d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
            });
        }
    }
    v
}

/// `GlobalMemoryStatusEx().ullAvailPhys` in MB.
pub fn avail_ram_mb() -> u32 {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        if GlobalMemoryStatusEx(&mut m).is_err() {
            return 0;
        }
    }
    (m.ullAvailPhys / MB).min(u32::MAX as u64) as u32
}

/// Physical cores (RelationProcessorCore entries), else half the logical processors.
pub fn physical_cores() -> u32 {
    unsafe {
        let mut len = 0u32;
        let _ = GetLogicalProcessorInformation(None, &mut len);
        let n = len as usize / std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
        if n > 0 {
            let mut buf = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); n];
            if GetLogicalProcessorInformation(Some(buf.as_mut_ptr()), &mut len).is_ok() {
                let cores = buf.iter().filter(|e| e.Relationship == RelationProcessorCore).count() as u32;
                if cores > 0 {
                    return cores;
                }
            }
        }
    }
    std::thread::available_parallelism().map(|n| (n.get() as u32 / 2).max(1)).unwrap_or(2)
}

#[repr(C)]
#[derive(Default)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

/// nvmlPciInfo_t (v3 layout) plus spare room.
#[repr(C)]
struct NvmlPciInfo {
    bus_id_legacy: [u8; 16],
    domain: u32,
    bus: u32,
    device: u32,
    pci_device_id: u32,
    pci_sub_system_id: u32,
    bus_id: [u8; 32],
    spare: [u8; 64],
}

type NvmlDev = *mut core::ffi::c_void;

/// NVML from System32 (never from the working directory), loaded at run time.
pub struct Nvml {
    lib: HMODULE,
    count: extern "C" fn(*mut u32) -> i32,
    handle: extern "C" fn(u32, *mut NvmlDev) -> i32,
    memory: extern "C" fn(NvmlDev, *mut NvmlMemory) -> i32,
    pci: extern "C" fn(NvmlDev, *mut NvmlPciInfo) -> i32,
    shutdown: extern "C" fn() -> i32,
}

unsafe impl Send for Nvml {}

impl Nvml {
    pub fn load() -> Option<Nvml> {
        unsafe {
            let lib = LoadLibraryExW(w!("nvml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            macro_rules! sym {
                ($name:expr, $ty:ty) => {
                    match GetProcAddress(lib, $name) {
                        Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f),
                        None => {
                            let _ = FreeLibrary(lib);
                            return None;
                        }
                    }
                };
            }
            let init = sym!(s!("nvmlInit_v2"), extern "C" fn() -> i32);
            let n = Nvml {
                lib,
                count: sym!(s!("nvmlDeviceGetCount_v2"), extern "C" fn(*mut u32) -> i32),
                handle: sym!(s!("nvmlDeviceGetHandleByIndex_v2"), extern "C" fn(u32, *mut NvmlDev) -> i32),
                memory: sym!(s!("nvmlDeviceGetMemoryInfo"), extern "C" fn(NvmlDev, *mut NvmlMemory) -> i32),
                pci: sym!(s!("nvmlDeviceGetPciInfo_v3"), extern "C" fn(NvmlDev, *mut NvmlPciInfo) -> i32),
                shutdown: sym!(s!("nvmlShutdown"), extern "C" fn() -> i32),
            };
            if init() != 0 {
                let _ = FreeLibrary(lib);
                return None;
            }
            Some(n)
        }
    }

    /// Free memory of the NVIDIA device with the adapter's PCI ids (NVML exposes no LUID). None
    /// when no device, or more than one identical device, matches.
    pub fn free_mb(&self, a: &AdapterInfo) -> Option<u32> {
        if a.vendor != policy::VENDOR_NVIDIA {
            return None;
        }
        let want_id = (a.device << 16) | a.vendor;
        let mut n = 0u32;
        if (self.count)(&mut n) != 0 {
            return None;
        }
        let mut found = None;
        let mut matches = 0;
        for i in 0..n.min(16) {
            let mut dev: NvmlDev = std::ptr::null_mut();
            if (self.handle)(i, &mut dev) != 0 {
                continue;
            }
            let mut pci = NvmlPciInfo { bus_id_legacy: [0; 16], domain: 0, bus: 0, device: 0, pci_device_id: 0, pci_sub_system_id: 0, bus_id: [0; 32], spare: [0; 64] };
            if (self.pci)(dev, &mut pci) != 0 || pci.pci_device_id != want_id || pci.pci_sub_system_id != a.subsys {
                continue;
            }
            matches += 1;
            let mut m = NvmlMemory::default();
            if (self.memory)(dev, &mut m) == 0 {
                found = Some((m.free / MB).min(u32::MAX as u64) as u32);
            }
        }
        if matches == 1 {
            found
        } else {
            None
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        (self.shutdown)();
        unsafe {
            let _ = FreeLibrary(self.lib);
        }
    }
}

/// The "GPU Adapter Memory(*)\Dedicated Usage" counter (Task Manager's source). The first collect
/// takes about 1 s; later reads are nearly free, so the query stays open.
pub struct Pdh {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

unsafe impl Send for Pdh {}

/// Parses "luid_0x00000000_0x0000D1A5_phys_0" into (high, low).
pub fn parse_luid_instance(name: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix("luid_0x")?;
    let (high, rest) = rest.split_once("_0x")?;
    let low = rest.split('_').next()?;
    Some((u32::from_str_radix(high, 16).ok()?, u32::from_str_radix(low, 16).ok()?))
}

impl Pdh {
    pub fn open() -> Option<Pdh> {
        unsafe {
            let mut q = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
                return None;
            }
            let mut c = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(q, w!(r"\GPU Adapter Memory(*)\Dedicated Usage"), 0, &mut c) != 0 {
                PdhCloseQuery(q);
                return None;
            }
            Some(Pdh { query: q, counter: c })
        }
    }

    /// Dedicated memory in use on one adapter (all its physical instances), in MB.
    pub fn dedicated_usage_mb(&mut self, a: &AdapterInfo) -> Option<u32> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let mut size = 0u32;
            let mut count = 0u32;
            let rc = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_LARGE, &mut size, &mut count, None);
            if rc != PDH_MORE_DATA || size == 0 {
                return None;
            }
            // u64 storage keeps the item array aligned.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_LARGE, &mut size, &mut count, Some(items)) != 0 {
                return None;
            }
            let items = std::slice::from_raw_parts(items, count as usize);
            let mut total: i64 = 0;
            let mut hit = false;
            for it in items {
                let name = it.szName.to_string().unwrap_or_default();
                if parse_luid_instance(&name) == Some((a.luid_high as u32, a.luid_low)) {
                    total += it.FmtValue.Anonymous.largeValue.max(0);
                    hit = true;
                }
            }
            hit.then(|| (total as u64 / MB).min(u32::MAX as u64) as u32)
        }
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// Free VRAM of one adapter: NVML when it matches, else DXGI dedicated minus PDH usage.
#[derive(Default)]
pub struct VramProbe {
    nvml: Option<Nvml>,
    nvml_tried: bool,
    pdh: Option<Pdh>,
    pdh_tried: bool,
}

impl VramProbe {
    pub fn free_mb(&mut self, a: &AdapterInfo) -> (Option<u32>, &'static str) {
        if !self.nvml_tried {
            self.nvml_tried = true;
            if a.vendor == policy::VENDOR_NVIDIA {
                self.nvml = Nvml::load();
            }
        }
        if let Some(v) = self.nvml.as_ref().and_then(|n| n.free_mb(a)) {
            return (Some(v), "nvml");
        }
        if !self.pdh_tried {
            self.pdh_tried = true;
            self.pdh = Pdh::open();
        }
        if let Some(used) = self.pdh.as_mut().and_then(|p| p.dedicated_usage_mb(a)) {
            return (Some(a.dedicated_mb.saturating_sub(used)), "pdh");
        }
        (None, "none")
    }
}

/// Parses `llama-server --list-devices` lines such as
/// "  Vulkan0: NVIDIA GeForce RTX 3080 (10051 MiB, 9283 MiB free)".
pub fn parse_list_devices(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((id, rest)) = line.split_once(": ") else { continue };
        let Some(num) = id.strip_prefix("Vulkan") else { continue };
        if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let name = match rest.rfind(" (") {
            Some(i) => &rest[..i],
            None => rest,
        };
        v.push((id.to_string(), name.trim().to_string()));
    }
    v
}

/// The `-dev` value for the adapter: a matching name, else the only device listed.
pub fn match_vulkan_device(list: &[(String, String)], adapter_name: &str) -> Option<String> {
    let want = adapter_name.trim().to_lowercase();
    if !want.is_empty() {
        for (id, name) in list {
            let n = name.to_lowercase();
            if n == want || n.contains(&want) || want.contains(&n) {
                return Some(id.clone());
            }
        }
    }
    (list.len() == 1).then(|| list[0].0.clone())
}

/// Everything the worker reads before starting a server.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub adapter: Option<AdapterInfo>,
    pub free_vram_mb: Option<u32>,
    pub vram_source: &'static str,
    pub avail_ram_mb: u32,
    pub physical_cores: u32,
}

/// Device queries, behind a trait so the worker can be tested with fakes.
pub trait Devices: Send {
    fn snapshot(&mut self) -> Snapshot;
    /// Free VRAM of the adapter of the last snapshot (VRAM guard).
    fn free_vram_mb(&mut self) -> Option<u32>;
    /// `-dev` value for Vulkan (runs `llama-server --list-devices` once).
    fn vulkan_device(&mut self, rt: &Runtime, adapter_name: &str) -> Option<String>;
}

#[derive(Default)]
pub struct SystemDevices {
    probe: VramProbe,
    adapter: Option<AdapterInfo>,
    vulkan: Option<Option<String>>,
}

impl Devices for SystemDevices {
    fn snapshot(&mut self) -> Snapshot {
        let list = adapters();
        self.adapter = policy::pick_adapter(&list, AdapterInfo::summary).cloned();
        let (free, source) = match &self.adapter {
            Some(a) => self.probe.free_mb(a),
            None => (None, "none"),
        };
        Snapshot { adapter: self.adapter.clone(), free_vram_mb: free, vram_source: source, avail_ram_mb: avail_ram_mb(), physical_cores: physical_cores() }
    }

    fn free_vram_mb(&mut self) -> Option<u32> {
        let a = self.adapter.clone()?;
        self.probe.free_mb(&a).0
    }

    fn vulkan_device(&mut self, rt: &Runtime, adapter_name: &str) -> Option<String> {
        if self.vulkan.is_none() {
            let out = crate::server::run_capture(&rt.exe, &["--list-devices"], &rt.vulkan_dir(), Duration::from_secs(20)).ok();
            let list = out.map(|(t, _)| parse_list_devices(&t)).unwrap_or_default();
            self.vulkan = Some(match_vulkan_device(&list, adapter_name));
        }
        self.vulkan.clone().flatten()
    }
}

/// Marker file (empty) whose presence means a Vulkan warm-up already compiled the shaders on this
/// PC, so the next start is not a "first GPU use".
pub fn vulkan_marker(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("run").join("vulkan_warmed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luid_instance_names() {
        assert_eq!(parse_luid_instance("luid_0x00000000_0x0000D1A5_phys_0"), Some((0, 0xD1A5)));
        assert_eq!(parse_luid_instance("luid_0x00000001_0x0001ABCD_phys_1"), Some((1, 0x1ABCD)));
        assert_eq!(parse_luid_instance("_Total"), None);
        assert_eq!(parse_luid_instance("luid_0xZZ_0x1_phys_0"), None);
    }

    #[test]
    fn list_devices() {
        let text = "ggml_vulkan: Found 1 Vulkan devices:\nAvailable devices:\n  Vulkan0: NVIDIA GeForce RTX 3080 (10051 MiB, 9283 MiB free)\n";
        let l = parse_list_devices(text);
        assert_eq!(l, vec![("Vulkan0".to_string(), "NVIDIA GeForce RTX 3080".to_string())]);
        assert_eq!(match_vulkan_device(&l, "NVIDIA GeForce RTX 3080").as_deref(), Some("Vulkan0"));
        assert_eq!(match_vulkan_device(&l, "Some other name").as_deref(), Some("Vulkan0"), "single device");
        let two = vec![("Vulkan0".to_string(), "AMD Radeon(TM) Graphics".to_string()), ("Vulkan1".to_string(), "NVIDIA GeForce RTX 3080".to_string())];
        assert_eq!(match_vulkan_device(&two, "nvidia geforce rtx 3080").as_deref(), Some("Vulkan1"));
        assert_eq!(match_vulkan_device(&two, "Intel Arc"), None);
        assert!(parse_list_devices("Available devices:\n  (none)\n").is_empty());
    }

    /// NVML (by PCI ids) and the PDH fallback (DXGI dedicated minus Dedicated Usage, by LUID) must
    /// agree within 300 MB on an NVIDIA adapter. Skipped without NVML.
    #[test]
    fn pdh_fallback_agrees_with_nvml() {
        let list = adapters();
        let Some(a) = policy::pick_adapter(&list, AdapterInfo::summary).cloned() else { return };
        let Some(nvml) = Nvml::load() else { return };
        let Some(n) = nvml.free_mb(&a) else { return };
        let Some(used) = Pdh::open().and_then(|mut p| p.dedicated_usage_mb(&a)) else { panic!("PDH counter missing") };
        let p = a.dedicated_mb.saturating_sub(used);
        assert!(n.abs_diff(p) < 300, "nvml free {n} MB, pdh free {p} MB");
    }

    /// Reads the real machine (no server, no model): values must be plausible.
    #[test]
    fn system_queries_are_plausible() {
        assert!(physical_cores() >= 1);
        assert!(avail_ram_mb() > 0);
        let list = adapters();
        assert!(!list.is_empty(), "DXGI always lists at least the basic render driver");
        let mut d = SystemDevices::default();
        let s = d.snapshot();
        if let (Some(a), Some(free)) = (&s.adapter, s.free_vram_mb) {
            assert!(free <= a.dedicated_mb, "free {free} > dedicated {}", a.dedicated_mb);
        }
    }
}
