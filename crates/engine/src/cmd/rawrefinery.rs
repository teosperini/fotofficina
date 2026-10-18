//! Denoise with RawRefinery (`photo.rawrefineryDenoise`, `rawrefinery.pump`): a Fotofficina addition.
//!
//! The first command starts `rawrefinery-cli` on the original raw files of the selected photos, on a worker thread
//! (a row in the activity stack). The second applies what the worker finished: the new `NAME_denoised.dng` is
//! imported (in place), stacked on top of its original, and the original's develop settings are carried over. The
//! UI calls the pump every frame while [`crate::rawrefinery::busy`]; scripts call it with `{wait: true}`.
//! Originals are never changed or deleted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_catalog::{MediaKind, PhotoId, Source};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection, str_param};
use crate::activity::{Cancel, TaskGuard};
use crate::rawrefinery::{self as rr, Event, Outcome, Request};
use crate::{Result, Session};

const LABEL: &str = "Denoise with RawRefinery";

/// One raw file to denoise and where its result goes.
struct Job {
    source: PhotoId,
    input: PathBuf,
    out_dir: PathBuf,
}

/// Why a photo can't be denoised, or `None` when it can.
fn refusal(ph: &lightcraft_catalog::Photo) -> Option<&'static str> {
    if ph.deleted {
        return Some("deleted");
    }
    if ph.copy_of.is_some() {
        return Some("a virtual copy: denoise the original");
    }
    if ph.kind != MediaKind::Raw {
        return Some("not a raw photo");
    }
    let Source::File { path } = &ph.source else { return Some("not a file") };
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if stem.ends_with("_denoised") {
        return Some("already denoised");
    }
    None
}

fn start(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.rawrefineryDenoise";
    let explicit_dir = str_param(p, "dir").filter(|d| !d.is_empty()).map(PathBuf::from);
    let model = str_param(p, "model").filter(|m| !m.is_empty());
    let folder = rr::out_folder(model);
    let mut jobs = Vec::new();
    let mut skipped = Vec::new();
    for id in s.targets(p)? {
        let Some(ph) = s.catalog.photo(id) else { continue };
        if let Some(why) = refusal(ph) {
            skipped.push(json!([id.0, why]));
            continue;
        }
        let Source::File { path } = &ph.source else { continue };
        let input = PathBuf::from(path);
        let out_dir = explicit_dir.clone().unwrap_or_else(|| input.parent().unwrap_or(Path::new("")).join(&folder));
        jobs.push(Job { source: id, input, out_dir });
    }
    if jobs.is_empty() {
        return Ok(json!({"started": 0, "skipped": skipped}));
    }
    let cli = rr::find_cli(str_param(p, "cli"), crate::config::config_dir().as_deref()).map_err(|e| bad(C, e))?;
    let number = |key: &str| p.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
    let blend = number("blend");
    if blend.is_some_and(|b| b > 100) {
        return Err(bad(C, "blend is a percentage from 0 to 100"));
    }
    let template = Request {
        model: model.map(str::to_string),
        iso: number("iso"),
        blend,
        device: str_param(p, "device").filter(|d| !d.is_empty()).map(str::to_string),
        ..Default::default()
    };

    // one program run per output folder, in the order the photos came
    let mut groups: Vec<(PathBuf, Vec<Job>)> = Vec::new();
    for job in jobs {
        match groups.iter_mut().find(|(dir, _)| *dir == job.out_dir) {
            Some((_, list)) => list.push(job),
            None => groups.push((job.out_dir.clone(), vec![job])),
        }
    }
    let total: usize = groups.iter().map(|(_, list)| list.len()).sum();
    let dirs: Vec<String> = groups.iter().map(|(dir, _)| dir.to_string_lossy().to_string()).collect();
    let task = s.activity.start("rawrefinery", "Denoising with RawRefinery", Cancel::Flag(Arc::new(AtomicBool::new(false))));
    task.progress(0, total as u64);
    let running = rr::Running::start();
    let spawned = std::thread::Builder::new().name("rawrefinery".into()).spawn(move || {
        let _running = running; // counted until the worker ends, also if it panics
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&cli, groups, &template, &task)));
        if outcome.is_err() {
            rr::push(Outcome::Failed { source: None, message: "RawRefinery stopped unexpectedly".into() });
        }
    });
    if let Err(e) = spawned {
        return Err(bad(C, format!("could not start the worker: {e}")));
    }
    Ok(json!({"started": total, "skipped": skipped, "dirs": dirs}))
}

