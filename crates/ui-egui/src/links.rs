//! Project links (Help menu, About dialog).

/// The app's name in repository URLs.
pub const APP: &str = "fotofficina";
/// This app's page (its repository).
pub const WEBSITE: &str = "https://github.com/teosperini/fotofficina";
/// This app's page (its repository).
pub const APP_PAGE: &str = WEBSITE;
/// This app's source repository.
pub const GITHUB: &str = WEBSITE;
/// The project this one is based on: LightCraft, by the ArtCraft team and contributors (credit, plain text).
pub const UPSTREAM: &str = "https://github.com/storytold/lightcraft";

/// (UI command id, menu label, URL) for each link, in Help-menu order.
/// The user documentation (docs/ in the repository).
pub const HELP: &str = "https://github.com/storytold/lightcraft/tree/main/docs";

pub const LINKS: &[(&str, &str, &str)] = &[
    ("app.help", "Fotofficina Help", HELP),
    ("app.website", "Fotofficina Website", APP_PAGE),
    ("app.github", "Fotofficina on GitHub", GITHUB),
    ("app.upstream", "Based on LightCraft…", UPSTREAM),
    ("app.feedback", "Send Feedback…", FEEDBACK),
];

/// Where feedback and bug reports go.
pub const FEEDBACK: &str = "https://github.com/teosperini/fotofficina/issues/new";

/// The URL behind a link command id.
pub fn url_of(cmd: &str) -> Option<&'static str> {
    LINKS.iter().find(|(id, _, _)| *id == cmd).map(|(_, _, u)| *u)
}

/// Open `url` in the user's browser (through the host's `open_url` service).
pub fn open(app: &mut crate::LightcraftApp, url: &str) -> Result<serde_json::Value, String> {
    let open = app.services.open_url.as_mut().ok_or("can't open links here")?;
    open(url)?;
    Ok(serde_json::json!({ "url": url }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_https_and_name_this_app() {
        for (id, label, url) in LINKS {
            assert!(url.starts_with("https://"), "{id}");
            assert!(!label.is_empty());
            assert_eq!(url_of(id), Some(*url));
        }
        assert!(APP_PAGE.ends_with(&format!("/{APP}")));
        assert!(GITHUB.ends_with(&format!("/teosperini/{APP}")));
    }
}
