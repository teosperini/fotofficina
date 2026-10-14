//! Denoise with RawRefinery (a Fotofficina addition, not part of LightCraft).
//!
//! RawRefinery is a separate program (Python + PyTorch) with a command line, `rawrefinery-cli`. This module
//! finds it, runs it on a photo's original raw file, follows its JSON progress lines and queues what it made
//! for the session to import, stack with the original and carry the develop settings over
//! (see `cmd/rawrefinery.rs`). The original is never touched: the result is a new `NAME_denoised.dng`.
//!
//! The raw never leaves the machine and nothing is downloaded here; RawRefinery fetches its own model the first
//! time it runs. Everything runs on a worker thread; the UI thread only applies the finished results.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use lightcraft_catalog::PhotoId;
use serde_json::Value;

/// Environment variable with the path of `rawrefinery-cli`.
pub const CLI_ENV: &str = "FOTOFFICINA_RAWREFINERY_CLI";
/// Settings file in the config folder: `{"cli": "/path/to/rawrefinery-cli"}`.
pub const CONFIG_FILE: &str = "rawrefinery.json";
const CLI_NAME: &str = "rawrefinery-cli";
/// The folder, next to the original, that holds the denoised files.
pub const DEFAULT_OUT_FOLDER: &str = "Denoised";

/// The denoise models of RawRefinery: (name the program knows, short label, what changes). The first is the default.
pub const MODELS: &[(&str, &str, &str)] = &[
    ("Tree Net Denoise", "Standard", "Best quality, slowest"),
    ("Tree Net Denoise Light", "Light", "Faster, a little softer"),
    ("Tree Net Denoise Super Light", "Super Light", "Fastest, keeps most of the detail"),
];

/// The folder for the results of `model` (`None` or the default model: [`DEFAULT_OUT_FOLDER`]). Each model has its
/// own folder, because the program never overwrites a result: with one shared folder, trying another model would
/// silently give back the first model's file.
pub fn out_folder(model: Option<&str>) -> String {
    match MODELS.iter().skip(1).find(|(name, _, _)| Some(*name) == model) {
        Some((_, label, _)) => format!("{DEFAULT_OUT_FOLDER} ({label})"),
        None => DEFAULT_OUT_FOLDER.to_string(),
    }
}

/// How long a message of the program's stderr is kept when explaining a failure.
const MAX_LINE: usize = 300;

/// Where `rawrefinery-cli` is: the explicit path, then [`CLI_ENV`], then `rawrefinery.json` in the config folder,
/// then the `PATH`. (An app started from the Dock has no shell `PATH`, hence the settings file.)
pub fn find_cli(explicit: Option<&str>, config_dir: Option<&Path>) -> Result<PathBuf, String> {
    let file = |p: &str| -> Option<PathBuf> {
        let path = PathBuf::from(p);
        path.is_file().then_some(path)
    };
    if let Some(p) = explicit.filter(|p| !p.is_empty()) {
        return file(p).ok_or_else(|| format!("rawrefinery-cli not found at {p}"));
    }
    if let Some(p) = std::env::var(CLI_ENV).ok().filter(|p| !p.is_empty()) {
        return file(&p).ok_or_else(|| format!("{CLI_ENV} points to {p}, which is not a file"));
    }
    if let Some(dir) = config_dir {
        let config = dir.join(CONFIG_FILE);
        if let Ok(text) = std::fs::read_to_string(&config)
            && let Some(p) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["cli"].as_str().map(str::to_string))
        {
            return file(&p).ok_or_else(|| format!("{}: \"cli\" points to {p}, which is not a file", config.display()));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(CLI_NAME);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(format!(
        "RawRefinery isn't set up: put the path of {CLI_NAME} in {CLI_ENV}, or in {CONFIG_FILE} ({{\"cli\": \"/path/to/{CLI_NAME}\"}}) in the settings folder"
    ))
}

/// One line of the program's `--json` output.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start { file: String, index: u64, total: u64 },
    Progress { file: String, fraction: f64 },
    Done { file: String, output: String },
    Skipped { file: String, output: Option<String>, reason: String },
    Error { file: Option<String>, message: String },
    Finished { done: u64, skipped: u64, failed: u64 },
}

/// Parse one output line; anything that isn't a known event (a stray log line) is `None`.
pub fn parse_event(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let count = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    match v.get("event")?.as_str()? {
        "start" => Some(Event::Start { file: text("file")?, index: count("index"), total: count("total") }),
        "progress" => Some(Event::Progress { file: text("file")?, fraction: v.get("fraction").and_then(Value::as_f64).unwrap_or(0.0) }),
        "done" => Some(Event::Done { file: text("file")?, output: text("output")? }),
        "skipped" => Some(Event::Skipped { file: text("file")?, output: text("output"), reason: text("reason").unwrap_or_default() }),
        "error" => Some(Event::Error { file: text("file"), message: text("message").unwrap_or_default() }),
        "finished" => Some(Event::Finished { done: count("done"), skipped: count("skipped"), failed: count("failed") }),
        _ => None,
    }
}

