//! policy::choose, a pure function: GPU (Vulkan, else CUDA) with full offload when enough VRAM is free, else CPU, else LowMemory.
//!
//! No partial offload: either the whole model fits on the GPU with a margin left for the game,
//! or the CPU runs it.

use crate::BackendPref;

/// VRAM the model needs with full offload and a 1024-token context (measured 2.6 to 2.8 GB).
pub const MODEL_VRAM_MB: u32 = 2900;
/// Available RAM needed for a CPU server (`-lm none`, measured 2.6 GB in use).
pub const CPU_RAM_MB: u32 = 3600;
/// A discrete GPU has at least 3 GB of dedicated memory.
pub const DISCRETE_MIN_MB: u32 = 3072;
pub const VENDOR_NVIDIA: u32 = 0x10DE;

/// What the policy needs to know about one DXGI adapter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdapterSummary {
    pub dedicated_mb: u32,
    pub vendor: u32,
    pub software: bool,
}

/// The adapter with the most dedicated VRAM, if it is a discrete hardware GPU.
pub fn pick_adapter<T>(list: &[T], summary: impl Fn(&T) -> AdapterSummary) -> Option<&T> {
    let best = list.iter().filter(|a| !summary(a).software).max_by_key(|a| summary(a).dedicated_mb)?;
    (summary(best).dedicated_mb >= DISCRETE_MIN_MB).then_some(best)
}

