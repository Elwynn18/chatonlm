use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Empreinte attendue du pack CUDA (`VERSION` :
/// `<version de llama.cpp>+chatonlm.cuda.<empreinte>`), même formule que
/// scripts/build-llama-cuda.sh : SHA-256 de llama.env, cuda.env, des
/// correctifs `packaging/llama/*.patch` (ordre des noms) et du script mis
/// bout à bout, retours chariot retirés, 12 premiers caractères. Un pack
/// installé d'une autre empreinte est périmé (à retélécharger).
fn cuda_pack_fingerprint(manifest_dir: &Path) -> String {
    let root = manifest_dir.join("..");
    let patches_dir = root.join("packaging/llama");
    let mut patches: Vec<PathBuf> = std::fs::read_dir(&patches_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "patch"))
        .collect();
    patches.sort();
    let mut files = vec![
        root.join("packaging/llama.env"),
        root.join("packaging/cuda.env"),
    ];
    files.extend(patches);
    files.push(root.join("scripts/build-llama-cuda.sh"));
    println!("cargo:rerun-if-changed={}", patches_dir.display());
    let mut hasher = Sha256::new();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let bytes =
            std::fs::read(file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        let bytes: Vec<u8> = bytes.into_iter().filter(|byte| *byte != b'\r').collect();
        hasher.update(&bytes);
    }
    hasher.finalize()[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn main() {
    let target = |name: &str| std::env::var(name).unwrap_or_default();
    let fingerprint = cuda_pack_fingerprint(Path::new(&target("CARGO_MANIFEST_DIR")));
    println!("cargo:rustc-env=CHATONLM_CUDA_PACK_FINGERPRINT={fingerprint}");
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
