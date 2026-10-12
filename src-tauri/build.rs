use std::path::PathBuf;

fn main() {
    let target = |name: &str| std::env::var(name).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") != "windows" || target("CARGO_CFG_TARGET_ENV") != "msvc" {
        // Linux, et contrôle croisé de Windows (gnullvm, où rien n'est lié).
        tauri_build::build();
        return;
    }

    // Windows (MSVC) : Tauri n'ajoute son manifeste d'application (contrôles
    // communs v6, exigés par les boîtes de dialogue) qu'aux ressources du
    // binaire de l'app. Les exécutables d'essai, qui lient aussi Tauri, ne
    // démarreraient pas sans lui (STATUS_ENTRYPOINT_NOT_FOUND). Le même
    // manifeste est donc donné à l'éditeur de liens, pour tous les exécutables
    // du paquet, et retiré des ressources (deux manifestes : échec du lien).
    // C'est ce que fait le crate tauri pour ses propres essais (son build.rs,
    // `embed_manifest_for_tests`).
    let manifest = PathBuf::from(target("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run the Tauri build script");
}
