//! Target identities shared by the frontend, code generator, and native artifacts.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Aarch64AppleDarwin,
    X86_64UnknownLinuxGnu,
}

impl Target {
    pub const fn host() -> Option<Self> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Self::Aarch64AppleDarwin)
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            Some(Self::X86_64UnknownLinuxGnu)
        } else {
            None
        }
    }

    pub const fn triple(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "aarch64-apple-darwin",
            Self::X86_64UnknownLinuxGnu => "x86_64-unknown-linux-gnu",
        }
    }

    pub fn from_triple(triple: &str) -> Option<Self> {
        [Self::Aarch64AppleDarwin, Self::X86_64UnknownLinuxGnu]
            .into_iter()
            .find(|target| target.triple() == triple)
    }

    pub const fn supports_native_packages(self) -> bool {
        true
    }

    pub const fn supports_async_aot(self) -> bool {
        true
    }

    pub const fn supports_jit(self) -> bool {
        match self {
            Self::Aarch64AppleDarwin | Self::X86_64UnknownLinuxGnu => true,
        }
    }

    pub const fn jit_flushes_instruction_cache(self) -> bool {
        matches!(self, Self::Aarch64AppleDarwin)
    }

    pub const fn supports_supplant(self) -> bool {
        matches!(self, Self::Aarch64AppleDarwin)
    }

    pub fn shared_library_name(self, stem: &str) -> String {
        match self {
            Self::Aarch64AppleDarwin => format!("{stem}.dylib"),
            Self::X86_64UnknownLinuxGnu => format!("{stem}.so"),
        }
    }

    pub const fn llvm_data_layout(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "e-m:o-i64:64-i128:128-n32:64-S128",
            Self::X86_64UnknownLinuxGnu => {
                "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
            }
        }
    }
}
