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

fn chosen_model(ctx: &egui::Context) -> usize {
    let n = lightcraft_engine::rawrefinery::MODELS.len();
    ctx.data_mut(|d| d.get_persisted::<usize>(model_slot())).filter(|i| *i < n).unwrap_or(0)
}

/// The model menu and the button under AI Denoise: denoise this photo's original raw with RawRefinery.
pub(crate) fn detail_button(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    egui::Frame::NONE.inner_margin(Margin { left: 24, right: 22, top: 2, bottom: 6 }).show(ui, |ui| {
        let models = lightcraft_engine::rawrefinery::MODELS;
        let mut index = chosen_model(ui.ctx());
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Model"));
            egui::ComboBox::from_id_salt("rawrefinery-model").width(120.0).selected_text(crate::i18n::tr(models[index].1)).show_ui(ui, |ui| {
                for (i, (_, label, what)) in models.iter().enumerate() {
                    ui.selectable_value(&mut index, i, format!("{} — {}", crate::i18n::tr(label), crate::i18n::tr(what)));
                }
            });
        });
        ui.label(egui::RichText::new(crate::i18n::tr(models[index].2)).small().weak());
        ui.ctx().data_mut(|d| d.insert_persisted(model_slot(), index));
        let working = lightcraft_engine::rawrefinery::workers() > 0;
        let r = ui
            .add_enabled(!working, egui::Button::new(crate::i18n::tr("Denoise with RawRefinery…")).small())
            .on_hover_text(crate::i18n::tr(
                "Denoise the original raw with RawRefinery, a separate program on this computer. The result is a new DNG next to the original, stacked with it.",
            ));
        register(ui.ctx(), "rawrefinery:denoise", r.rect);
        if r.clicked() {
            start(app, ui.ctx(), id);
        }
    });
}

fn start(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId) {
    match app.run("photo.rawrefineryDenoise", json!({"ids": [id.0], "model": lightcraft_engine::rawrefinery::MODELS[chosen_model(ctx)].0})) {
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
