// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: Ollama
// SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
//
// pick_files translated to Rust from Ollama (https://github.com/ollama/ollama),
// file app/cmd/app/webview.go (selectFiles), commit 42e911bc3d05
// (dialog title, "Supported Files" filter, multiple selection).
// Modified for ChatonLM: the dialog is opened by the backend with the
// extensions of the attachment validation, matched whatever their case; the
// chosen files are not read here but returned as paths, refused files being
// reported by the staging instead of silently skipped; only the paths handed
// over by the dialog (or a native drop) can then be staged.

//! Pièces jointes : sélecteur natif et zone de préparation (voir
//! `attachments`) ; enregistrement d'une image de la conversation (propre à
//! ChatonLM).

use percent_encoding::percent_decode_str;
use tauri::http::HeaderMap;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::attachments::{self, StageResult, Staging};
use crate::error::{AppError, AppResult};
use crate::i18n::t;
use crate::store::{Store, chats};

/// En-tête qui porte le nom d'un fichier envoyé en octets bruts, encodé par
/// `encodeURIComponent`.
const FILENAME_HEADER: &str = "x-filename";

/// En-tête facultatif : taille du fichier collé annoncée par la page (entier
/// décimal). Au-delà de 10 Mio, la page n'en lit pas le contenu.
const SIZE_HEADER: &str = "x-size";

/// Ouvre le sélecteur de fichiers natif (plusieurs fichiers, filtre sur les
/// extensions acceptées) au-dessus de la fenêtre appelante, comme
/// `selectFiles` de l'officiel ; chemins choisis, aucun si l'utilisateur
/// annule. Les chemins choisis sont remis à la zone de préparation : seuls
/// ceux-là (et ceux d'un dépôt natif) peuvent ensuite être préparés.
#[tauri::command]
pub async fn pick_files(
    window: WebviewWindow,
    staging: State<'_, Staging>,
) -> AppResult<Vec<String>> {
    let generation = staging.generation();
    // Essais de l'app réelle : pas de dialogue à piloter, les chemins
    // « choisis » sont lus dans le fichier désigné, relu à chaque appel.
    #[cfg(feature = "e2e")]
    if let Some(list) = std::env::var_os("CHATONLM_E2E_PICK_FILE") {
        let list = std::path::PathBuf::from(list);
        let picked = tauri::async_runtime::spawn_blocking(move || e2e_picks(&list)).await;
        return Ok(hand_over(&staging, generation, picked.unwrap_or_default()));
    }
    let patterns: Vec<String> = attachments::accepted_extensions()
        .map(|extension| picker_pattern(extension, cfg!(windows)))
        .collect();
    let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
    // Le greffon ouvre le dialogue sur le fil principal et rappelle depuis un
    // autre fil : rien n'est bloqué pendant que l'utilisateur choisit.
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(t!("dialogs.selectFiles"))
        .add_filter(t!("dialogs.supportedFiles"), &patterns)
        .pick_files(move |paths| {
            let _ = sender.send(paths);
        });
    // Rappel jamais fait (boucle d'événements arrêtée) : comme une annulation.
    let paths = receiver.await.ok().flatten().unwrap_or_default();
    let paths = paths
        .into_iter()
        .filter_map(|path| path.into_path().ok())
        // Chemin non UTF-8 : la préparation répondra « Error reading file ».
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    Ok(hand_over(&staging, generation, paths))
}

/// Enregistre une image de la conversation (image produite, propre à
/// ChatonLM) dans le fichier choisi par la boîte « Enregistrer » native, au
/// nom de la pièce jointe ; vrai si le fichier est écrit, faux si
/// l'utilisateur annule.
#[tauri::command]
pub async fn save_image(
    window: WebviewWindow,
    store: State<'_, Store>,
    attachment_id: i64,
) -> AppResult<bool> {
    let found = store
        .call(move |connection| chats::image_file(connection, attachment_id))
        .await?;
    let Some((filename, data)) = found else {
        return Err(AppError::NotFound(t!("chat.imageNotFound")));
    };
    let Some(path) = pick_image_path(&window, suggested_name(&filename)).await else {
        return Ok(false);
    };
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, data))
        .await
        .map_err(|error| AppError::Internal(error.to_string()))??;
    Ok(true)
}

