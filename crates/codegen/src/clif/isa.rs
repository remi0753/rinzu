use anyhow::{Context, Result, anyhow};
use cranelift_codegen::isa::{self, OwnedTargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use rinzu_abi::target::Target;
use target_lexicon::Triple;

fn flags(pic: bool) -> Result<settings::Flags> {
    let mut flags = settings::builder();
    flags
        .set("is_pic", if pic { "true" } else { "false" })
        .context("configure position-independent code")?;
    flags
        .set("use_colocated_libcalls", "false")
        .context("allow external runtime symbols")?;
    Ok(settings::Flags::new(flags))
}

pub(crate) fn native_isa(pic: bool) -> Result<OwnedTargetIsa> {
    cranelift_native::builder()
        .map_err(|message| anyhow!(message))?
        .finish(flags(pic)?)
        .context("build native Cranelift ISA")
}

pub(super) fn aot_isa(target: Target) -> Result<OwnedTargetIsa> {
    if target == Target::Aarch64AppleDarwin && Target::host() == Some(target) {
        // Preserve the existing Apple Silicon ISA configuration, including PAC.
        return native_isa(true);
    }
    let triple = target
        .triple()
        .parse::<Triple>()
        .map_err(|error| anyhow!("invalid Cranelift target {}: {error}", target.triple()))?;
    isa::lookup(triple)
        .with_context(|| format!("Cranelift does not provide ISA for {}", target.triple()))?
        .finish(flags(true)?)
        .with_context(|| format!("configure the AOT ISA for {}", target.triple()))
}
