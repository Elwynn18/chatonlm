// Windows : app graphique, sans fenêtre de console (build de production
// seulement : les builds de débogage gardent leur sortie dans le terminal).
// L'enfant d'extraction des PDF garde ses tubes, remis par l'app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    // Processus enfant d'extraction d'un PDF : traité avant toute
    // initialisation de Tauri ou de GTK.
    if let Some(code) = chatonlm_lib::child_process() {
        std::process::exit(code);
    }
    chatonlm_lib::run();
}
