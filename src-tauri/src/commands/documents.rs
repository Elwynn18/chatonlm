//! Enregistrement d'un document exporté par la page (src/lib/export : .md,
//! .docx, .pdf), propre à ChatonLM. La page produit les octets ; le moteur
//! ouvre la boîte « Enregistrer sous » native (filtrée sur le format, nom
//! proposé) et les écrit là où l'utilisateur l'a choisi. Un seul chemin pour
//! les trois formats, le même sous Linux et sous Windows ; la page n'a aucun
//! accès direct aux fichiers.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use tauri::WebviewWindow;
use tauri::http::HeaderMap;
use tauri::ipc::{InvokeBody, Request};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::i18n::t;

/// En-tête du nom proposé (sans extension), encodé par `encodeURIComponent`.
const FILENAME_HEADER: &str = "x-filename";
/// En-tête du format : `md`, `docx` ou `pdf`.
const KIND_HEADER: &str = "x-kind";

/// Taille maximale d'un document : bien au-delà d'une transcription de
/// plusieurs heures (quelques Mo en PDF), assez bas pour ne pas écrire
/// n'importe quoi sur le disque.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;

/// Longueur maximale du nom proposé, en caractères, sans l'extension.
const MAX_NAME_CHARS: usize = 100;

/// Format d'un document exporté.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Markdown,
    Word,
    Pdf,
}

impl Kind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "md" => Some(Self::Markdown),
            "docx" => Some(Self::Word),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Word => "docx",
            Self::Pdf => "pdf",
        }
    }

    /// Nom du filtre de la boîte.
    fn filter(self) -> String {
        match self {
            Self::Markdown => t!("documents.markdownFiles"),
            Self::Word => t!("documents.wordFiles"),
            Self::Pdf => t!("documents.pdfFiles"),
        }
    }
}

/// Enregistre un document : octets bruts dans le corps, nom proposé dans
/// `x-filename`, format dans `x-kind`. Chemin écrit, ou `None` si
/// l'utilisateur annule la boîte.
#[tauri::command]
pub async fn save_document(
    window: WebviewWindow,
    request: Request<'_>,
) -> AppResult<Option<String>> {
    let (kind, name, data) = parse_request(request.headers(), request.body())?;
    let data = data.to_vec();
    let Some(path) = pick_save_path(&window, kind, file_name(&name, kind)).await else {
        return Ok(None);
    };
    let written = tauri::async_runtime::spawn_blocking(move || write_document(path, kind, &data))
        .await
        .map_err(|error| AppError::Internal(error.to_string()))??;
    Ok(Some(written.to_string_lossy().into_owned()))
}

/// Format, nom proposé et contenu d'une requête. Erreurs de forme en anglais
/// (la page envoie toujours une requête valide) ; un document trop gros est
/// refusé par un message traduit.
fn parse_request<'a>(
    headers: &HeaderMap,
    body: &'a InvokeBody,
) -> AppResult<(Kind, String, &'a [u8])> {
    let kind = headers
        .get(KIND_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(Kind::parse)
        // i18n-ignore
        .ok_or_else(|| AppError::Invalid("missing or unknown x-kind header".into()))?;
    let name = headers
        .get(FILENAME_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| percent_decode_str(value).decode_utf8().ok())
        .map(|name| name.into_owned())
        // i18n-ignore
        .ok_or_else(|| AppError::Invalid("missing or invalid x-filename header".into()))?;
    let InvokeBody::Raw(data) = body else {
        // i18n-ignore : la page envoie toujours des octets bruts.
        return Err(AppError::Invalid(
            "the document must be sent as raw bytes".into(),
        ));
    };
    if data.len() > MAX_DOCUMENT_BYTES {
        return Err(AppError::Invalid(t!("documents.tooLarge")));
    }
    Ok((kind, name, data))
}

/// Nom proposé dans la boîte : `name` sans les caractères qu'un nom de
/// fichier ne peut pas porter sous Windows ou Linux (séparateurs, `:*?"<>|`,
/// caractères de contrôle), espaces réduites, ni point ni espace aux bouts,
/// 100 caractères au plus, jamais un nom réservé de Windows ; puis
/// l'extension du format.
fn file_name(name: &str, kind: Kind) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut stem: String = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    stem = stem
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    if stem.is_empty() {
        stem = t!("documents.defaultName");
    }
    if is_reserved_on_windows(&stem) {
        stem.push('_');
    }
    format!("{stem}.{}", kind.extension())
}

/// Noms de périphériques de Windows, refusés comme nom de fichier quelle
/// que soit l'extension.
fn is_reserved_on_windows(stem: &str) -> bool {
    let base = stem
        .split('.')
        .next()
        .unwrap_or(stem)
        .trim_end()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0')
}

/// Chemin choisi, avec l'extension du format si l'utilisateur l'a retirée
/// (la boîte GTK ne l'ajoute pas) ; tel quel si le fichier ainsi nommé
/// existe déjà, pour ne jamais en écraser un que la boîte n'a pas montré.
fn with_extension(path: PathBuf, kind: Kind, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let has = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(kind.extension()));
    if has {
        return path;
    }
    let mut extended = path.clone().into_os_string();
    extended.push(".");
    extended.push(kind.extension());
    let extended = PathBuf::from(extended);
    if exists(&extended) { path } else { extended }
}

/// Écrit le document au chemin choisi (extension ajoutée au besoin) ; chemin
/// écrit.
fn write_document(path: PathBuf, kind: Kind, data: &[u8]) -> AppResult<PathBuf> {
    let path = with_extension(path, kind, Path::exists);
    std::fs::write(&path, data)?;
    Ok(path)
}

