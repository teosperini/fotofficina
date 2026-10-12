//! Denoise with RawRefinery: the commands, with a stand-in for `rawrefinery-cli` (a small shell script that
//! speaks the same JSON lines), so no model or Python is needed.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;
use crate::tests_xmp::synthetic_dng_with;

/// The worker's results travel through one process-wide queue, so these tests take turns.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-rawrefinery-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A script that behaves like the program on success: for each input it copies `payload` to
/// `<out>/<stem>_denoised.dng` and reports it.
#[cfg(unix)]
fn fake_cli(dir: &Path, payload: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = r#"#!/bin/sh
out=""
inputs=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    --json) shift ;;
    --*) shift 2 ;;
    *) inputs="$inputs|$1"; shift ;;
  esac
done
mkdir -p "$out"
count=0
IFS='|'
for f in $inputs; do
  [ -z "$f" ] && continue
  name=$(basename "$f"); stem="${name%.*}"
  cp "PAYLOAD" "$out/${stem}_denoised.dng"
  echo "{\"event\":\"start\",\"file\":\"$f\",\"index\":1,\"total\":1,\"output\":\"$out/${stem}_denoised.dng\"}"
  echo "{\"event\":\"done\",\"file\":\"$f\",\"output\":\"$out/${stem}_denoised.dng\"}"
  count=$((count + 1))
done
echo "{\"event\":\"finished\",\"total\":$count,\"done\":$count,\"skipped\":0,\"failed\":0}"
"#
    .replace("PAYLOAD", &payload.to_string_lossy());
    let path = dir.join("fake-rawrefinery-cli");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A session with one imported raw (`shot.dng` in `dir`), edited, and the file the stand-in "denoises" it into.
#[cfg(unix)]
fn session_with_edited_raw(dir: &Path) -> (Session, lightcraft_catalog::PhotoId, PathBuf) {
    let raw = dir.join("shot.dng");
    std::fs::write(&raw, synthetic_dng_with(None, Default::default())).unwrap();
    // the "denoised" picture: the same raw with one sample changed, so it is a different file
    let mut payload = synthetic_dng_with(None, Default::default());
    if let Some(last) = payload.last_mut() {
        *last ^= 0x01;
    }
    let payload_path = dir.join("payload.dng.bin");
    std::fs::write(&payload_path, payload).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [raw.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
    s.execute("develop.set", &json!({"values": {"light.exposure": 1.0}})).unwrap();
    (s, id, payload_path)
}

#[cfg(unix)]
#[test]
fn denoise_imports_stacks_and_carries_the_settings_over() {
    let _turn = turn();
    let dir = temp_dir("flow");
    let (mut s, id, payload) = session_with_edited_raw(&dir);
    let cli = fake_cli(&dir, &payload);
    assert!(!crate::rawrefinery::busy());

    let r = s.execute("photo.rawrefineryDenoise", &json!({"ids": [id.0], "cli": cli.to_string_lossy()})).unwrap();
    assert_eq!(r["started"], 1, "{r}");
    let r = s.execute("rawrefinery.pump", &json!({"wait": true})).unwrap();
    assert_eq!(r["made"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(r["failed"], json!([]), "{r}");
    assert_eq!(r["running"], false);
    let new = lightcraft_catalog::PhotoId(r["made"][0]["id"].as_u64().unwrap());
    assert_eq!(r["made"][0]["original"], id.0);

    // the result is a new file next to the original, in a Denoised folder; the original is untouched
    let out = dir.join("Denoised/shot_denoised.dng");
    assert!(out.is_file(), "{out:?}");
    assert!(dir.join("shot.dng").is_file());
    assert_eq!(s.catalog.photos().count(), 2);
    // ...stacked on top of its original
    let stack = s.catalog.stack_of(new).map(|st| st.photos.clone()).unwrap_or_default();
    assert_eq!(stack, vec![new, id], "the denoised photo tops the stack");
    // ...with the original's edits
    assert_eq!(s.develop_of(new).unwrap().light.exposure, 1.0);
    // nothing is left running or waiting
    assert!(!crate::rawrefinery::busy());
    assert!(s.activity.list().is_empty(), "{:?}", s.activity.list());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn only_original_raws_are_denoised_and_a_missing_program_is_explained() {
    let _turn = turn();
    let dir = temp_dir("refuse");
    let (mut s, id, _payload) = session_with_edited_raw(&dir);

    // a result of an earlier run is not denoised again
    let again = dir.join("shot_denoised.dng");
    let mut bytes = synthetic_dng_with(None, Default::default());
    let middle = bytes.len() / 2;
    if let Some(byte) = bytes.get_mut(middle) {
        *byte ^= 0x04;
    }
    std::fs::write(&again, bytes).unwrap();
    s.execute("library.import", &json!({"paths": [again.to_string_lossy()]})).unwrap();
    let twin = s.catalog.photos().find(|p| p.file_name == "shot_denoised.dng").map(|p| p.id).unwrap();
    let r = s.execute("photo.rawrefineryDenoise", &json!({"ids": [twin.0], "cli": "/no/such/program"})).unwrap();
    assert_eq!(r["started"], 0, "{r}");
    assert_eq!(r["skipped"][0][1], "already denoised", "{r}");

    // a raw that can be denoised, but no program: a message that says what to do
    let e = s.execute("photo.rawrefineryDenoise", &json!({"ids": [id.0], "cli": "/no/such/program"})).unwrap_err().to_string();
    assert!(e.contains("not found"), "{e}");
    assert!(!crate::rawrefinery::busy(), "nothing was started");
    let r: Value = s.execute("rawrefinery.pump", &json!({})).unwrap();
    assert_eq!(r["made"], json!([]));
    let _ = std::fs::remove_dir_all(&dir);
}
