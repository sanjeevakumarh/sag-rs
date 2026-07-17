//! Node bootstrap: detect this box (OS + GPU vendor) and recommend a serving
//! engine, so `setup-node.sh` can stay a ~15-line wrapper with no install logic in
//! bash (ARCHITECTURE.md "The two bootstrap scripts"). Detection sits behind an
//! [`EnvProbe`] seam so it is unit-tested with no real hardware.

/// The GPU stack detected on this box (drives the serving-engine choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Cpu,
}

impl GpuVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::Cpu => "CPU-only",
        }
    }
}

/// What bootstrap concluded about this box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub os: String,
    pub gpu: GpuVendor,
    /// Recommended serving engine, per the ARCHITECTURE.md detect-and-serve table.
    pub engine: &'static str,
}

/// Seam for probing the environment (command / path presence), so [`detect`] is
/// testable without real hardware.
pub trait EnvProbe {
    fn has_command(&self, name: &str) -> bool;
    fn has_path(&self, path: &str) -> bool;
}

/// Real probe: looks up commands on `PATH` and checks the filesystem.
pub struct SystemProbe;

impl EnvProbe for SystemProbe {
    fn has_command(&self, name: &str) -> bool {
        std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
            .unwrap_or(false)
    }

    fn has_path(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
}

/// Detect GPU vendor from tell-tale tools/paths, then map `(os, vendor)` to a
/// serving engine. Pure given a probe — the detection heuristics live in one place.
pub fn detect(os: &str, probe: &dyn EnvProbe) -> Detection {
    let gpu = if probe.has_command("nvidia-smi") {
        GpuVendor::Nvidia
    } else if probe.has_command("rocm-smi") || probe.has_path("/opt/rocm") {
        GpuVendor::Amd
    } else if probe.has_command("xpu-smi") || probe.has_command("sycl-ls") {
        GpuVendor::Intel
    } else {
        GpuVendor::Cpu
    };

    let engine = recommend_engine(os, gpu);
    Detection {
        os: os.to_string(),
        gpu,
        engine,
    }
}

/// Map `(os, gpu)` to a serving engine (ARCHITECTURE.md "detect rather than ask").
fn recommend_engine(os: &str, gpu: GpuVendor) -> &'static str {
    match os {
        "macos" => "ollama (MLX opt-in later)",
        "windows" => "ollama",
        _ => match gpu {
            GpuVendor::Nvidia => "vLLM (fallback: llama.cpp)",
            GpuVendor::Amd => "llama.cpp / ROCm",
            GpuVendor::Intel => "llama.cpp (Vulkan/SYCL)",
            GpuVendor::Cpu => "llama.cpp (CPU)",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProbe {
        commands: &'static [&'static str],
        paths: &'static [&'static str],
    }
    impl EnvProbe for FakeProbe {
        fn has_command(&self, name: &str) -> bool {
            self.commands.contains(&name)
        }
        fn has_path(&self, path: &str) -> bool {
            self.paths.contains(&path)
        }
    }

    fn probe(commands: &'static [&'static str], paths: &'static [&'static str]) -> FakeProbe {
        FakeProbe { commands, paths }
    }

    #[test]
    fn nvidia_on_linux_picks_vllm() {
        let d = detect("linux", &probe(&["nvidia-smi"], &[]));
        assert_eq!(d.gpu, GpuVendor::Nvidia);
        assert!(d.engine.starts_with("vLLM"));
    }

    #[test]
    fn amd_detected_by_rocm_path() {
        let d = detect("linux", &probe(&[], &["/opt/rocm"]));
        assert_eq!(d.gpu, GpuVendor::Amd);
        assert!(d.engine.contains("ROCm"));
    }

    #[test]
    fn intel_detected_by_sycl() {
        let d = detect("linux", &probe(&["sycl-ls"], &[]));
        assert_eq!(d.gpu, GpuVendor::Intel);
    }

    #[test]
    fn no_gpu_falls_back_to_cpu_llamacpp() {
        let d = detect("linux", &probe(&[], &[]));
        assert_eq!(d.gpu, GpuVendor::Cpu);
        assert!(d.engine.contains("CPU"));
    }

    #[test]
    fn macos_prefers_ollama_regardless_of_gpu() {
        let d = detect("macos", &probe(&["nvidia-smi"], &[]));
        assert!(d.engine.contains("ollama"));
    }
}