/// Nom proposé pour enregistrer une image : celui de la pièce jointe, en
/// `.png` s'il n'a pas d'extension d'image.
fn suggested_name(filename: &str) -> String {
    let lower = filename.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
    if matches!(
        extension,
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
    ) {
        filename.to_owned()
    } else {
        format!("{filename}.png")
    }
}

/// Chemin choisi dans la boîte « Enregistrer » native, au-dessus de la
/// fenêtre ; `None` si l'utilisateur annule.
async fn pick_image_path(window: &WebviewWindow, name: String) -> Option<std::path::PathBuf> {
    // Essais de l'app réelle : pas de boîte à piloter, le chemin « choisi »
    // est la valeur de la variable (vide : annulé), comme pour les notes d'un
    // cours.
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os("CHATONLM_E2E_SAVE_FILE") {
        return (!path.is_empty()).then(|| std::path::PathBuf::from(path));
    }
    let png = name.to_ascii_lowercase().ends_with(".png");
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let dialog = window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(t!("dialogs.saveImage"))
        .set_file_name(name);
    let dialog = if png {
        dialog.add_filter(t!("dialogs.pngImages"), &["png"])
    } else {
        dialog
    };
    dialog.save_file(move |path| {
        let _ = sender.send(path);
    });
    // Rappel jamais fait (boucle d'événements arrêtée) : comme une annulation.
    receiver
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}

/// Remet les chemins choisis à la zone de préparation, sauf si la page a été
/// rechargée pendant le choix (`generation` : avant l'ouverture), et les
/// renvoie.
fn hand_over(staging: &Staging, generation: u64, paths: Vec<String>) -> Vec<String> {
    staging.grant_if_current(generation, &paths);
    paths
}

/// Nom d'une image collée depuis le presse-papiers (comme un fichier collé
/// sans nom, voir src/lib/pastedFiles.ts).
const PASTED_IMAGE: &str = "image.png";

/// Prépare l'image du presse-papiers (capture d'écran collée), que WebKitGTK
/// ne donne pas à la page ; `None` s'il n'y en a pas.
#[tauri::command]
pub async fn stage_clipboard_image(
    app: AppHandle,
    staging: State<'_, Staging>,
) -> AppResult<Option<StageResult>> {
    let Some(png) = attachments::clipboard::read_png(&app).await? else {
        return Ok(None);
    };
    Ok(Some(staging.stage_bytes(PASTED_IMAGE.into(), png).await))
}

/// Prépare des fichiers choisis par leur chemin (sélecteur natif, dépôt),
/// dans l'ordre ; un dossier donne ses fichiers de premier niveau.
#[tauri::command]
pub async fn stage_paths(
    staging: State<'_, Staging>,
    paths: Vec<String>,
) -> AppResult<Vec<StageResult>> {
    Ok(staging.stage_paths(paths).await)
}

/// Prépare un fichier collé : son contenu en corps brut, son nom dans
/// l'en-tête `x-filename`, sa taille annoncée (facultative) dans `x-size`.
#[tauri::command]
pub async fn stage_bytes(
    staging: State<'_, Staging>,
    request: Request<'_>,
) -> AppResult<StageResult> {
    stage_request(staging.inner(), request.headers(), request.body()).await
}

/// Retire des fichiers préparés ; sans effet pour un identifiant inconnu.
#[tauri::command]
pub async fn unstage(staging: State<'_, Staging>, ids: Vec<String>) -> AppResult<()> {
    staging.unstage(&ids);
    Ok(())
}

