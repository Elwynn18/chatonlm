//! Fenêtre principale.
//!
//! Elle est déclarée dans `tauri.conf.json` avec `"create": false` et construite
//! ici, ce qui permet de régler ce que la configuration ne sait pas exprimer :
//! navigation autorisée, nouvelles fenêtres refusées, taille mémorisée, fond du
//! thème. Elle naît masquée : le frontend l'affiche quand le thème est
//! appliqué, pour éviter un flash (sauf au lancement par `--dictate` ou
//! `--proofread`, où elle reste masquée : voir `desktop`).

use std::thread;
use std::time::Duration;

use tauri::webview::NewWindowResponse;
use tauri::window::Color;
use tauri::{App, LogicalSize, Manager, Runtime, Url, WebviewWindowBuilder, Window};

use crate::context_menu;
use crate::desktop;
use crate::store::Store;
use crate::store::settings::{self, Theme};

const MAIN_WINDOW: &str = "main";

/// Délai au-delà duquel la fenêtre est affichée même si le frontend ne l'a pas
/// fait (erreur JavaScript, permission manquante...).
const SHOW_FALLBACK: Duration = Duration::from_millis(1500);

/// Fond du thème sombre (neutral-900 de Tailwind, comme l'app officielle).
const DARK_BACKGROUND: Color = Color(0x17, 0x17, 0x17, 0xff);