/// What to run: the raw files, where the results go and the optional settings of the program.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub model: Option<String>,
    /// `None` = each photo's own ISO.
    pub iso: Option<u32>,
    /// Percent of the original mixed back in (0 = fully denoised).
    pub blend: Option<u32>,
    pub device: Option<String>,
}

/// Run `rawrefinery-cli` and hand every event to `on_event` as it arrives. Stops the program when `cancelled()` turns
/// true (`Err("cancelled")`). An error means the program could not do its job at all (it didn't start, or ended
/// without saying it had finished); a failure of one photo arrives as an [`Event::Error`] and is not an `Err`.
pub fn run(cli: &Path, req: &Request, cancelled: &dyn Fn() -> bool, on_event: &mut dyn FnMut(Event)) -> Result<(), String> {
    let mut command = Command::new(cli);
    command.args(&req.inputs).arg("--out").arg(&req.out_dir).arg("--json");
    if let Some(m) = &req.model {
        command.arg("--model").arg(m);
    }
    if let Some(i) = req.iso {
        command.arg("--iso").arg(i.to_string());
    }
    if let Some(b) = req.blend {
        command.arg("--blend").arg(b.to_string());
    }
    if let Some(d) = &req.device {
        command.arg("--device").arg(d);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("could not start {}: {e}", cli.display()))?;
    let stdout = child.stdout.take().ok_or("could not read the output of RawRefinery")?;
    let stderr = child.stderr.take();

    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let errors = std::thread::spawn(move || stderr_tail(stderr));

    let mut finished = false;
    let mut stop = None;
    loop {
        if cancelled() {
            stop = Some("cancelled".to_string());
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if let Some(event) = parse_event(&line) {
                    finished |= matches!(event, Event::Finished { .. });
                    on_event(event);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| e.to_string());
    if let Some(reason) = stop {
        // the reader threads end by themselves when the pipes close; a child of the program may keep
        // them open for a while, so don't wait for it
        return Err(reason);
    }
    let _ = reader.join();
    let tail = errors.join().unwrap_or_default();
    let status = status?;
    if !finished && !status.success() {
        let why = tail.last().cloned().unwrap_or_else(|| format!("it ended with {status}"));
        return Err(format!("RawRefinery stopped: {why}"));
    }
    Ok(())
}

/// The last lines of the program's error output, each capped (to explain a failure).
fn stderr_tail(stderr: Option<impl Read>) -> Vec<String> {
    let Some(stderr) = stderr else { return Vec::new() };
    let mut last: VecDeque<String> = VecDeque::new();
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if last.len() == 20 {
            last.pop_front();
        }
        last.push_back(line.chars().take(MAX_LINE).collect());
    }
    last.into_iter().collect()
}

// ---- what the worker hands to the session

/// How one photo ended; the session applies these on its own thread (`rawrefinery.pump`).
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// `existing`: the file was already there from an earlier run, nothing was made now.
    Made {
        source: PhotoId,
        output: PathBuf,
        existing: bool,
    },
    Skipped {
        source: PhotoId,
        reason: String,
    },
    Failed {
        source: Option<PhotoId>,
        message: String,
    },
}

static QUEUE: OnceLock<Mutex<Vec<Outcome>>> = OnceLock::new();
static RUNNING: AtomicUsize = AtomicUsize::new(0);

fn queue() -> std::sync::MutexGuard<'static, Vec<Outcome>> {
    QUEUE.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn push(outcome: Outcome) {
    queue().push(outcome);
}

/// Everything the workers finished since the last call.
pub fn take_outcomes() -> Vec<Outcome> {
    std::mem::take(&mut *queue())
}

/// True while a worker runs or results wait to be applied (the UI polls `rawrefinery.pump` meanwhile).
pub fn busy() -> bool {
    RUNNING.load(Ordering::Acquire) > 0 || !queue().is_empty()
}

/// How many workers are running.
pub fn workers() -> usize {
    RUNNING.load(Ordering::Acquire)
}

/// Counts a running worker; dropping it (also when the worker panics) ends it.
pub struct Running(());

