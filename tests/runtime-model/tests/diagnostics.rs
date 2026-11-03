use rinzu_runtime_model::diagnostics::{
    DiagnosticCause, HostDriving, LatencyProfiler, SourceLocation,
};
use rinzu_runtime_model::host_adapters::{
    FrameTickAdapter, HostAdapter, NestedMode, QueuePostAdapter,
};
use std::thread;
use std::time::Duration;

fn source(function: &'static str, line: u32) -> SourceLocation {
    SourceLocation {
        file: "sample.rinzu",
        line,
        function,
    }
}

fn inject_and_assert(driving: HostDriving) {
    let mut profiler = LatencyProfiler::new(driving, source("onEvent", 10));
    profiler.record_pending_wait(source("loadFile", 12), Duration::from_millis(40));
    for (cause, location) in [
        (DiagnosticCause::CpuManaged, source("layout", 20)),
        (DiagnosticCause::BlockingNative, source("legacyRead", 30)),
        (DiagnosticCause::Transport, source("spawnInput", 40)),
        (DiagnosticCause::GcPause, source("allocateTree", 50)),
        (
            DiagnosticCause::CallbackReentry,
            source("nativeDelegate", 60),
        ),
    ] {
        profiler.measure_owner_segment(cause, location, || {
            thread::sleep(Duration::from_millis(1));
        });
    }
    let report = profiler.finish();
    assert_eq!(report.event_source, source("onEvent", 10));
    assert!(report.event_latency >= Duration::from_millis(5));
    assert_eq!(report.events.len(), 6);
    assert_eq!(report.events[0].cause, DiagnosticCause::PendingWait);
    assert_eq!(report.events[0].owner_latency, Duration::ZERO);
    assert_eq!(report.events[0].source, source("loadFile", 12));
    for event in &report.events[1..] {
        assert!(event.owner_latency >= Duration::from_millis(1));
        assert_eq!(event.source.file, "sample.rinzu");
        assert!(event.source.line >= 20);
    }
}

#[test]
fn queue_event_and_frame_latency_identify_each_cause_and_source() {
    let mut queue = QueuePostAdapter::default();
    queue.activate(1);
    queue.post_owner_turn(1, NestedMode::Root, 1).unwrap();
    assert_eq!(queue.drive_once(), Some(1));
    inject_and_assert(HostDriving::QueuePost);

    let mut frame = FrameTickAdapter::default();
    frame.activate(2);
    frame.post_owner_turn(2, NestedMode::Root, 2).unwrap();
    assert_eq!(frame.drive_once(), Some(2));
    inject_and_assert(HostDriving::FrameTick);
}
