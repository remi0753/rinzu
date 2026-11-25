//! Source coordinates use the same stable file identities as diagnostics.
use rinzu_compiler::IrModule;
use rinzu_runtime::data::SourceCatalog;
use std::fmt::Write;
use std::sync::Arc;

pub fn format_error_traces(module: &IrModule, failure: &rinzu_runtime::failure::Failure) -> String {
    if failure.traces.is_empty() {
        return String::new();
    }
    rinzu_runtime::error_trace::format_failure(failure, &catalog(module))
}

pub(crate) fn catalog(module: &IrModule) -> Arc<SourceCatalog> {
    let (identities, files) = crate::baseline::source_identities(module);
    Arc::new(SourceCatalog {
        files,
        positions: module
            .source_locations
            .iter()
            .filter_map(|(file, positions)| {
                Some((
                    *identities.get(file)?,
                    positions
                        .iter()
                        .map(|(byte, at)| (*byte, (at.line, at.column)))
                        .collect(),
                ))
            })
            .collect(),
    })
}

pub(crate) fn c_source(module: &IrModule) -> String {
    let catalog = catalog(module);
    let mut output = String::new();
    for (file, positions) in &catalog.positions {
        if positions.is_empty() {
            continue;
        }
        let name = crate::data_schema::c_string(&catalog.files[file]);
        writeln!(
            output,
            "static const char rinzu_source_name_{file}[] = {name};"
        )
        .unwrap();
    }
    output.push_str("static const rinzu_source_entry_t rinzu_program_sources[] = {\n");
    for (file, positions) in &catalog.positions {
        let length = catalog.files[file].len();
        for (byte, (line, column)) in positions {
            writeln!(
                output,
                "{{UINT64_C({file}),{byte},{line},{column},rinzu_source_name_{file},{length}}},"
            )
            .unwrap();
        }
    }
    output.push_str("{0,0,0,0,NULL,0}};\nstatic void rinzu_configure_program_sources(rinzu_managed_data_t *data) { data->source_entries = rinzu_program_sources; data->source_entry_count = sizeof rinzu_program_sources / sizeof *rinzu_program_sources - 1; }\n");
    output
}
