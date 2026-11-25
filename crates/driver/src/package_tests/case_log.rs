use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub(super) const FILE: &str = "case-events.jsonl";
pub(super) const PATH_ENV: &str = "RINZU_FILE_TEST_EVENTS";
pub(super) const FILTER_ENV: &str = "RINZU_FILE_TEST_CASE";
pub(super) const SUPERVISOR_ENV: &str = "RINZU_FILE_TEST_SUPERVISOR";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NamedCase {
    pub name: String,
    pub status: String,
    elapsed_ms: Option<u64>,
    errors: Vec<CaseError>,
}

#[derive(Deserialize, Serialize)]
struct CaseError {
    name: String,
    message: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Event {
    Start {
        sequence: usize,
        name: String,
    },
    Error {
        sequence: usize,
        #[serde(flatten)]
        error: CaseError,
    },
    End {
        sequence: usize,
        failed: bool,
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
    },
    Invalid {
        message: String,
    },
}

#[derive(Default)]
pub(super) struct CaseLog {
    pub cases: Vec<NamedCase>,
    pub misuse: Option<String>,
    pub problem: Option<String>,
}

impl CaseLog {
    pub fn read(scratch: &Path) -> Self {
        let mut log = Self::default();
        if let Err(error) = log.read_events(&scratch.join(FILE)) {
            log.problem = Some(format!("invalid named case log: {error}"));
        }
        log
    }

    pub fn has_active_case(&self) -> bool {
        self.cases
            .last()
            .is_some_and(|case| case.status == "running")
    }

    pub fn failed(&self) -> bool {
        self.misuse.is_some() || self.cases.iter().any(|case| case.status == "fail")
    }

    pub fn finish_active(&mut self, status: &str) {
        if self.has_active_case() {
            self.cases.last_mut().expect("active case").status = status.to_owned();
        }
    }

    fn read_events(&mut self, path: &Path) -> Result<(), String> {
        let file = super::open_report(path).map_err(|error| error.to_string())?;
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        let mut reader = BufReader::new(file.take(size));
        let mut line = Vec::new();
        let mut names = std::collections::HashSet::new();
        loop {
            line.clear();
            reader
                .by_ref()
                .take((super::OUTPUT_LIMIT + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|error| error.to_string())?;
            if line.is_empty() {
                return Ok(());
            }
            if line.len() > super::OUTPUT_LIMIT || line.last() != Some(&b'\n') {
                return Err("incomplete or oversized record".into());
            }
            match serde_json::from_slice::<Event>(&line).map_err(|error| error.to_string())? {
                Event::Start { sequence, name } => {
                    if sequence != self.cases.len() + 1
                        || self.has_active_case()
                        || !names.insert(name.clone())
                    {
                        return Err("unexpected case start".into());
                    }
                    self.cases.push(NamedCase {
                        name,
                        status: "running".into(),
                        elapsed_ms: None,
                        errors: Vec::new(),
                    });
                }
                Event::Error { sequence, error } => {
                    self.active_case(sequence)?.errors.push(error);
                }
                Event::End {
                    sequence,
                    failed,
                    elapsed_ms,
                } => {
                    let case = self.active_case(sequence)?;
                    if failed == case.errors.is_empty() {
                        return Err("case result disagrees with its errors".into());
                    }
                    case.status = if failed { "fail" } else { "pass" }.into();
                    case.elapsed_ms = Some(elapsed_ms);
                }
                Event::Invalid { message } => {
                    self.misuse.get_or_insert(message);
                }
            }
        }
    }

    fn active_case(&mut self, sequence: usize) -> Result<&mut NamedCase, String> {
        if sequence != self.cases.len() || !self.has_active_case() {
            return Err("record does not belong to the active case".into());
        }
        Ok(self.cases.last_mut().expect("active case"))
    }
}
