use std::path::Path;

use object::{Object, ObjectKind};
use rinzu_bindgen::native_binding::NativeProviderImage;
use rinzu_bindgen::source_package::sha256;

pub(crate) fn parse(bytes: &[u8], kind: ObjectKind) -> Result<object::File<'_>, String> {
    let file = object::File::parse(bytes).map_err(|error| error.to_string())?;
    if !matches!(file, object::File::Elf64(_))
        || !file.is_little_endian()
        || file.architecture() != object::Architecture::X86_64
        || file.kind() != kind
    {
        return Err(format!("expected little-endian ELF64 x86_64 {kind:?}"));
    }
    if kind == ObjectKind::Dynamic
        && file.segments().any(|segment| {
            use object::{ObjectSegment, SegmentFlags, elf};
            matches!(segment.flags(), SegmentFlags::Elf { p_flags, .. }
                if p_flags & (elf::PF_W | elf::PF_X) == (elf::PF_W | elf::PF_X))
        })
    {
        return Err("ELF shared image has a writable executable segment".into());
    }
    Ok(file)
}

pub(crate) fn closed_object(bytes: &[u8]) -> bool {
    use object::read::elf::SectionHeader;
    let Ok(object::File::Elf64(file)) = parse(bytes, ObjectKind::Relocatable) else {
        return false;
    };
    // .linker-options and .deplibs can direct the linker to undeclared
    // inputs, and FatLTO may cause compiler plugins to be used at link time.
    const LINKER_OPTIONS: object::elf::SectionType = object::elf::SectionType(0x6fff4c01);
    const FAT_LTO: object::elf::SectionType = object::elf::SectionType(0x6fff4c0c);
    file.elf_section_table().iter().all(|section| {
        !matches!(
            section.sh_type(file.endian()),
            LINKER_OPTIONS | object::elf::SHT_LLVM_DEPENDENT_LIBRARIES | FAT_LTO
        )
    }) && !file.sections().any(|section| {
        use object::ObjectSection;
        section
            .name()
            .is_ok_and(|name| name.starts_with(".gnu.lto_"))
    })
}

pub(crate) fn shared(
    bytes: &[u8],
) -> Result<
    (
        object::read::elf::ElfFile64<'_>,
        Vec<String>,
        Option<String>,
    ),
    String,
> {
    let object::File::Elf64(file) = parse(bytes, ObjectKind::Dynamic)? else {
        unreachable!();
    };
    let dynamic = file
        .elf_dynamic_table()
        .map_err(|error| error.to_string())?;
    let mut needed = Vec::new();
    let mut soname = None;
    for entry in &dynamic {
        match entry.tag {
            object::elf::DT_NEEDED | object::elf::DT_SONAME => {
                let bytes = dynamic.string(entry).map_err(|error| error.to_string())?;
                let name = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
                if name.is_empty()
                    || name.contains(['/', '\\', '\n', '\r'])
                    || matches!(name, "." | "..")
                {
                    return Err(format!(
                        "ELF dependency or SONAME is not a leaf name: {name}"
                    ));
                }
                if entry.tag == object::elf::DT_NEEDED {
                    needed.push(name.to_owned());
                } else if soname.replace(name.to_owned()).is_some() {
                    return Err("ELF image has multiple SONAMEs".into());
                }
            }
            object::elf::DT_FLAGS_1 if entry.val & object::elf::DF_1_PIE.0 != 0 => {
                return Err("ELF image is a position-independent executable".into());
            }
            _ => {}
        }
    }
    Ok((file, needed, soname))
}

pub(crate) fn image_info(path: &Path, bytes: &[u8]) -> Result<NativeProviderImage, String> {
    let (file, _, soname) = shared(bytes)?;
    let path = path.to_str().ok_or("ELF image path must be UTF-8")?;
    let build_id = file
        .build_id()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("ELF build-id is absent: {path}"))?;
    Ok(NativeProviderImage {
        path: path.into(),
        bundled: false,
        sha256: Some(sha256(bytes)),
        uuid: String::new(),
        build_id: Some(build_id.iter().map(|byte| format!("{byte:02x}")).collect()),
        soname,
    })
}