#[derive(Clone, Copy, Debug)]
pub struct Input {
    pub pref: BackendPref,
    /// Two GPU failures or the VRAM guard: CPU for the rest of the session.
    pub gpu_blocked: bool,
    /// The picked discrete adapter (None: no discrete GPU).
    pub adapter: Option<AdapterSummary>,
    pub free_vram_mb: Option<u32>,
    pub avail_ram_mb: u32,
    pub physical_cores: u32,
    /// Setting `cpuThreads` (0 = automatic).
    pub cpu_threads: u32,
    pub gpu_margin_mb: u32,
    /// `runtime\vulkan\ggml-vulkan.dll` exists.
    pub has_vulkan: bool,
    /// `runtime\cuda_v12\` or `cuda_v13\` exists.
    pub has_cuda: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Vulkan { threads: u32 },
    Cuda { threads: u32 },
    Cpu { threads: u32 },
    /// Not enough RAM for a CPU server.
    LowMemory,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decision {
    pub choice: Choice,
    /// Why (for the log): "gpu", "cpu_pref", "gpu_blocked", "no_discrete_gpu", "vram_unknown",
    /// "vram_low", "no_gpu_backend", plus "ram_low" for LowMemory.
    pub reason: &'static str,
}

/// clamp(physicalCores / 2, 2, 4), or the setting when it is not 0.
pub fn cpu_threads(physical_cores: u32, setting: u32) -> u32 {
    if setting > 0 {
        setting.min(64)
    } else {
        (physical_cores / 2).clamp(2, 4)
    }
}

pub fn choose(i: &Input) -> Decision {
    let threads = cpu_threads(i.physical_cores, i.cpu_threads);
    let gpu_reason = gpu_choice(i, threads);
    if let Ok(choice) = gpu_reason {
        return Decision { choice, reason: "gpu" };
    }
    let reason = gpu_reason.unwrap_err();
    if i.avail_ram_mb < CPU_RAM_MB {
        return Decision { choice: Choice::LowMemory, reason: "ram_low" };
    }
    Decision { choice: Choice::Cpu { threads }, reason }
}

fn gpu_choice(i: &Input, threads: u32) -> Result<Choice, &'static str> {
    if i.pref == BackendPref::Cpu {
        return Err("cpu_pref");
    }
    if i.gpu_blocked {
        return Err("gpu_blocked");
    }
    let a = i.adapter.ok_or("no_discrete_gpu")?;
    if a.software || a.dedicated_mb < DISCRETE_MIN_MB {
        return Err("no_discrete_gpu");
    }
    let free = i.free_vram_mb.ok_or("vram_unknown")?;
    if free < MODEL_VRAM_MB + i.gpu_margin_mb {
        return Err("vram_low");
    }
    let cuda_ok = a.vendor == VENDOR_NVIDIA && i.has_cuda;
    match i.pref {
        BackendPref::Vulkan if i.has_vulkan => Ok(Choice::Vulkan { threads }),
        BackendPref::Cuda if cuda_ok => Ok(Choice::Cuda { threads }),
        BackendPref::Auto if i.has_vulkan => Ok(Choice::Vulkan { threads }),
        BackendPref::Auto if cuda_ok => Ok(Choice::Cuda { threads }),
        _ => Err("no_gpu_backend"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTX: AdapterSummary = AdapterSummary { dedicated_mb: 10240, vendor: VENDOR_NVIDIA, software: false };

    fn base() -> Input {
        Input {
            pref: BackendPref::Auto,
            gpu_blocked: false,
            adapter: Some(RTX),
            free_vram_mb: Some(4700),
            avail_ram_mb: 7000,
            physical_cores: 8,
            cpu_threads: 0,
            gpu_margin_mb: 1500,
            has_vulkan: true,
            has_cuda: true,
        }
    }

    #[test]
    fn table() {
        // 10 GB with 4.7 free gives Vulkan.
        assert_eq!(choose(&base()), Decision { choice: Choice::Vulkan { threads: 4 }, reason: "gpu" });
        // 6 GB with 2.5 free gives CPU.
        let i = Input { adapter: Some(AdapterSummary { dedicated_mb: 6144, ..RTX }), free_vram_mb: Some(2500), ..base() };
        assert_eq!(choose(&i), Decision { choice: Choice::Cpu { threads: 4 }, reason: "vram_low" });
        // Integrated GPU gives CPU.
        let i = Input { adapter: Some(AdapterSummary { dedicated_mb: 512, vendor: 0x8086, software: false }), ..base() };
        assert_eq!(choose(&i).choice, Choice::Cpu { threads: 4 });
        assert_eq!(choose(&i).reason, "no_discrete_gpu");
        let i = Input { adapter: None, ..base() };
        assert_eq!(choose(&i).reason, "no_discrete_gpu");
        // 3 GB RAM available gives LowMemory (on the CPU path).
        let i = Input { avail_ram_mb: 3000, free_vram_mb: Some(1000), ..base() };
        assert_eq!(choose(&i), Decision { choice: Choice::LowMemory, reason: "ram_low" });
        // ... but the GPU path does not need RAM.
        let i = Input { avail_ram_mb: 3000, ..base() };
        assert!(matches!(choose(&i).choice, Choice::Vulkan { .. }));
        // Forced CPU.
        let i = Input { pref: BackendPref::Cpu, ..base() };
        assert_eq!(choose(&i), Decision { choice: Choice::Cpu { threads: 4 }, reason: "cpu_pref" });
        // Vulkan missing on NVIDIA gives CUDA.
        let i = Input { has_vulkan: false, ..base() };
        assert_eq!(choose(&i).choice, Choice::Cuda { threads: 4 });
        // ... but not on another vendor.
        let i = Input { has_vulkan: false, adapter: Some(AdapterSummary { vendor: 0x1002, ..RTX }), ..base() };
        assert_eq!(choose(&i).reason, "no_gpu_backend");
    }

    #[test]
    fn margins_and_prefs() {
        // Exactly 2900 + 1500 is enough; one MB less is not.
        assert!(matches!(choose(&Input { free_vram_mb: Some(4400), ..base() }).choice, Choice::Vulkan { .. }));
        assert_eq!(choose(&Input { free_vram_mb: Some(4399), ..base() }).reason, "vram_low");
        assert_eq!(choose(&Input { free_vram_mb: None, ..base() }).reason, "vram_unknown");
        assert_eq!(choose(&Input { gpu_blocked: true, ..base() }).reason, "gpu_blocked");
        assert_eq!(choose(&Input { pref: BackendPref::Cuda, ..base() }).choice, Choice::Cuda { threads: 4 });
        assert_eq!(choose(&Input { pref: BackendPref::Cuda, has_cuda: false, ..base() }).reason, "no_gpu_backend");
        assert_eq!(choose(&Input { pref: BackendPref::Vulkan, has_vulkan: false, ..base() }).reason, "no_gpu_backend");
        assert_eq!(choose(&Input { gpu_margin_mb: 0, free_vram_mb: Some(2900), ..base() }).reason, "gpu");
    }

    #[test]
    fn threads() {
        assert_eq!(cpu_threads(8, 0), 4);
        assert_eq!(cpu_threads(16, 0), 4);
        assert_eq!(cpu_threads(6, 0), 3);
        assert_eq!(cpu_threads(2, 0), 2);
        assert_eq!(cpu_threads(1, 0), 2);
        assert_eq!(cpu_threads(8, 6), 6);
        assert_eq!(choose(&Input { pref: BackendPref::Cpu, physical_cores: 4, ..base() }).choice, Choice::Cpu { threads: 2 });
    }

    #[test]
    fn adapter_pick() {
        let list = [
            AdapterSummary { dedicated_mb: 128, vendor: 0x8086, software: false },
            AdapterSummary { dedicated_mb: 10240, vendor: VENDOR_NVIDIA, software: false },
            AdapterSummary { dedicated_mb: 20000, vendor: 0x1414, software: true },
        ];
        assert_eq!(pick_adapter(&list, |a| *a).unwrap().vendor, VENDOR_NVIDIA);
        assert!(pick_adapter(&list[..1], |a| *a).is_none(), "integrated only");
        assert!(pick_adapter(&list[2..], |a| *a).is_none(), "software only");
        let empty: [AdapterSummary; 0] = [];
        assert!(pick_adapter(&empty, |a| *a).is_none());
    }
}
