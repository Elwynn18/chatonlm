//! Menu contextuel de WebView2, filtré par inclusion (voir le module
//! parent), et touches de navigateur coupées.
//!
//! Le branchement COM n'est essayé que sur une vraie machine ; la logique du
//! filtrage est celle du module parent, essayée partout.

use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR,
    ICoreWebView2_11, ICoreWebView2ContextMenuRequestedEventArgs, ICoreWebView2Settings3,
};
use webview2_com::{ContextMenuRequestedEventHandler, take_pwstr};
use windows::core::{Interface, PWSTR};

use super::retained_names;

/// « Inspect » : seulement dans un build de débogage, le seul où Tauri active
/// les outils de développement.
const INSPECTOR: bool = cfg!(debug_assertions);

/// Branche le filtrage sur la webview de la fenêtre et coupe ses touches de
/// navigateur (F5 et Ctrl+R : recharger, Ctrl+P : imprimer, Ctrl+F et F3 :
/// chercher, F7, F12...). Les touches d'édition (Ctrl+C, Ctrl+V, Ctrl+A,
/// Ctrl+Z...) restent, et la page reçoit toujours toutes les touches (ses
/// propres raccourcis ne changent pas).
pub fn install(window: &WebviewWindow) {
    let result = window.with_webview(|webview| {
        // SAFETY : appels COM sur le fil de la webview (fil principal), avec
        // des interfaces obtenues de son contrôleur vivant.
        let installed = unsafe { install_on(&webview) };
        if let Err(error) = installed {
            eprintln!("chatonlm: cannot filter the context menu: {error}");
        }
    });
    if let Err(error) = result {
        eprintln!("chatonlm: cannot filter the context menu: {error}");
    }
}

unsafe fn install_on(webview: &tauri::webview::PlatformWebview) -> windows::core::Result<()> {
    let core = unsafe { webview.controller().CoreWebView2() }?;
    match unsafe { core.Settings() }.and_then(|settings| settings.cast::<ICoreWebView2Settings3>())
    {
        Ok(settings) => unsafe { settings.SetAreBrowserAcceleratorKeysEnabled(false) }?,
        Err(error) => eprintln!("chatonlm: browser keys left enabled: {error}"),
    }
    let handler = ContextMenuRequestedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            // SAFETY : arguments de l'évènement en cours, sur le fil de la
            // webview.
            unsafe { filter(&args) }?;
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        core.cast::<ICoreWebView2_11>()?
            .add_ContextMenuRequested(&handler, &mut token)
    }
}

/// Retire du menu les entrées qui ne sont pas gardées ; s'il n'en reste
/// aucune, aucun menu n'est montré.
unsafe fn filter(args: &ICoreWebView2ContextMenuRequestedEventArgs) -> windows::core::Result<()> {
    let items = unsafe { args.MenuItems() }?;
    let mut count = 0u32;
    unsafe { items.Count(&mut count) }?;
    // Nom de chaque entrée (`None` : séparateur).
    let mut names: Vec<Option<String>> = Vec::with_capacity(count as usize);
    for index in 0..count {
        let item = unsafe { items.GetValueAtIndex(index) }?;
        let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
        unsafe { item.Kind(&mut kind) }?;
        if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR {
            names.push(None);
        } else {
            let mut name = PWSTR::null();
            unsafe { item.Name(&mut name) }?;
            names.push(Some(take_pwstr(name)));
        }
    }
    #[cfg(debug_assertions)]
    eprintln!("chatonlm: context menu entries: {names:?}");
    let entries: Vec<Option<&str>> = names.iter().map(Option::as_deref).collect();
    let kept = retained_names(&entries, INSPECTOR);
    // De la fin vers le début : les rangs des entrées restantes ne bougent pas.
    for index in (0..count).rev() {
        if !kept[index as usize] {
            unsafe { items.RemoveValueAtIndex(index) }?;
        }
    }
    if !kept.contains(&true) {
        unsafe { args.SetHandled(true) }?;
    }
    Ok(())
}
