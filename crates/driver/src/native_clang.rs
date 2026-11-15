use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use rinzu_abi::target::Target;

pub fn target() -> Result<String, String> {
    if Target::host() == Some(Target::X86_64UnknownLinuxGnu) {
        return Ok(Target::X86_64UnknownLinuxGnu.triple().into());
    }
    super::platform::macos::clang_target()
}

#[derive(Clone, Debug)]
pub struct TargetClang {
    pub executable: PathBuf,
    pub sysroot: PathBuf,
    pub fingerprint: BTreeMap<String, String>,
    pub(crate) target: Target,
}

impl TargetClang {
    pub fn resolve() -> Result<Self, String> {
        let _timing = rinzu_compiler::timing::span("native.toolchain");
        match Target::host() {
            Some(Target::Aarch64AppleDarwin) => super::platform::macos::resolve_clang(),
            Some(Target::X86_64UnknownLinuxGnu) => super::platform::linux::resolve_clang(),
            None => Err(format!(
                "no native toolchain for {}",
                rinzu_runtime::current_target()
            )),
        }
    }

    pub fn verify(&self) -> Result<(), String> {
        if Self::resolve()?.fingerprint != self.fingerprint {
            return Err(
                "native toolchain/target/SDK changed before publication; check again".into(),
            );
        }
        Ok(())
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .arg("-target")
            .arg(&self.fingerprint["clang.target"]);
        match self.target {
            Target::Aarch64AppleDarwin => {
                super::platform::macos::configure_clang(self, &mut command);
            }
            Target::X86_64UnknownLinuxGnu => {
                super::platform::linux::configure_clang(self, &mut command);
            }
        }
        command
    }

    pub fn executable_command(&self) -> Command {
        let mut command = self.command();
        if self.target == Target::X86_64UnknownLinuxGnu {
            super::platform::linux::configure_executable(self, &mut command);
        }
        command
    }

    pub fn shared_command(&self, name: &str) -> Command {
        let mut command = self.command();
        match self.target {
            Target::Aarch64AppleDarwin => {
                command
                    .args(["-dynamiclib", "-install_name"])
                    .arg(format!("@rpath/{name}"));
            }
            Target::X86_64UnknownLinuxGnu => {
                super::platform::linux::configure_shared(self, &mut command, name);
            }
        }
        command
    }
}
