use rinzu_abi::target::Target;
use std::path::{Path, PathBuf};

pub fn build_directory(root: &Path) -> PathBuf {
    let build = root.join("build");
    match Target::host() {
        Some(Target::X86_64UnknownLinuxGnu) => build.join(Target::X86_64UnknownLinuxGnu.triple()),
        _ => build,
    }
}