/// Essais de l'app réelle seulement : remet des chemins comme un dépôt natif
/// (e2e/drop-paste.test.mjs simule les dépôts par des évènements
/// `tauri://drag-drop` émis par la page, qui n'autorisent rien).
#[cfg(feature = "e2e")]
#[tauri::command]
pub async fn e2e_allow_paths(staging: State<'_, Staging>, paths: Vec<String>) -> AppResult<()> {
    staging.grant(paths);
    Ok(())
}

/// Travail de `stage_bytes`. En-têtes illisibles : requête refusée. Puis
/// l'ordre de l'officiel : type, taille annoncée (refusée sans regarder le
/// corps, que la page n'a pas lu : il peut être vide), enfin les contrôles du
/// contenu reçu.
async fn stage_request(
    staging: &Staging,
    headers: &HeaderMap,
    body: &InvokeBody,
) -> AppResult<StageResult> {
    let filename = filename(headers.get(FILENAME_HEADER).map(|v| v.as_bytes()))?;
    let declared = declared_size(headers.get(SIZE_HEADER).map(|v| v.as_bytes()))?;
    if let Some(error) = attachments::refusal(&filename, declared) {
        return Ok(StageResult::Rejected {
            filename,
            error: error.text(),
        });
    }
    let InvokeBody::Raw(data) = body else {
        // i18n-ignore : la page envoie toujours des octets bruts.
        return Err(AppError::Invalid(
            "the file must be sent as raw bytes".into(),
        ));
    };
    Ok(staging.stage_bytes(filename, data.clone()).await)
}

/// Nom du fichier, décodé comme `decodeURIComponent`. Erreurs en anglais :
/// la page envoie toujours un en-tête valide.
fn filename(header: Option<&[u8]>) -> AppResult<String> {
    // i18n-ignore
    let header = header.ok_or_else(|| AppError::Invalid("missing x-filename header".into()))?;
    let encoded = std::str::from_utf8(header)
        // i18n-ignore
        .map_err(|_| AppError::Invalid("invalid x-filename header".into()))?;
    percent_decode_str(encoded)
        .decode_utf8()
        .map(|name| name.into_owned())
        // i18n-ignore
        .map_err(|_| AppError::Invalid("invalid x-filename header".into()))
}

/// Taille annoncée par la page (en-tête facultatif) : chiffres décimaux
/// seulement.
fn declared_size(header: Option<&[u8]>) -> AppResult<Option<u64>> {
    let Some(header) = header else {
        return Ok(None);
    };
    // i18n-ignore : la page envoie toujours une taille valide.
    let invalid = || AppError::Invalid("invalid x-size header".into());
    if header.is_empty() || !header.iter().all(u8::is_ascii_digit) {
        return Err(invalid());
    }
    std::str::from_utf8(header)
        .ok()
        .and_then(|digits| digits.parse().ok())
        .map(Some)
        .ok_or_else(invalid)
}

/// Motif d'une extension pour le filtre du sélecteur. Sous Windows,
/// l'extension nue : son filtre ignore déjà la casse et ne connaît que `*`
/// et `?` (un motif à crochets ne montrerait aucun fichier). Ailleurs, le
/// motif insensible à la casse de [`case_insensitive_glob`].
fn picker_pattern(extension: &str, windows: bool) -> String {
    if windows {
        extension.to_owned()
    } else {
        case_insensitive_glob(extension)
    }
}

