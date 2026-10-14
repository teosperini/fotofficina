//! The versions strip of the Edit panel (a Fotofficina addition): when the photo being edited is in a stack, one
//! small button per member, so the original and its RawRefinery results can be switched without leaving Edit.
//!
//! Every member has its own edits; the one with the highlighted button is the photo the panel edits and shows.

use std::path::Path;

use egui::{Margin, RichText};
use lightcraft_catalog::{PhotoId, Source};
use serde_json::json;

use crate::LightcraftApp;
use crate::widgets::text_button;

/// What to call a member: a RawRefinery result (`NAME_denoised.dng`, in `Denoised` or `Denoised (Light)`...) says
/// which model made it, the raw it came from is the Original, anything else keeps its file name.
fn label(app: &LightcraftApp, id: PhotoId, has_result: bool) -> String {
    let Some(photo) = app.session.catalog.photo(id) else { return String::new() };
    let path = match &photo.source {
        Source::File { path } => Path::new(path),
        _ => return photo.file_name.clone(),
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if stem.ends_with("_denoised") {
        let folder = path.parent().and_then(Path::file_name).map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        return match folder.strip_prefix("Denoised (").and_then(|rest| rest.strip_suffix(')')) {
            Some(model) => format!("{} · {model}", crate::i18n::tr("Denoised")),
            None => crate::i18n::tr("Denoised").to_string(),
        };
    }
    if has_result { crate::i18n::tr("Original").to_string() } else { photo.file_name.clone() }
}

fn is_result(app: &LightcraftApp, id: PhotoId) -> bool {
    match app.session.catalog.photo(id).map(|p| &p.source) {
        Some(Source::File { path }) => Path::new(path).file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().ends_with("_denoised")),
        _ => false,
    }
}

/// Nothing for a photo that is not stacked with others.
pub(crate) fn strip(app: &mut LightcraftApp, ui: &mut egui::Ui, active: PhotoId) {
    let Some(members) = app.session.catalog.stack_of(active).map(|s| s.photos.clone()).filter(|m| m.len() > 1) else { return };
    let has_result = members.iter().any(|m| is_result(app, *m));
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut switch_to = None;
    egui::Frame::NONE.inner_margin(Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
        ui.label(RichText::new(crate::i18n::tr("Versions")).font(t.font(12.0)).color(t.text_dim))
            .on_hover_text(crate::i18n::tr("The photos of this stack. Each one has its own edits; the highlighted one is being edited."));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (i, member) in members.iter().enumerate() {
                let name = label(app, *member, has_result);
                let r = text_button(ui, &format!("labelSet-version-{i}"), &name, *member == active)
                    .on_hover_text(app.session.catalog.photo(*member).map(|p| p.file_name.clone()).unwrap_or_default());
                if r.clicked() && *member != active {
                    switch_to = Some(*member);
                }
            }
        });
    });
    if let Some(id) = switch_to {
        let _ = app.run("library.select", json!({"ids": [id.0], "active": id.0}));
    }
}
