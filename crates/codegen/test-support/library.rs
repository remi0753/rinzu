//! These tests invoke individual functions from Rust and inspect their results.
//! They are library calls, including when a fixture's function is named `main`.
//! Actual process-entry checks live in the compiler and driver integration tests.
use rinzu_compiler::{
    CompilerOptions, Diagnostic, FrontendArtifact, SourceFile, compile_source_with_options,
};

pub fn compile_library(file: &SourceFile) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    compile_library_with_options(file, &CompilerOptions::default())
}

pub fn compile_library_with_options(
    file: &SourceFile,
    options: &CompilerOptions,
) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    compile_source_with_options(
        file,
        &CompilerOptions {
            is_library: true,
            ..options.clone()
        },
    )
}
