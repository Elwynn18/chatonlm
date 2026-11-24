//! Notification simple sous Linux : notify-rust (session D-Bus).

use std::path::Path;

use notify_rust::{Hint, Notification};

/// Notification au nom de l'app : `body` échappé (les serveurs lisent un
/// sous-ensemble de HTML), icône écrite par l'app, sinon celle du thème
/// (nom de l'identifiant).
pub(super) fn notification(
    identifier: &str,
    icon: Option<&Path>,
    title: &str,
    body: &str,
) -> Notification {
    let icon = icon.map_or_else(
        || identifier.to_owned(),
        |path| path.to_string_lossy().into_owned(),
    );
    let mut notification = Notification::new();
    notification
        .appname("ChatonLM")
        .summary(title)
        .body(&crate::dictation::notify::escape(body))
        .icon(&icon)
        .hint(Hint::DesktopEntry(identifier.to_owned()));
    notification
}

pub(super) async fn show(
    identifier: &str,
    icon: Option<&Path>,
    title: &str,
    body: &str,
) -> Result<(), String> {
    notification(identifier, icon, title, body)
        .show_async()
        .await
        .map(drop)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notification_is_in_the_name_of_the_app() {
        let shown = notification(
            "com.chatonlm.chatonlm",
            None,
            "Project finished",
            "Guide <potager> & semis",
        );
        assert_eq!(shown.appname, "ChatonLM");
        assert_eq!(shown.summary, "Project finished");
        assert_eq!(shown.body, "Guide &lt;potager&gt; &amp; semis");
        assert_eq!(shown.icon, "com.chatonlm.chatonlm");
        assert!(
            shown
                .hints
                .contains(&Hint::DesktopEntry("com.chatonlm.chatonlm".into()))
        );
        let with_icon = notification(
            "com.chatonlm.chatonlm",
            Some(Path::new("/data/icons/chatonlm.png")),
            "t",
            "b",
        );
        assert_eq!(with_icon.icon, "/data/icons/chatonlm.png");
    }
}