/// Chemin choisi dans la boîte « Enregistrer sous » native, au-dessus de la
/// fenêtre ; `None` si l'utilisateur annule.
async fn pick_save_path(window: &WebviewWindow, kind: Kind, name: String) -> Option<PathBuf> {
    // Essais de l'app réelle : pas de boîte à piloter, le chemin « choisi »
    // est la valeur de la variable (vide : annulé), comme pour les notes d'un
    // cours ou une image.
    #[cfg(feature = "e2e")]
    if let Some(path) = std::env::var_os("CHATONLM_E2E_SAVE_FILE") {
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(t!("documents.save"))
        .set_file_name(name)
        .add_filter(kind.filter(), &[kind.extension()])
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    // Rappel jamais fait (boucle d'événements arrêtée) : comme une annulation.
    receiver
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}

#[cfg(test)]
mod tests {
    use tauri::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn a_request_gives_its_kind_decoded_name_and_bytes() {
        let body = InvokeBody::Raw(b"%PDF-1.3".to_vec());
        let (kind, name, data) = parse_request(
            &headers(&[
                ("x-kind", "pdf"),
                ("x-filename", "R%C3%A9union%20%C2%AB%20a%20%C2%BB"),
            ]),
            &body,
        )
        .unwrap();
        assert_eq!(kind, Kind::Pdf);
        assert_eq!(name, "Réunion « a »");
        assert_eq!(data, b"%PDF-1.3");
        for (value, expected) in [("md", Kind::Markdown), ("docx", Kind::Word)] {
            let (kind, ..) =
                parse_request(&headers(&[("x-kind", value), ("x-filename", "a")]), &body).unwrap();
            assert_eq!(kind, expected);
        }
    }

    #[test]
    fn a_malformed_or_too_large_request_is_refused() {
        let body = InvokeBody::Raw(vec![0]);
        let cases = [
            headers(&[("x-filename", "a")]),
            headers(&[("x-kind", "odt"), ("x-filename", "a")]),
            headers(&[("x-kind", "pdf")]),
            headers(&[("x-kind", "pdf"), ("x-filename", "%FF")]),
        ];
        for case in &cases {
            assert!(matches!(
                parse_request(case, &body),
                Err(AppError::Invalid(_))
            ));
        }
        let json = InvokeBody::Json(serde_json::json!({}));
        let ok = headers(&[("x-kind", "pdf"), ("x-filename", "a")]);
        assert!(matches!(
            parse_request(&ok, &json),
            Err(AppError::Invalid(_))
        ));
        let huge = InvokeBody::Raw(vec![0; MAX_DOCUMENT_BYTES + 1]);
        assert!(matches!(
            parse_request(&ok, &huge),
            Err(AppError::Invalid(_))
        ));
        let limit = InvokeBody::Raw(vec![0; MAX_DOCUMENT_BYTES]);
        assert!(parse_request(&ok, &limit).is_ok());
    }

    #[test]
    fn the_proposed_name_is_a_valid_file_name_on_both_systems() {
        assert_eq!(
            file_name("Réunion du 14/11", Kind::Pdf),
            "Réunion du 14 11.pdf"
        );
        assert_eq!(
            file_name(" a:b*c?\"d<e>f|g\\h\n\ti ", Kind::Word),
            "a b c d e f g h i.docx"
        );
        assert_eq!(file_name("...notes. ", Kind::Markdown), "notes.md");
        assert_eq!(file_name("CON", Kind::Pdf), "CON_.pdf");
        assert_eq!(file_name("com1", Kind::Pdf), "com1_.pdf");
        assert_eq!(file_name("COM10", Kind::Pdf), "COM10.pdf");
        let long = "é".repeat(300);
        assert_eq!(
            file_name(&long, Kind::Pdf).chars().count(),
            MAX_NAME_CHARS + 4
        );
        assert!(!file_name("  / ", Kind::Pdf).starts_with('.'));
        assert!(file_name("  / ", Kind::Pdf).ends_with(".pdf"));
    }

    #[test]
    fn the_extension_is_added_unless_present_or_the_file_exists() {
        let never = |_: &Path| false;
        let always = |_: &Path| true;
        assert_eq!(
            with_extension(PathBuf::from("/d/notes"), Kind::Pdf, never),
            PathBuf::from("/d/notes.pdf")
        );
        assert_eq!(
            with_extension(PathBuf::from("/d/notes.PDF"), Kind::Pdf, never),
            PathBuf::from("/d/notes.PDF")
        );
        assert_eq!(
            with_extension(PathBuf::from("/d/v1.2"), Kind::Word, never),
            PathBuf::from("/d/v1.2.docx")
        );
        assert_eq!(
            with_extension(PathBuf::from("/d/notes"), Kind::Markdown, always),
            PathBuf::from("/d/notes")
        );
    }

    #[test]
    fn the_document_is_written_at_the_chosen_path() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = "# Été\n« œuvre »".as_bytes();
        let written = write_document(dir.path().join("note"), Kind::Markdown, bytes).unwrap();
        assert_eq!(written, dir.path().join("note.md"));
        assert_eq!(std::fs::read(&written).unwrap(), bytes);
        // Chemin déjà au bon format : écrasé tel quel (la boîte l'a confirmé).
        let pdf = dir.path().join("a.pdf");
        std::fs::write(&pdf, b"old").unwrap();
        assert_eq!(
            write_document(pdf.clone(), Kind::Pdf, b"%PDF").unwrap(),
            pdf
        );
        assert_eq!(std::fs::read(&pdf).unwrap(), b"%PDF");
        // Dossier absent : erreur de fichier, rien d'écrit.
        let missing = dir.path().join("absent").join("x.docx");
        assert!(matches!(
            write_document(missing.clone(), Kind::Word, b"PK"),
            Err(AppError::Io(_))
        ));
        assert!(!missing.exists());
    }
}