impl Running {
    pub fn start() -> Running {
        RUNNING.fetch_add(1, Ordering::AcqRel);
        Running(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn each_model_has_its_own_results_folder() {
        assert_eq!(super::out_folder(None), "Denoised");
        assert_eq!(super::out_folder(Some("Tree Net Denoise")), "Denoised");
        assert_eq!(super::out_folder(Some("Tree Net Denoise Light")), "Denoised (Light)");
        assert_eq!(super::out_folder(Some("Tree Net Denoise Super Light")), "Denoised (Super Light)");
        assert!(super::MODELS.iter().all(|(name, _, _)| name.starts_with("Tree Net Denoise")));
    }

    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn fake_cli(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-cli");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn request() -> Request {
        Request { inputs: vec![PathBuf::from("/photos/a.CR2")], out_dir: PathBuf::from("/photos/Denoised"), ..Default::default() }
    }

    #[test]
    fn events_are_parsed_and_noise_is_ignored() {
        assert_eq!(
            parse_event(r#"{"event": "start", "file": "a.CR2", "index": 1, "total": 3, "output": "x"}"#),
            Some(Event::Start { file: "a.CR2".into(), index: 1, total: 3 })
        );
        assert_eq!(
            parse_event(r#"{"event":"done","file":"a","output":"/o/a_denoised.dng","seconds":1.5}"#),
            Some(Event::Done { file: "a".into(), output: "/o/a_denoised.dng".into() })
        );
        assert_eq!(
            parse_event(r#"{"event":"skipped","file":"a","reason":"output already exists","output":"/o/a.dng"}"#),
            Some(Event::Skipped { file: "a".into(), output: Some("/o/a.dng".into()), reason: "output already exists".into() })
        );
        assert_eq!(parse_event(r#"{"event":"error","message":"boom"}"#), Some(Event::Error { file: None, message: "boom".into() }));
        assert_eq!(
            parse_event(r#"{"event":"finished","total":2,"done":1,"skipped":1,"failed":0}"#),
            Some(Event::Finished { done: 1, skipped: 1, failed: 0 })
        );
        for junk in ["", "Using Device mps", "[1, 2]", r#"{"event":"mystery"}"#, r#"{"event":"done"}"#, r#"{"no":"event"}"#] {
            assert_eq!(parse_event(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn the_program_is_found_in_the_documented_order() {
        let dir = std::env::temp_dir().join(format!("fotofficina-find-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = fake_cli(&dir, "true");
        let explicit = cli.to_string_lossy().to_string();
        assert_eq!(find_cli(Some(&explicit), None).unwrap(), cli);
        assert!(find_cli(Some("/no/such/file"), None).unwrap_err().contains("not found"));
        std::fs::write(dir.join(CONFIG_FILE), serde_json::json!({"cli": explicit}).to_string()).unwrap();
        if std::env::var_os(CLI_ENV).is_none() {
            assert_eq!(find_cli(None, Some(&dir)).unwrap(), cli);
        }
        std::fs::write(dir.join(CONFIG_FILE), r#"{"cli": "/no/such/file"}"#).unwrap();
        if std::env::var_os(CLI_ENV).is_none() {
            assert!(find_cli(None, Some(&dir)).unwrap_err().contains("not a file"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_reports_every_event_in_order() {
        let dir = std::env::temp_dir().join(format!("fotofficina-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = fake_cli(
            &dir,
            r#"echo 'chatter on stdout'
echo '{"event":"start","file":"/photos/a.CR2","index":1,"total":1,"output":"/photos/Denoised/a_denoised.dng"}'
echo '{"event":"done","file":"/photos/a.CR2","output":"/photos/Denoised/a_denoised.dng"}'
echo '{"event":"finished","total":1,"done":1,"skipped":0,"failed":0}'"#,
        );
        let mut events = Vec::new();
        run(&cli, &request(), &|| false, &mut |e| events.push(e)).unwrap();
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(events[1], Event::Done { .. }));
        assert!(matches!(events[2], Event::Finished { done: 1, .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_dies_without_finishing_is_an_error_with_its_reason() {
        let dir = std::env::temp_dir().join(format!("fotofficina-die-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = fake_cli(&dir, "echo 'ModuleNotFoundError: No module named torch' >&2\nexit 3");
        let e = run(&cli, &request(), &|| false, &mut |_| {}).unwrap_err();
        assert!(e.contains("ModuleNotFoundError"), "{e}");
        assert!(run(&dir.join("missing"), &request(), &|| false, &mut |_| {}).unwrap_err().contains("could not start"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_stops_the_program() {
        let dir = std::env::temp_dir().join(format!("fotofficina-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = fake_cli(&dir, "sleep 30");
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let e = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                cancel.store(true, Ordering::Relaxed);
            });
            run(&cli, &request(), &|| cancel.load(Ordering::Relaxed), &mut |_| {})
        })
        .unwrap_err();
        assert_eq!(e, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(10), "the sleeping program was killed, not waited for");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
