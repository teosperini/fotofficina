//! Denoise with RawRefinery (a Fotofficina addition): the button in Edit ▸ Detail, and the check every frame that
//! applies the finished results (import, stack, settings; the engine side is `cmd/rawrefinery.rs`).
//!
//! The same command is in the Photo menu (it works on the whole selection). Progress is the one row of the
//! activity stack; what happened at the end is a toast.

use std::time::Duration;

use egui::Margin;
use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::widgets::register;

/// How long the end-of-run message stays (it can say what went wrong).
const TOAST_SECONDS: f64 = 8.0;

/// Which model the button uses: an index into the engine's `MODELS`, remembered between runs of the app.
fn model_slot() -> egui::Id {
    egui::Id::new("rawrefinery:model")
}

/// The entry after RawRefinery's models: the AI Denoise built into the app (RawNIND), which changes no file: it works
/// on the photo itself with its Amount slider. Free to use; its weights are GPL-3.0, so they are not bundled: the user
/// downloads them from Settings ▸ AI Denoise after accepting the terms.
const BUILT_IN: (&str, &str) = ("RawNIND", "Built in, free: no new file, use Amount. Downloaded on request (GPL)");

/// Number of choices in the menu: RawRefinery's models, then the built-in one.
fn choices() -> usize {
    lightcraft_engine::rawrefinery::MODELS.len() + 1
}

fn is_built_in(index: usize) -> bool {
    index >= lightcraft_engine::rawrefinery::MODELS.len()
}

fn label_of(index: usize) -> (&'static str, &'static str) {
    match lightcraft_engine::rawrefinery::MODELS.get(index) {
        Some((_, label, what)) => (label, what),
        None => BUILT_IN,
    }
}

fn chosen_model(ctx: &egui::Context) -> usize {
    ctx.data_mut(|d| d.get_persisted::<usize>(model_slot())).filter(|i| *i < choices()).unwrap_or(0)
}

/// The model menu and the button under AI Denoise: denoise this photo's original raw with RawRefinery.
pub(crate) fn detail_button(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    egui::Frame::NONE.inner_margin(Margin { left: 24, right: 22, top: 2, bottom: 6 }).show(ui, |ui| {
        let mut index = chosen_model(ui.ctx());
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Model"));
            egui::ComboBox::from_id_salt("rawrefinery-model").width(120.0).selected_text(crate::i18n::tr(label_of(index).0)).show_ui(ui, |ui| {
                for i in 0..choices() {
                    let (label, what) = label_of(i);
                    ui.selectable_value(&mut index, i, format!("{} — {}", crate::i18n::tr(label), crate::i18n::tr(what)));
                }
            });
        });
        ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tr(label_of(index).1)).small().weak()).wrap());
        ui.ctx().data_mut(|d| d.insert_persisted(model_slot(), index));
        let built_in = is_built_in(index);
        let working = !built_in && lightcraft_engine::rawrefinery::workers() > 0;
        let applicable = !built_in || !matches!(app.session.denoise_photo_state(id), lightcraft_engine::denoise::PhotoState::NotApplicable);
        let (text, hover) = if built_in {
            (
                "Denoise with RawNIND…",
                "Turn on the AI Denoise built into the app for this photo and set its Amount under Detail. No new file: the raw is untouched. The first time, the model is downloaded from Settings after you accept its terms.",
            )
        } else {
            (
                "Denoise with RawRefinery…",
                "Denoise the original raw with RawRefinery, a separate program on this computer. The result is a new DNG next to the original, stacked with it.",
            )
        };
        let r = ui.add_enabled(!working && applicable, egui::Button::new(crate::i18n::tr(text)).small()).on_hover_text(crate::i18n::tr(hover));
        register(ui.ctx(), "rawrefinery:denoise", r.rect);
        if r.clicked() {
            start(app, ui.ctx(), id);
        }
    });
}

fn start(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId) {
    if is_built_in(chosen_model(ctx)) {
        // the built-in AI Denoise: its own toggle opens Settings to install the model when there is none. It starts at
        // 70 (the app's own start is 50; 100 leaves too smooth, banded gradients next to Lightroom's): the slider is under Detail
        let amount = app.session.catalog.photo(id).map_or(0.0, |p| p.develop.enhance.denoise);
        if amount <= 0.0 {
            let _ = app.run("develop.set", json!({"ids": [id.0], "values": {"enhance.denoise": 70}}));
        }
        if let Err(e) = app.run("denoise.toggle", json!({"id": id.0, "enabled": true})) {
            app.toast_for(ctx, e, TOAST_SECONDS);
        }
        return;
    }
    match app.run(
        "photo.rawrefineryDenoise",
        json!({"ids": [id.0], "model": lightcraft_engine::rawrefinery::MODELS.get(chosen_model(ctx)).map_or("", |m| m.0)}),
    ) {
        Ok(v) if v["started"].as_u64() == Some(0) => {
            let why = v["skipped"][0][1].as_str().unwrap_or("nothing to denoise");
            app.toast_for(ctx, format!("RawRefinery: {why}"), TOAST_SECONDS);
        }
        Ok(_) => app.toast(ctx, "Denoising with RawRefinery…"),
        Err(e) => app.toast_for(ctx, e, TOAST_SECONDS),
    }
}

/// Called every frame: while a RawRefinery worker runs or results wait, apply them and say how it went.
pub(crate) fn pump(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !lightcraft_engine::rawrefinery::busy() {
        return;
    }
    let Ok(report) = app.session.execute("rawrefinery.pump", &json!({})) else { return };
    let count = |key: &str| report[key].as_array().map_or(0, Vec::len);
    let (made, failed, skipped) = (count("made"), count("failed"), count("skipped"));
    if made > 0 {
        let text = if made == 1 {
            "Denoised with RawRefinery: the result is stacked with the original".to_string()
        } else {
            format!("Denoised {made} photos with RawRefinery: each result is stacked with its original")
        };
        app.toast_for(ctx, text, TOAST_SECONDS);
    }
    if failed > 0 {
        let message = report["failed"][0][1].as_str().unwrap_or("it failed");
        log::warn!("RawRefinery: {message}");
        app.toast_for(ctx, format!("RawRefinery: {message}"), TOAST_SECONDS);
    } else if made == 0 && skipped > 0 {
        app.toast_for(ctx, format!("RawRefinery: {}", reason(&report)), TOAST_SECONDS);
    }
    if made > 0 {
        ctx.request_repaint();
    }
    if report["running"] == true || lightcraft_engine::rawrefinery::busy() {
        ctx.request_repaint_after(Duration::from_millis(300));
    }
}

fn reason(report: &Value) -> String {
    report["skipped"][0][1].as_str().unwrap_or("skipped").to_string()
}