/// Motif d'une extension, insensible à la casse : `png` → `[pP][nN][gG]`.
/// rfd en fait `*.[pP][nN][gG]` ; un motif GTK 3 nu (`*.png`) est sensible à
/// la casse et masquerait « PHOTO.PNG », que la préparation accepte.
fn case_insensitive_glob(extension: &str) -> String {
    extension
        .chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Crochet des essais e2e : chemins listés dans ce fichier, un par ligne
/// (lignes vides ignorées) ; fichier absent ou illisible : annulation.
#[cfg(any(test, feature = "e2e"))]
fn e2e_picks(list: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(list)
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use tauri::http::HeaderValue;

    use super::*;
    use crate::attachments::MAX_FILE_SIZE;
    use crate::attachments::tests::{
        ERROR_READING_FILE, FILE_TOO_LARGE, FILE_TYPE_NOT_SUPPORTED, rejection, staged, staging,
        temp_dir,
    };

    #[test]
    fn a_saved_image_keeps_its_name_as_an_image_file() {
        assert_eq!(suggested_name("image-7.png"), "image-7.png");
        assert_eq!(suggested_name("Photo.JPG"), "Photo.JPG");
        assert_eq!(suggested_name("image"), "image.png");
        assert_eq!(suggested_name("notes.txt"), "notes.txt.png");
    }

    #[test]
    fn the_filename_is_decoded_like_decode_uri_component() {
        // encodeURIComponent("résumé 2026 (v2)#1.txt")
        let encoded = "r%C3%A9sum%C3%A9%202026%20(v2)%231.txt";
        assert_eq!(
            filename(Some(encoded.as_bytes())).unwrap(),
            "résumé 2026 (v2)#1.txt"
        );
        assert_eq!(filename(Some(b"notes.txt")).unwrap(), "notes.txt");
        for bad in [None, Some(&b"%FF.txt"[..]), Some(&b"\xff.txt"[..])] {
            assert!(
                matches!(filename(bad), Err(AppError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_declared_size_is_a_decimal_integer() {
        assert_eq!(declared_size(None).unwrap(), None);
        assert_eq!(declared_size(Some(b"0")).unwrap(), Some(0));
        assert_eq!(declared_size(Some(b"10485761")).unwrap(), Some(10_485_761));
        // String(size) d'un File ne donne jamais autre chose.
        for bad in [
            "",
            "-1",
            "+5",
            " 5",
            "5 ",
            "1e3",
            "0x10",
            "5.0",
            "abc",
            "99999999999999999999999",
        ] {
            assert!(
                matches!(
                    declared_size(Some(bad.as_bytes())),
                    Err(AppError::Invalid(_))
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            declared_size(Some(b"\xff")),
            Err(AppError::Invalid(_))
        ));
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn raw(data: &[u8]) -> InvokeBody {
        InvokeBody::Raw(data.to_vec())
    }

    #[tokio::test]
    async fn a_declared_size_is_refused_after_the_type_without_reading_the_body() {
        let staging = staging();
        let too_large = (MAX_FILE_SIZE + 1).to_string();
        // Le type d'abord, comme l'officiel.
        let result = stage_request(
            &staging,
            &headers(&[("x-filename", "big.exe"), ("x-size", &too_large)]),
            &raw(b""),
        )
        .await
        .unwrap();
        assert_eq!(rejection(&result), ("big.exe", FILE_TYPE_NOT_SUPPORTED));
        // Puis la taille annoncée, quel que soit le corps : la page ne lit
        // pas le fichier (corps vide, ou JSON si l'IPC s'est replié).
        for body in [raw(b""), InvokeBody::Json(serde_json::json!({})), raw(b"x")] {
            let result = stage_request(
                &staging,
                &headers(&[("x-filename", "photo.png"), ("x-size", &too_large)]),
                &body,
            )
            .await
            .unwrap();
            assert_eq!(rejection(&result), ("photo.png", FILE_TOO_LARGE));
        }
        // Taille permise : le contenu reçu fait foi.
        let limit = MAX_FILE_SIZE.to_string();
        let result = stage_request(
            &staging,
            &headers(&[("x-filename", "notes.txt"), ("x-size", &limit)]),
            &raw(b"hello"),
        )
        .await
        .unwrap();
        assert_eq!(staged(&result).size, 5);
        let oversized = vec![b'a'; usize::try_from(MAX_FILE_SIZE).unwrap() + 1];
        let result = stage_request(
            &staging,
            &headers(&[("x-filename", "big.txt"), ("x-size", "1")]),
            &InvokeBody::Raw(oversized),
        )
        .await
        .unwrap();
        assert_eq!(rejection(&result), ("big.txt", FILE_TOO_LARGE));
        // Sans taille annoncée, comme avant.
        let result = stage_request(
            &staging,
            &headers(&[("x-filename", "notes.txt")]),
            &raw(b"hi"),
        )
        .await
        .unwrap();
        assert_eq!(staged(&result).filename, "notes.txt");
    }

    #[tokio::test]
    async fn a_malformed_request_is_refused() {
        let staging = staging();
        for (pairs, body) in [
            // En-têtes illisibles, avant tout contrôle du fichier.
            (
                vec![("x-filename", "notes.txt"), ("x-size", "abc")],
                raw(b"hi"),
            ),
            (
                vec![("x-filename", "big.exe"), ("x-size", "-1")],
                raw(b"hi"),
            ),
            (vec![("x-size", "2")], raw(b"hi")),
            // Contenu à lire mais pas en octets bruts.
            (
                vec![("x-filename", "notes.txt"), ("x-size", "2")],
                InvokeBody::Json(serde_json::json!([104, 105])),
            ),
        ] {
            let result = stage_request(&staging, &headers(&pairs), &body).await;
            assert!(matches!(result, Err(AppError::Invalid(_))), "{pairs:?}");
        }
    }

    #[test]
    fn the_picker_filter_ignores_case() {
        assert_eq!(case_insensitive_glob("png"), "[pP][nN][gG]");
        assert_eq!(case_insensitive_glob("c"), "[cC]");
        // Les chiffres restent tels quels (aucune extension acceptée n'en a).
        assert_eq!(case_insensitive_glob("mp4"), "[mM][pP]4");
        assert_eq!(picker_pattern("png", false), "[pP][nN][gG]");
        // Windows : extension nue (filtre déjà insensible à la casse).
        assert_eq!(picker_pattern("png", true), "png");
    }

    #[tokio::test]
    async fn picked_paths_are_handed_over_once() {
        let dir = temp_dir();
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();
        let notes = notes.to_str().unwrap().to_owned();
        let staging = staging();
        // Choisi : renvoyé à la page et préparable, une fois.
        let picked = hand_over(&staging, staging.generation(), vec![notes.clone()]);
        assert_eq!(picked, std::slice::from_ref(&notes));
        let results = staging.stage_paths(picked.clone()).await;
        assert_eq!(staged(&results[0]).filename, "notes.txt");
        let results = staging.stage_paths(picked).await;
        assert_eq!(rejection(&results[0]), ("notes.txt", ERROR_READING_FILE));
        // Page rechargée pendant le choix : renvoyé, mais pas remis.
        let before = staging.generation();
        staging.clear();
        let picked = hand_over(&staging, before, vec![notes.clone()]);
        assert_eq!(picked, [notes]);
        let results = staging.stage_paths(picked).await;
        assert_eq!(rejection(&results[0]), ("notes.txt", ERROR_READING_FILE));
    }

    #[test]
    fn the_e2e_hook_reads_one_path_per_line() {
        let dir = temp_dir();
        let list = dir.path().join("picks.txt");
        // Fichier absent : annulation.
        assert!(e2e_picks(&list).is_empty());
        std::fs::write(&list, "/files/a b.txt\n\n  \n/files/photo.PNG\r\n").unwrap();
        assert_eq!(e2e_picks(&list), ["/files/a b.txt", "/files/photo.PNG"]);
        // Relu à chaque appel.
        std::fs::write(&list, "/files/other.md").unwrap();
        assert_eq!(e2e_picks(&list), ["/files/other.md"]);
        std::fs::write(&list, "").unwrap();
        assert!(e2e_picks(&list).is_empty());
    }
}
