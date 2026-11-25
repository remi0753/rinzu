use super::*;
use crate::{Diagnostic, DiagnosticCode, SourceCache, SourceSnapshot, check_package_with_sources};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-function-differential-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("rinzu.toml"),
            "[package]\nname=\"differential\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        Self(root)
    }

    fn source(&self, body: &str) {
        // This unchanged function witnesses reuse even when the edit shifts all
        // following source ranges or makes another function fail checking.
        std::fs::write(
            self.0.join("src/main.rinzu"),
            format!("fn unrelated(value: Int) -> Int {{ return value; }}\n{body}"),
        )
        .unwrap();
    }

    fn check(
        &self,
        sources: &mut SourceCache,
        functions: Option<&mut FunctionFrontendCache>,
    ) -> Result<serde_json::Value, Vec<Diagnostic>> {
        let mut snapshot = SourceSnapshot::new(sources, Default::default());
        if let Some(functions) = functions {
            snapshot.set_function_frontend_cache(functions);
        }
        check_package_with_sources(&self.0, &mut snapshot)
            .map(|(_, artifact)| serde_json::to_value(artifact.ir).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn round_trip(cache: &mut FunctionFrontendCache) {
    let wire = rmp_serde::to_vec(cache).unwrap();
    *cache = rmp_serde::from_slice(&wire).unwrap();
}

fn differential(before: &str, from: &str, to: &str, invalid: Option<DiagnosticCode>) {
    assert_eq!(
        before.matches(from).count(),
        1,
        "the edit must select exactly one input"
    );
    let edited = before.replacen(from, to, 1);
    assert_ne!(before, edited);
    let fixture = Fixture::new();
    fixture.source(before);
    let mut sources = SourceCache::default();
    let mut functions = FunctionFrontendCache::default();
    let original = fixture
        .check(&mut sources, None)
        .expect("the seed must be valid");
    assert_eq!(
        fixture.check(&mut sources, Some(&mut functions)).unwrap(),
        original
    );
    for (stage, text) in [
        ("warm", before),
        ("edited", edited.as_str()),
        ("reverted", before),
    ] {
        round_trip(&mut functions);
        fixture.source(text);
        RESTORED.with_borrow_mut(BTreeMap::clear);
        let restored = fixture.check(&mut sources, Some(&mut functions));
        let counts = RESTORED.with_borrow(Clone::clone);
        let fresh = fixture.check(&mut sources, None);
        assert_eq!(
            restored, fresh,
            "IR or complete diagnostics differ at {stage}"
        );
        assert!(
            counts.values().sum::<usize>() > 0,
            "no memo restored at {stage}"
        );
        if stage == "warm" {
            for name in [
                std::any::type_name::<SharedSummary<crate::error_effects::Summary>>(),
                std::any::type_name::<()>(),
                std::any::type_name::<SharedSummary<crate::error_effects::responsibility::Contract>>(
                ),
                std::any::type_name::<IrFunction>(),
            ] {
                assert!(
                    counts.get(name).copied().unwrap_or(0) > 0,
                    "no warm restoration of {name}"
                );
            }
        }
        if stage == "edited" {
            if let Some(code) = invalid {
                let diagnostics = fresh.expect_err("the edited input must be rejected");
                assert!(
                    diagnostics.iter().any(|diagnostic| diagnostic.code == code),
                    "{diagnostics:#?}"
                );
                // Diagnostic equality above includes primary/secondary ranges,
                // messages, codes and every suggested edit, not just acceptance.
                assert!(
                    diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.primary.end_byte
                            >= diagnostic.primary.start_byte)
                );
            } else {
                assert_ne!(
                    fresh.unwrap(),
                    original,
                    "the accepted edit must affect the IR"
                );
            }
        } else {
            assert_eq!(
                fresh.unwrap(),
                original,
                "the original input must recover at {stage}"
            );
        }
    }
}

#[test]
fn constants() {
    differential(
        "let answer = 41; fn leaf()->Int{return answer;} fn main(){let value=leaf();}",
        "answer = 41",
        "answer = 42",
        None,
    );
}

#[test]
fn schemas() {
    differential(
        "class Pair{const left:Int,const right:Int} fn leaf(value:Pair)->Int{return value.left;} fn main(){let value=leaf(Pair(left:1,right:2));}",
        "const left:Int,const right:Int",
        "const right:Int,const left:Int",
        None,
    );
}

#[test]
fn propagated_errors() {
    differential(
        "error A{} error B{} fn leaf(){throw A(message:\"failure\");} fn middle(){leaf();} fn main(){try{middle();}catch[A]{}}",
        "throw A",
        "throw B",
        Some(DiagnosticCode::NonExhaustiveCatch),
    );
}

#[test]
fn mutually_recursive_errors() {
    differential(
        "error A{} error B{} fn left(flag:Bool){if flag{right(false);}else{throw A(message:\"failure\");}} fn right(flag:Bool){left(flag);} fn main(){try{left(true);}catch[A]{}}",
        "throw A",
        "throw B",
        Some(DiagnosticCode::NonExhaustiveCatch),
    );
}

#[test]
fn explicit_contracts() {
    differential(
        "error A{} fn leaf() nothrow{} fn middle() nothrow{leaf();} fn main(){middle();}",
        "fn leaf() nothrow",
        "fn leaf() throws(A)",
        Some(DiagnosticCode::InvalidErrorContract),
    );
}

#[test]
fn future_observation() {
    differential(
        "error A{} async fn fail(){throw A(message:\"failure\");} async fn receive(value:Future<Unit>){try{await value;}catch errors{}} async fn main(){await receive(fail());}",
        "try{await value;}catch errors{}",
        "",
        Some(DiagnosticCode::FutureDiscarded),
    );
}

#[test]
fn future_factories() {
    differential(
        "error A{} error B{} async fn a(){throw A(message:\"a\");} async fn b(){throw B(message:\"b\");} fn factory()->Future<Unit>{return a();} async fn main(){try{await factory();}catch[A]{}}",
        "return a();",
        "return b();",
        Some(DiagnosticCode::NonExhaustiveCatch),
    );
}

#[test]
fn future_aliases() {
    differential(
        "error A{} async fn fail(){throw A(message:\"failure\");} async fn receive(value:Future<Unit>){let alias=value;try{await alias;}catch errors{}} async fn main(){await receive(fail());}",
        "try{await alias;}catch errors{}",
        "",
        Some(DiagnosticCode::FutureDiscarded),
    );
}

#[test]
fn function_references() {
    differential(
        "error A{} error B{} fn a()->Int{throw A(message:\"a\");} fn b()->Int{throw B(message:\"b\");} fn factory()->fn()->Int{return a;} fn main(){try{let callback=factory();let value=callback();}catch[A]{}}",
        "return a;",
        "return b;",
        Some(DiagnosticCode::NonExhaustiveCatch),
    );
}

#[test]
fn async_closures() {
    differential(
        "error A{} error B{} async fn main(){let callback=async fn(){throw A(message:\"failure\");};try{await callback();}catch[A]{}}",
        "throw A",
        "throw B",
        Some(DiagnosticCode::NonExhaustiveCatch),
    );
}

#[test]
fn finally_changes() {
    differential(
        "fn leaf()->Int{return 1;} fn main(){let value=leaf();}",
        "return 1;",
        "try{return 1;}finally{let cleanup=2;}",
        None,
    );
}

#[test]
fn source_positions() {
    differential(
        "fn leaf()->Int{return 1;} fn main(){let value=leaf();}",
        "fn leaf",
        "\n\nfn leaf",
        None,
    );
}

#[test]
fn function_sets() {
    differential(
        "fn leaf()->Int{return 1;} fn main(){let value=leaf();}",
        "fn leaf",
        "fn inserted()->Int{return 7;}\nfn leaf",
        None,
    );
}

#[test]
fn rethrows() {
    differential(
        "error A{} error B{} fn leaf(){throw A(message:\"failure\");} fn forward(){try{leaf();}catch[problem:Error]{throw problem;}} fn main(){try{forward();}catch[A]{}}",
        "throw A",
        "throw B",
        Some(DiagnosticCode::NonExhaustiveCatch),
    );
}