/// The worker: runs the program for each output folder and queues one outcome per photo.
fn work(cli: &Path, groups: Vec<(PathBuf, Vec<Job>)>, template: &Request, task: &TaskGuard) {
    let total: usize = groups.iter().map(|(_, list)| list.len()).sum();
    let mut finished = 0usize;
    for (out_dir, jobs) in groups {
        let mut by_input: HashMap<PathBuf, PhotoId> = jobs.iter().map(|j| (j.input.clone(), j.source)).collect();
        let request = Request { inputs: jobs.iter().map(|j| j.input.clone()).collect(), out_dir, ..template.clone() };
        let mut settle = |file: &str, make: &dyn Fn(PhotoId) -> Outcome| {
            if let Some(source) = by_input.remove(Path::new(file)) {
                rr::push(make(source));
                finished += 1;
                task.progress(finished as u64, total as u64);
            }
        };
        let result = rr::run(cli, &request, &|| task.is_cancelled(), &mut |event| match event {
            Event::Start { file, .. } => task.detail(&Path::new(&file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(file)),
            Event::Done { file, output } => {
                // the result gets its original's look before it is imported (see `rawrefinery::match_look`)
                if let Some(what) = rr::match_look(Path::new(&file), Path::new(&output)) {
                    log::info!("RawRefinery: {output} given the look of {file}: {what}");
                }
                settle(&file, &|source| Outcome::Made { source, output: PathBuf::from(&output), existing: false })
            }
            Event::Skipped { file, output: Some(output), .. } => {
                settle(&file, &|source| Outcome::Made { source, output: PathBuf::from(&output), existing: true })
            }
            Event::Skipped { file, reason, .. } => settle(&file, &|source| Outcome::Skipped { source, reason: reason.clone() }),
            Event::Error { file: Some(file), message } => settle(&file, &|source| Outcome::Failed { source: Some(source), message: message.clone() }),
            Event::Error { file: None, message } => rr::push(Outcome::Failed { source: None, message }),
            Event::Progress { .. } | Event::Finished { .. } => {}
        });
        // photos the program never reported: it stopped, or the file names didn't match
        let why = match result {
            Ok(()) => "RawRefinery gave no result for this photo".to_string(),
            Err(e) => e,
        };
        for source in by_input.into_values() {
            let outcome = if why == "cancelled" {
                Outcome::Skipped { source, reason: why.clone() }
            } else {
                Outcome::Failed { source: Some(source), message: why.clone() }
            };
            rr::push(outcome);
        }
        if why == "cancelled" {
            return;
        }
    }
}

/// Import a finished file; if it is in the library already (an earlier run), find that photo.
fn import_one(s: &mut Session, path: &str) -> std::result::Result<PhotoId, String> {
    let r = s.execute("library.import", &json!({"paths": [path]})).map_err(|e| e.to_string())?;
    if let Some(id) = r["imported"].get(0).and_then(Value::as_u64) {
        return Ok(PhotoId(id));
    }
    s.catalog
        .photos()
        .find(|p| matches!(&p.source, Source::File { path: q } if q == path))
        .map(|p| p.id)
        .ok_or_else(|| format!("{path} could not be added to the library"))
}

fn apply(s: &mut Session, outcome: Outcome, report: &mut Value) {
    let push = |report: &mut Value, key: &str, item: Value| {
        if let Some(list) = report.get_mut(key).and_then(Value::as_array_mut) {
            list.push(item);
        }
    };
    match outcome {
        Outcome::Made { source, output, existing } => {
            let path = output.to_string_lossy().to_string();
            match import_one(s, &path) {
                Ok(new) => {
                    // the denoised file on top of its original, expanded so both show
                    let _ = s.execute("stack.group", &json!({"ids": [new.0, source.0], "top": new.0, "collapsed": false}));
                    // an edited original hands its settings on; an untouched one leaves the import look alone
                    if s.catalog.photo(source).is_some_and(|p| p.is_edited())
                        && let Some(settings) = s.develop_of(source)
                        && let Some(op) = s.develop_op(new, (*settings).clone(), LABEL)
                    {
                        let _ = s.commit(LABEL, op);
                    }
                    push(report, "made", json!({"id": new.0, "original": source.0, "path": path, "existing": existing}));
                }
                Err(e) => push(report, "failed", json!([source.0, e])),
            }
        }
        Outcome::Skipped { source, reason } => push(report, "skipped", json!([source.0, reason])),
        Outcome::Failed { source, message } => push(report, "failed", json!([source.map(|id| id.0), message])),
    }
}

fn pump(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("wait").and_then(Value::as_bool) == Some(true) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60 * 60);
        while rr::workers() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let mut report = json!({"made": [], "skipped": [], "failed": []});
    for outcome in rr::take_outcomes() {
        apply(s, outcome, &mut report);
    }
    report["running"] = json!(rr::workers() > 0);
    Ok(report)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.rawrefineryDenoise",
            "Denoise with RawRefinery…",
            ["Photo"],
            None,
            "{ids?, dir?, model?, iso?, blend?, device?, cli?} → {started, skipped: [[id, why]], dirs} — denoise the original raw of each selected photo with RawRefinery (separate program, `rawrefinery-cli`, found via `cli`, FOTOFFICINA_RAWREFINERY_CLI, rawrefinery.json in the settings folder or the PATH) in the background. Each result is a new NAME_denoised.dng in `dir` (default: a Denoised folder next to the original (Denoised (Light) / Denoised (Super Light) for the other models)); the original is never changed. Finished files are imported, stacked on top of the original and given its settings by rawrefinery.pump. iso: noise level (default: each photo's own); blend: percent of the original mixed back in (0–100)",
            has_selection,
            start
        ),
        cmd!(
            query "rawrefinery.pump",
            "RawRefinery Results",
            [],
            None,
            "{wait?: true} → {made: [{id, original, path, existing}], skipped: [[id, why]], failed: [[id|null, message]], running} — apply what the RawRefinery worker finished since the last call (import, stack, settings); wait: true first waits for the worker to end (scripts, CLI, tests)",
            always,
            pump
        ),
    ]
}