/// `store` vaut `None` si la base n'a pas pu s'ouvrir : fenêtre par défaut,
/// le frontend explique le problème.
pub fn create_main_window(app: &App, store: Option<&Store>) -> tauri::Result<()> {
    // Essais e2e : setup ralenti avant que la base soit gérée, pour qu'une
    // relance (`--dictate`) arrive à coup sûr pendant le setup.
    #[cfg(feature = "e2e")]
    if let Some(ms) = std::env::var("CHATONLM_E2E_SETUP_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        thread::sleep(Duration::from_millis(ms));
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .cloned()
        .expect("main window missing from tauri.conf.json");
    let dev_origin = dev_origin(tauri::is_dev(), app.config().build.dev_url.clone());
    // Réglages illisibles : fenêtre par défaut, l'erreur réapparaîtra ailleurs.
    let saved = store.and_then(|store| store.blocking(|c| settings::get(c, None)).ok());

    let mut builder = WebviewWindowBuilder::from_config(app.handle(), &config)?
        .on_navigation(move |url| is_app_url(url, dev_origin.as_ref(), cfg!(windows)))
        .on_new_window(|url, _features| refuse_new_window(&url));
    if let Some(saved) = &saved {
        if let Some((width, height)) = restored_size(saved, config.min_width, config.min_height) {
            builder = builder.inner_size(width, height);
        }
        if saved.theme == Theme::Dark {
            builder = builder.background_color(DARK_BACKGROUND);
        }
    }
    // WebView2 : ses données sous le dossier de données de l'app (il y ajoute
    // « EBWebView »), pas de saisie automatique dans les champs, barre de
    // titre du thème choisi (sous Linux, le thème de Tauri ne sert jamais :
    // voir `apply_theme`).
    #[cfg(windows)]
    {
        builder = builder.general_autofill_enabled(false);
        match crate::paths::data_dir(app.handle()) {
            Ok(dir) => builder = builder.data_directory(dir),
            Err(error) => eprintln!("chatonlm: no data folder for the web view: {error}"),
        }
        if let Some(saved) = &saved {
            builder = builder.theme(title_bar_theme(saved.theme));
        }
    }
    // Lancée par `chatonlm --dictate` ou `--proofread` : la fenêtre reste
    // masquée (la page ne peut pas l'afficher), une relance ou la zone de
    // notification la montre.
    let start_hidden = desktop::started_hidden();
    if start_hidden {
        desktop::keep_main_window_hidden(app.handle())?;
    }
    let window = builder.build()?;
    // Menu contextuel natif sans entrées de navigateur (voir context_menu.rs).
    context_menu::install(&window);

    if start_hidden {
        return Ok(());
    }
    thread::spawn(move || {
        thread::sleep(SHOW_FALLBACK);
        if !window.is_visible().unwrap_or(true) {
            // Signalé : les essais en session virtuelle échouent sur ce message,
            // car c'est au frontend d'afficher la fenêtre.
            eprintln!("chatonlm: window shown by the startup fallback");
            let _ = window.show();
        }
    });
    Ok(())
}

/// Taille mémorisée, jamais sous le minimum de la fenêtre.
fn restored_size(
    saved: &settings::Settings,
    min_width: Option<f64>,
    min_height: Option<f64>,
) -> Option<(f64, f64)> {
    let width = f64::from(saved.window_width?);
    let height = f64::from(saved.window_height?);
    Some((
        width.max(min_width.unwrap_or(0.0)),
        height.max(min_height.unwrap_or(0.0)),
    ))
}

/// Mémorise la taille de la fenêtre à sa fermeture. Une fenêtre agrandie ou
/// en plein écran garde la taille précédente, comme on l'attend au relancement.
pub fn remember_size<R: Runtime>(window: &Window<R>) {
    if window.label() != MAIN_WINDOW
        || window.is_maximized().unwrap_or(true)
        || window.is_fullscreen().unwrap_or(true)
    {
        return;
    }
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size: LogicalSize<f64> = size.to_logical(scale);
    // Pas de base (ouverture échouée) : rien à mémoriser.
    let Some(store) = window.try_state::<Store>() else {
        return;
    };
    let result = store.blocking(|c| {
        settings::save_window_size(c, size.width.round() as u32, size.height.round() as u32)
    });
    if let Err(error) = result {
        eprintln!("chatonlm: cannot save the window size: {error}");
    }
}

/// Thème de la barre de titre de Windows pour le réglage `theme` (`None` :
/// celui du système).
#[cfg_attr(not(windows), allow(dead_code))]
fn title_bar_theme(theme: Theme) -> Option<tauri::Theme> {
    match theme {
        Theme::System => None,
        Theme::Light => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
    }
}

/// Réglage `theme` changé. Sous Windows, la barre de titre (et le
/// `prefers-color-scheme` de WebView2) suit le réglage. Sous Linux, rien :
/// le `set_theme` de Tauri ne doit jamais y être appelé (tao : `None` force
/// le clair, et toute valeur s'applique à toute l'app) ; le thème n'y est que
/// la classe `dark` de la page.
pub fn apply_theme<R: Runtime>(app: &tauri::AppHandle<R>, theme: Theme) {
    #[cfg(windows)]
    if let Some(window) = app.get_webview_window(MAIN_WINDOW)
        && let Err(error) = window.set_theme(title_bar_theme(theme))
    {
        eprintln!("chatonlm: cannot set the title bar theme: {error}");
    }
    #[cfg(not(windows))]
    let _ = (app, theme);
}

/// L'URL du serveur Vite reste dans la configuration embarquée des builds de
/// production : elle n'est autorisée qu'en mode dev.
fn dev_origin(is_dev: bool, dev_url: Option<Url>) -> Option<Url> {
    dev_url.filter(|_| is_dev)
}

/// La webview ne quitte jamais l'application : toute autre navigation est
/// refusée. Les liens des réponses et des messages d'erreur s'ouvrent dans le
/// navigateur par la commande `open_external`, appelée par le frontend
/// (`ExternalLink`) ; un lien qui y échapperait, ou un lien glissé sur la
/// fenêtre, s'arrête ici. `windows` : sous Windows, WebView2 sert la page de
/// l'app sous `http://tauri.localhost` (pas de protocole `tauri://`).
fn is_app_url(url: &Url, dev_origin: Option<&Url>, windows: bool) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" if windows && url.host_str() == Some("tauri.localhost") && url.port().is_none() => {
            true
        }
        "http" | "https" => dev_origin.is_some_and(|dev| dev.origin() == url.origin()),
        _ => false,
    }
}

