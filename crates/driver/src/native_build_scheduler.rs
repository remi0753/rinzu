//! Bounded SDK compilation with dependencies released as each job completes.
use std::collections::{BTreeMap, BTreeSet};

pub fn build<T: Send>(
    names: &[String],
    dependencies: &[BTreeSet<String>],
    jobs: usize,
    build_one: impl Fn(usize, &BTreeMap<String, String>) -> Result<(T, String), String> + Sync,
) -> Result<(Vec<T>, BTreeMap<String, String>), String> {
    assert!(jobs > 0 && names.len() == dependencies.len());
    let mut scheduled = vec![false; names.len()];
    let mut outputs: Vec<Option<T>> = std::iter::repeat_with(|| None).take(names.len()).collect();
    let mut digests = BTreeMap::new();
    let attempt = |index, inputs: &BTreeMap<String, String>| {
        crate::cli_diagnostics::capture(|| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_one(index, inputs)))
                .unwrap_or_else(|_| Err("native SDK compiler worker panicked".into()))
        })
    };
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut active = 0;
        let mut completed = 0;
        let mut errors = BTreeMap::<usize, (String, Vec<String>)>::new();
        loop {
            for index in 0..names.len() {
                if active == jobs {
                    break;
                }
                if scheduled[index]
                    || !dependencies[index]
                        .iter()
                        .all(|name| digests.contains_key(name))
                {
                    continue;
                }
                scheduled[index] = true;
                active += 1;
                if jobs == 1 || names.len() == 1 {
                    let _ = sender.send((index, attempt(index, &digests)));
                } else {
                    let send = sender.clone();
                    let snapshot = digests.clone();
                    let attempt = &attempt;
                    if std::thread::Builder::new()
                        .name(format!("rinzu-sdk-{index}"))
                        .stack_size(8 * 1024 * 1024)
                        .spawn_scoped(scope, move || {
                            // A failed worker must always release its scheduler slot,
                            // including under the test harness's unwind strategy.
                            let _ = send.send((index, attempt(index, &snapshot)));
                        })
                        .is_err()
                    {
                        let _ = sender.send((index, attempt(index, &digests)));
                    }
                }
            }
            if active == 0 {
                let mut first_error = None;
                for (error, blocks) in errors.into_values() {
                    first_error.get_or_insert(error);
                    for block in blocks {
                        crate::cli_diagnostics::write_block(&block);
                    }
                }
                if let Some(error) = first_error {
                    return Err(error);
                }
                if completed != names.len() {
                    return Err("source SDK dependencies have no validated build interface (missing dependency or cycle)".into());
                }
                return Ok((outputs.into_iter().map(Option::unwrap).collect(), digests));
            }
            let (index, (result, blocks)) = receiver.recv().map_err(|error| error.to_string())?;
            active -= 1;
            completed += 1;
            match result {
                Ok((output, digest)) => {
                    outputs[index] = Some(output);
                    digests.insert(names[index].clone(), digest);
                }
                Err(error) => {
                    // Finish independent work, but never start consumers of a
                    // failed dependency. Report errors in checked graph order.
                    errors.insert(index, (error, blocks));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    #[test]
    fn a_completed_dependency_releases_work_before_unrelated_jobs_finish() {
        let names = ["io".into(), "macos".into(), "stream".into()];
        let dependencies = [
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::from(["io".into()]),
        ];
        let (send, receive) = mpsc::channel();
        let receive = Mutex::new(receive);
        let (outputs, digests) = build(&names, &dependencies, 2, |index, inputs| {
            if index == 1 {
                receive
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|error| error.to_string())?;
            } else if index == 2 {
                assert_eq!(inputs["io"], "digest-0");
                send.send(()).unwrap();
            }
            Ok((index, format!("digest-{index}")))
        })
        .unwrap();
        assert_eq!(outputs, [0, 1, 2]);
        assert_eq!(digests["stream"], "digest-2");
    }

    #[test]
    fn completion_order_does_not_change_the_first_error() {
        let (send, receive) = mpsc::channel();
        let receive = Mutex::new(receive);
        let result = build::<()>(
            &["first".into(), "second".into()],
            &[BTreeSet::new(), BTreeSet::new()],
            2,
            |index, _| {
                if index == 0 {
                    receive
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                    Err("first failed".into())
                } else {
                    send.send(()).unwrap();
                    Err("second failed".into())
                }
            },
        );
        assert_eq!(result.unwrap_err(), "first failed");
    }

    #[test]
    fn missing_dependencies_return_an_error_without_waiting() {
        let result = build::<()>(
            &["consumer".into()],
            &[BTreeSet::from(["missing".into()])],
            2,
            |_, _| unreachable!(),
        );
        assert!(result.unwrap_err().contains("missing dependency or cycle"));
    }
}
