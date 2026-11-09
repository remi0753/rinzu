pub fn create_math(root: &std::path::Path) {
    let sdk = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/macos");
    let text = std::fs::read_to_string(sdk.join("src/macos.rinzu")).unwrap();
    let module = rinzu_compiler::parse_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "macos.rinzu",
        &text,
    ))
    .unwrap();
    let mut source = String::new();
    for import in &module.imports {
        if import.path == ["rinzu", "native"] || import.path == ["rinzu", "string"] {
            let range = import.range;
            source.push_str(&text[range.start_byte as usize..range.end_byte as usize]);
            source.push('\n');
        }
    }
    source.push_str("\nnative {\n");
    for member in module.native_blocks.iter().flat_map(|block| &block.members) {
        let range = match member {
            rinzu_compiler::ast::NativeMember::Record(record) if record.name == "Transform2D" => {
                Some(record.range)
            }
            rinzu_compiler::ast::NativeMember::Function(function)
                if ["rawRotation2D", "rawDiagnostic"].contains(&function.name.as_str()) =>
            {
                Some(function.range)
            }
            _ => None,
        };
        if let Some(range) = range {
            source.push_str(&text[range.start_byte as usize..range.end_byte as usize]);
            source.push('\n');
        }
    }
    source.push_str("}\n");
    for name in ["identity2D", "rotation2D", "checked", "diagnostic"] {
        let function = module
            .declarations
            .iter()
            .find(|declaration| declaration.name() == name)
            .unwrap();
        let range = function.range();
        source.push_str(&text[range.start_byte as usize..range.end_byte as usize]);
        source.push('\n');
    }
    source.push_str("export Transform2D;export rotation2D;\n");
    for directory in ["src", "math/src", "math/native/src", "math/native/include"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    let manifest = std::fs::read_to_string(sdk.join("rinzu.toml"))
        .unwrap()
        .replace("rinzu.macos", "mac_math");
    std::fs::write(root.join("math/rinzu.toml"), manifest).unwrap();
    std::fs::write(root.join("math/src/macos.rinzu"), source).unwrap();
    for path in ["native/src/macos.m", "native/include/rinzu_macos.h"] {
        std::fs::copy(sdk.join(path), root.join("math").join(path)).unwrap();
    }
    std::fs::write(root.join("rinzu.toml"), "[package]\nname=\"mac_native_data\"\nlanguage=\"0.5\"\n[dependencies]\nmac_math={path=\"math\"}\n").unwrap();
    std::fs::write(root.join("src/main.rinzu"), r#"
import mac_math;import rinzu.stdio;
fn execute()->Int {
    let value=mac_math.rotation2D(1.5707963267948966);
    if value.b<0.999999 || value.c > -0.999999 || value.a > 0.000001 || value.d > 0.000001 || value.tx!=0.0 || value.ty!=0.0 {panic("rotation writeback was lost");}
    return 42;
}
async fn main(){try{rinzu.stdio.writeLine(execute());}catch errors{panic("unexpected rotation Error");}}
"#).unwrap();
}
