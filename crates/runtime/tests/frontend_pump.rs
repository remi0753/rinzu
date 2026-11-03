use std::sync::{Arc, Mutex};

use rinzu_compiler::{FileId, SourceFile, compile_source};
use rinzu_runtime::scheduler::TaskOutcome;
use rinzu_runtime::standalone::{
    ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
};

struct FrontendMain {
    function_name: String,
    activations: Arc<Mutex<Vec<std::thread::ThreadId>>>,
}

impl ManagedTask for FrontendMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        self.activations
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        assert_eq!(self.function_name, "main");
        assert_eq!(event, TaskEvent::Start);
        TaskAction::Return
    }
}

#[test]
fn verified_frontend_main_runs_to_ledger_baseline_on_owner_pump() {
    let source = SourceFile::new(
        FileId(1),
        "main.rinzu",
        r#"
fn main() {
    let message = "runtime foundation";
}
"#,
    );
    let artifact = compile_source(&source).expect("I1 frontend program must compile");
    let main = artifact
        .ir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("verified IR must contain main");

    let owner = std::thread::current().id();
    let activations = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime
        .start_main(Box::new(FrontendMain {
            function_name: main.name.clone(),
            activations: Arc::clone(&activations),
        }))
        .unwrap();

    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert_eq!(&*activations.lock().unwrap(), &[owner]);
    assert!(runtime.resources().used.is_baseline());
}