/// `window.open`, `target="_blank"`, clic milieu sur un tel lien : jamais de
/// nouvelle fenêtre (sans gestionnaire, WebKit ne l'ouvrait pas non plus, mais
/// sans le dire). Signalé pour repérer un lien que le frontend n'intercepte pas.
fn refuse_new_window<R: Runtime>(url: &Url) -> NewWindowResponse<R> {
    eprintln!("chatonlm: new window refused: {url}");
    NewWindowResponse::Deny
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn app_protocol_is_allowed() {
        assert!(is_app_url(&url("tauri://localhost/c/new"), None, false));
        assert!(!is_app_url(&url("tauri://example.com/"), None, false));
        // L'origine de WebView2 n'est pas celle de l'app sous Linux.
        assert!(!is_app_url(
            &url("http://tauri.localhost/c/new"),
            None,
            false
        ));
    }

    #[test]
    fn the_webview2_origin_is_allowed_on_windows_only() {
        assert!(is_app_url(&url("http://tauri.localhost/"), None, true));
        assert!(is_app_url(
            &url("http://tauri.localhost/settings?section=dictation"),
            None,
            true
        ));
        assert!(!is_app_url(&url("https://tauri.localhost/"), None, true));
        assert!(!is_app_url(
            &url("http://tauri.localhost:8080/"),
            None,
            true
        ));
        assert!(!is_app_url(&url("http://localhost/"), None, true));
        assert!(!is_app_url(
            &url("http://evil.tauri.localhost/"),
            None,
            true
        ));
        assert!(!is_app_url(&url("http://chatonlm.localhost/x"), None, true));
        assert!(!is_app_url(&url("https://ollama.com/"), None, true));
    }

    #[test]
    fn the_title_bar_follows_the_theme_setting() {
        assert_eq!(title_bar_theme(Theme::System), None);
        assert_eq!(title_bar_theme(Theme::Dark), Some(tauri::Theme::Dark));
        assert_eq!(title_bar_theme(Theme::Light), Some(tauri::Theme::Light));
    }

    #[test]
    fn dev_server_is_allowed_only_in_dev_mode() {
        let dev_url = Some(url("http://localhost:1420"));
        let in_dev = dev_origin(true, dev_url.clone());
        let in_production = dev_origin(false, dev_url);
        let page = url("http://localhost:1420/settings");
        assert!(is_app_url(&page, in_dev.as_ref(), false));
        assert!(!is_app_url(&page, in_production.as_ref(), false));
        assert!(!is_app_url(
            &url("http://localhost:9999/"),
            in_dev.as_ref(),
            false
        ));
    }

    #[test]
    fn external_sites_are_refused() {
        let dev = dev_origin(true, Some(url("http://localhost:1420")));
        assert!(!is_app_url(
            &url("https://ollama.com/download"),
            dev.as_ref(),
            false
        ));
        assert!(!is_app_url(&url("file:///etc/passwd"), dev.as_ref(), false));
    }

    #[test]
    fn new_windows_are_refused() {
        let response = refuse_new_window::<tauri::Wry>(&url("https://ollama.com/"));
        assert!(matches!(response, NewWindowResponse::Deny));
    }

    fn saved(width: Option<u32>, height: Option<u32>) -> settings::Settings {
        settings::Settings {
            server_url: String::new(),
            effective_server_url: String::new(),
            context_length: 0,
            theme: Theme::System,
            selected_model: String::new(),
            window_width: width,
            window_height: height,
            web_search_enabled: false,
            keep_alive: 0,
            preload_model: false,
            language: crate::i18n::LanguageChoice::System,
            effective_language: crate::i18n::Lang::En,
            proofread_model: String::new(),
            proofread_shortcut: false,
            proofread_paste: true,
            study_model: String::new(),
            auto_read: false,
        }
    }

    #[test]
    fn saved_size_is_restored_but_never_below_the_minimum() {
        let (min_w, min_h) = (Some(900.0), Some(660.0));
        assert_eq!(restored_size(&saved(None, None), min_w, min_h), None);
        assert_eq!(
            restored_size(&saved(Some(1200), Some(800)), min_w, min_h),
            Some((1200.0, 800.0))
        );
        assert_eq!(
            restored_size(&saved(Some(400), Some(300)), min_w, min_h),
            Some((900.0, 660.0))
        );
    }
}
