//! File-only JSONL observation. No sampler exists unless explicitly requested.
use crate::resource_accounting::{self, Category};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    pub path: PathBuf,
    pub interval_ms: u64,
}

#[derive(serde::Serialize)]
struct Snapshot<'a> {
    schema: &'static str,
    version: u32,
    producer: &'a str,
    process_id: u32,
    sequence: u64,
    phase: &'a str,
    elapsed_ns: u64,
    consistency: &'static str,
    sources: &'a BTreeMap<u64, String>,
    categories: Vec<Category>,
}
struct Writer {
    file: File,
    started: Instant,
    sequence: u64,
    producer: String,
    sources: BTreeMap<u64, String>,
    end_categories: Vec<Category>,
    error: Option<String>,
}
impl Writer {
    fn snapshot(&mut self, phase: &str) -> io::Result<()> {
        if let Some(error) = &self.error {
            return Err(io::Error::other(error.clone()));
        }
        let mut categories = resource_accounting::snapshot();
        if phase == "end" {
            for row in &self.end_categories {
                categories.retain(|existing| existing.id != row.id || existing.scope != row.scope);
                categories.push(row.clone());
            }
        }
        let snapshot = Snapshot {
            schema: "rinzu.resource-report",
            version: 1,
            producer: &self.producer,
            process_id: std::process::id(),
            sequence: self.sequence,
            phase,
            elapsed_ns: u64::try_from(self.started.elapsed().as_nanos())
                .map_err(|_| io::Error::other("resource report elapsed time overflow"))?,
            consistency: "subsystem-consistent; cross-subsystem-best-effort",
            sources: &self.sources,
            categories,
        };
        let result = (|| {
            serde_json::to_writer(&mut self.file, &snapshot)?;
            self.file.write_all(b"\n")?;
            self.file.flush()
        })();
        if let Err(error) = &result {
            self.error = Some(error.to_string());
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("resource report sequence overflow"))?;
        result
    }
}
pub struct Session {
    writer: Arc<Mutex<Writer>>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    sampler: Option<JoinHandle<()>>,
    previously_enabled: bool,
}
impl Session {
    pub fn start(
        config: &Config,
        producer: &str,
        sources: BTreeMap<u64, String>,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&config.path)?;
        let writer = Arc::new(Mutex::new(Writer {
            file,
            started: Instant::now(),
            sequence: 0,
            producer: producer.to_owned(),
            sources,
            end_categories: Vec::new(),
            error: None,
        }));
        let previously_enabled = resource_accounting::enabled();
        resource_accounting::enable();
        if let Err(error) = writer.lock().unwrap().snapshot("start") {
            if !previously_enabled {
                resource_accounting::disable();
            }
            return Err(error);
        }
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let mut session = Self {
            writer,
            stop,
            sampler: None,
            previously_enabled,
        };
        if config.interval_ms != 0 {
            let writer = Arc::clone(&session.writer);
            let stop = Arc::clone(&session.stop);
            let interval = Duration::from_millis(config.interval_ms);
            session.sampler = Some(
                std::thread::Builder::new()
                    .name("rinzu-resources".into())
                    .spawn(move || {
                        loop {
                            let guard = stop.0.lock().unwrap();
                            let (guard, _) = stop
                                .1
                                .wait_timeout_while(guard, interval, |stopped| !*stopped)
                                .unwrap();
                            if *guard {
                                break;
                            }
                            drop(guard);
                            if writer.lock().unwrap().snapshot("sample").is_err() {
                                break;
                            }
                        }
                    })?,
            );
        }
        Ok(session)
    }
    pub fn end_categories(&mut self, rows: Vec<Category>) {
        self.writer.lock().unwrap().end_categories = rows;
    }
    fn stop_sampler(&mut self) -> io::Result<()> {
        *self.stop.0.lock().unwrap() = true;
        self.stop.1.notify_one();
        if let Some(sampler) = self.sampler.take() {
            sampler
                .join()
                .map_err(|_| io::Error::other("resource sampler panicked"))?;
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<()> {
        self.stop_sampler()?;
        self.writer.lock().unwrap().snapshot("end")
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop_sampler();
        if !self.previously_enabled {
            resource_accounting::disable();
        }
    }
}
