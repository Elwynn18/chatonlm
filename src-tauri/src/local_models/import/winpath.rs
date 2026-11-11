//! Chemins Windows : logique pure (formes des chemins, comparaison sans la
//! casse, volume), sans appel système, essayée aussi sous Linux. Les chemins
//! sont traités comme du texte : sous Linux, `std::path` ne connaît ni les
//! lettres de lecteur ni `\`.

#![cfg_attr(not(windows), allow(dead_code))]

/// Chemin Windows sous une seule forme : sans préfixe `\\?\`, séparateurs
/// `\`, sans séparateur final (sauf la racine d'un lecteur, `C:\`).
pub fn plain(path: &str) -> String {
    let path = path.replace('/', "\\");
    let path = match path.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned(),
    };
    let trimmed = path.trim_end_matches('\\');
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        return format!("{trimmed}\\");
    }
    trimmed.to_owned()
}

/// Volume d'un chemin absolu : `C:` (lettre en capitale), ou
/// `\\serveur\partage` en minuscules ; `None` pour un chemin relatif.
pub fn volume(path: &str) -> Option<String> {
    let path = plain(path);
    if let Some(rest) = path.strip_prefix(r"\\") {
        let mut parts = rest.split('\\').filter(|part| !part.is_empty());
        let (server, share) = (parts.next()?, parts.next()?);
        return Some(format!(r"\\{server}\{share}").to_lowercase());
    }
    let mut chars = path.chars();
    let (letter, colon) = (chars.next()?, chars.next()?);
    let rooted = matches!(chars.next(), Some('\\'));
    (letter.is_ascii_alphabetic() && colon == ':' && rooted)
        .then(|| format!("{}:", letter.to_ascii_uppercase()))
}

/// Dossiers du chemin sous son volume, en minuscules (les noms de fichiers
/// ne tiennent pas compte de la casse sous Windows).
fn folders(path: &str) -> Option<(String, Vec<String>)> {
    let volume = volume(path)?;
    let path = plain(path);
    let rest = path.get(volume.len()..).unwrap_or_default();
    Some((
        volume,
        rest.split('\\')
            .filter(|part| !part.is_empty())
            .map(str::to_lowercase)
            .collect(),
    ))
}

/// Même dossier, sans la casse.
pub fn same(a: &str, b: &str) -> bool {
    folders(a).is_some() && folders(a) == folders(b)
}

/// `path` est `root` ou un dossier sous `root`.
pub fn within(path: &str, root: &str) -> bool {
    match (folders(path), folders(root)) {
        (Some((volume, path)), Some((root_volume, root))) => {
            volume == root_volume && path.starts_with(&root)
        }
        _ => false,
    }
}

/// Dossiers des modèles réglés pour l'Ollama de la machine : la variable
/// d'utilisateur `OLLAMA_MODELS`, puis celle du système (valeurs vides
/// ignorées), lues comme Ollama (`envconfig.Var` : espaces puis guillemets
/// retirés, [`super::sources::env_value`]). Le réglage de l'app officielle
/// d'Ollama, qui l'emporte quand il existe, n'est connu que d'elle ;
/// `%USERPROFILE%\.ollama\models` est ajouté par l'appelant (commun aux
/// deux systèmes).
pub fn configured_dirs(user_variable: Option<&str>, machine_variable: Option<&str>) -> Vec<String> {
    [user_variable, machine_variable]
        .into_iter()
        .flatten()
        .map(super::sources::env_value)
        .filter(|value| !value.is_empty())
        .map(|value| plain(&value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_take_one_form() {
        assert_eq!(plain(r"\\?\C:\Users\me\Models\"), r"C:\Users\me\Models");
        assert_eq!(plain("D:/Models/"), r"D:\Models");
        assert_eq!(plain(r"C:\"), r"C:\");
        assert_eq!(plain("c:"), r"c:\");
        assert_eq!(plain(r"\\?\UNC\nas\share\models"), r"\\nas\share\models");
        assert_eq!(volume(r"c:\Users"), Some("C:".into()));
        assert_eq!(volume(r"\\NAS\Share\models"), Some(r"\\nas\share".into()));
        assert_eq!(volume(r"Models\here"), None);
        assert_eq!(volume(r"C:relative"), None);
        assert_eq!(volume("/home/me/models"), None);
    }

    #[test]
    fn folders_are_compared_without_case() {
        assert!(same(r"C:\Models", r"c:\models\"));
        assert!(!same(r"C:\Models", r"D:\Models"));
        assert!(within(r"C:\Windows\System32", r"c:\windows"));
        assert!(within(r"C:\Windows", r"C:\Windows"));
        assert!(!within(r"C:\WindowsApps", r"C:\Windows"));
        assert!(!within(r"D:\Windows", r"C:\Windows"));
    }

    #[test]
    fn configured_folders_follow_ollama_s_own_order() {
        assert_eq!(
            configured_dirs(Some(r"D:\Models\"), Some(r"E:\Shared")),
            [r"D:\Models", r"E:\Shared"]
        );
        assert_eq!(
            configured_dirs(Some("  "), Some(r"E:\Shared")),
            [r"E:\Shared"]
        );
        assert!(configured_dirs(None, Some("")).is_empty());
        // Guillemets saisis dans l'éditeur des variables : retirés, comme
        // Ollama le fait.
        assert_eq!(
            configured_dirs(Some(r#" "D:\Models" "#), Some(r"'E:\Shared'")),
            [r"D:\Models", r"E:\Shared"]
        );
        assert!(configured_dirs(Some(r#""""#), None).is_empty());
    }
}
