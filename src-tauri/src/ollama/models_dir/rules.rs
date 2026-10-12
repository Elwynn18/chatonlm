//! Dossier des modèles sous Windows : logique pure (formes des chemins,
//! dossiers refusés, lancement d'Ollama reconnu, commande à copier), sans
//! appel système, essayée aussi sous Linux. Les appels Win32 sont dans
//! `models_dir/windows.rs`.
//!
//! Les chemins sont traités comme du texte : sous Linux, `std::path` ne
//! connaît ni les lettres de lecteur ni `\`.

#![cfg_attr(not(windows), allow(dead_code))]

use super::{FolderProblem, ModelsLocation};

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

/// Même dossier.
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

/// Racine d'un lecteur ou d'un partage.
pub fn is_root(path: &str) -> bool {
    folders(path).is_some_and(|(_, folders)| folders.is_empty())
}

/// Dossiers du système où le dossier des modèles n'a pas sa place.
#[derive(Clone, Debug, Default)]
pub struct SystemFolders {
    /// Arbres refusés en entier : `%SystemRoot%`, `%ProgramFiles%`,
    /// `%ProgramFiles(x86)%`, `%ProgramData%`, dossier d'installation de
    /// ChatonLM.
    pub trees: Vec<String>,
    /// Dossiers refusés eux-mêmes (leurs sous-dossiers sont admis) :
    /// profil de l'utilisateur, dossier des profils.
    pub exact: Vec<String>,
}

/// Problème d'un chemin canonique, d'après sa forme seulement (le contenu
/// du dossier est vérifié ensuite) : relatif, caractère de contrôle,
/// dossier du système, dossier actuel (`old`) ou l'un dans l'autre.
pub fn path_problem(
    path: &str,
    old: Option<&str>,
    system: &SystemFolders,
) -> Option<FolderProblem> {
    if volume(path).is_none() {
        return Some(FolderProblem::Missing);
    }
    if path.chars().any(char::is_control) {
        return Some(FolderProblem::BadCharacters);
    }
    if is_root(path)
        || system.trees.iter().any(|tree| within(path, tree))
        || system.exact.iter().any(|folder| same(path, folder))
    {
        return Some(FolderProblem::System);
    }
    if let Some(old) = old {
        if same(path, old) {
            return Some(FolderProblem::Same);
        }
        if within(path, old) || within(old, path) {
            return Some(FolderProblem::Inside);
        }
    }
    None
}

/// Mot entre apostrophes pour PowerShell (apostrophe doublée) : aucune
/// variable ni commande n'y est interprétée.
pub fn powershell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "''"))
}

/// Commande à copier pour relancer `ollama serve` (lancé à la main) sur un
/// autre dossier, en PowerShell ; `host_env` : valeur de `OLLAMA_HOST` à
/// redonner (port non standard).
pub fn manual_command(path: &str, host_env: Option<&str>) -> String {
    let host = host_env
        .map(|host| format!("$env:OLLAMA_HOST = {}; ", powershell_quote(host)))
        .unwrap_or_default();
    format!(
        "{host}$env:OLLAMA_MODELS = {}; ollama serve",
        powershell_quote(path)
    )
}

/// Dossier des modèles probable de l'Ollama de la machine : la variable
/// d'utilisateur `OLLAMA_MODELS`, sinon celle du système, sinon
/// `%USERPROFILE%\.ollama\models`. Le réglage de l'app officielle, qui
/// l'emporte quand il existe, n'est connu que d'elle.
pub fn probable_dir(
    user_variable: Option<&str>,
    machine_variable: Option<&str>,
    profile: Option<&str>,
) -> Option<String> {
    let set = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(plain)
    };
    set(user_variable)
        .or_else(|| set(machine_variable))
        .or_else(|| profile.map(|profile| format!(r"{}\.ollama\models", plain(profile))))
}

/// Programme qui écoute à l'adresse réglée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listener {
    /// Nom de son exécutable et de celui de son parent.
    Process {
        name: String,
        parent: Option<String>,
    },
    /// Processus que le système ne laisse pas lire (autre compte, service).
    Hidden,
}

/// Relais de WSL et de Docker Desktop : l'écoute sur l'hôte est la leur,
/// Ollama tourne derrière.
const RELAYS: [&str; 6] = [
    "wslrelay.exe",
    "wslhost.exe",
    "com.docker.backend.exe",
    "docker-proxy.exe",
    "vpnkit.exe",
    "gvproxy.exe",
];

/// Où Ollama range ses modèles, d'après ce qui écoute à l'adresse réglée.
/// - `ollama.exe` lancé par `ollama app.exe` : l'app de bureau ;
/// - `ollama.exe` lancé autrement : à la main ;
/// - relais de WSL ou de Docker : conteneur ;
/// - rien n'écoute sur le port standard et l'app de bureau est installée :
///   c'est elle qui servira ;
/// - sinon inconnu.
pub fn location(
    listener: Option<&Listener>,
    app_installed: bool,
    standard_port: bool,
    answers: bool,
    dir: Option<String>,
    host_env: Option<String>,
) -> ModelsLocation {
    let is = |name: &str, expected: &str| name.eq_ignore_ascii_case(expected);
    match listener {
        Some(Listener::Process { name, parent }) if is(name, "ollama.exe") => {
            if parent
                .as_deref()
                .is_some_and(|parent| is(parent, "ollama app.exe"))
            {
                ModelsLocation::DesktopApp { dir }
            } else {
                ModelsLocation::Manual { dir, host_env }
            }
        }
        Some(Listener::Process { name, .. }) if is(name, "ollama app.exe") => {
            ModelsLocation::DesktopApp { dir }
        }
        Some(Listener::Process { name, .. }) if RELAYS.iter().any(|relay| is(name, relay)) => {
            ModelsLocation::Container
        }
        Some(_) => ModelsLocation::Unknown { running: true },
        None if app_installed && standard_port => ModelsLocation::DesktopApp { dir },
        None => ModelsLocation::Unknown { running: answers },
    }
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
        assert!(is_root(r"D:\") && is_root(r"\\nas\share") && !is_root(r"D:\Models"));
    }

    fn system() -> SystemFolders {
        SystemFolders {
            trees: vec![
                r"C:\Windows".into(),
                r"C:\Program Files".into(),
                r"C:\ProgramData".into(),
            ],
            exact: vec![r"C:\Users\me".into(), r"C:\Users".into()],
        }
    }

    #[test]
    fn system_folders_and_the_current_one_are_refused() {
        let problem = |path: &str, old: Option<&str>| path_problem(path, old, &system());
        let old = Some(r"C:\Users\me\AppData\Local\ChatonLM\ollama\models");
        assert_eq!(problem(r"D:\Models", old), None);
        assert_eq!(problem(r"C:\Users\me\Models", old), None);
        for refused in [
            r"C:\",
            r"D:\",
            r"C:\Windows\Temp\models",
            r"c:\program files\Ollama",
            r"C:\ProgramData",
            r"C:\Users\me",
            r"C:\Users",
        ] {
            assert_eq!(
                problem(refused, old),
                Some(FolderProblem::System),
                "{refused}"
            );
        }
        assert_eq!(problem(r"models", old), Some(FolderProblem::Missing));
        assert_eq!(
            problem("D:\\Mod\u{7}els", old),
            Some(FolderProblem::BadCharacters)
        );
        assert_eq!(
            problem(r"c:\users\ME\appdata\local\chatonlm\OLLAMA\models", old),
            Some(FolderProblem::Same)
        );
        assert_eq!(
            problem(r"C:\Users\me\AppData\Local\ChatonLM\ollama\models\sub", old),
            Some(FolderProblem::Inside)
        );
        assert_eq!(
            problem(r"C:\Users\me\AppData\Local\ChatonLM", old),
            Some(FolderProblem::Inside)
        );
        // Sans dossier actuel connu : seule la forme compte.
        assert_eq!(problem(r"D:\Models", None), None);
    }

    #[test]
    fn the_command_to_copy_is_powershell() {
        assert_eq!(
            manual_command(r"D:\Models", None),
            r"$env:OLLAMA_MODELS = 'D:\Models'; ollama serve"
        );
        assert_eq!(
            manual_command(r"D:\It's here", Some("127.0.0.1:11500")),
            r"$env:OLLAMA_HOST = '127.0.0.1:11500'; $env:OLLAMA_MODELS = 'D:\It''s here'; ollama serve"
        );
    }

    #[test]
    fn the_probable_folder_follows_ollama_s_own_order() {
        let profile = Some(r"C:\Users\me");
        assert_eq!(
            probable_dir(Some(r"D:\Models\"), Some(r"E:\Other"), profile).as_deref(),
            Some(r"D:\Models")
        );
        assert_eq!(
            probable_dir(Some("  "), Some(r"E:\Other"), profile).as_deref(),
            Some(r"E:\Other")
        );
        assert_eq!(
            probable_dir(None, None, profile).as_deref(),
            Some(r"C:\Users\me\.ollama\models")
        );
        assert_eq!(probable_dir(None, None, None), None);
    }

    #[test]
    fn the_launch_is_told_from_what_listens() {
        let dir = || Some(r"C:\Users\me\.ollama\models".to_owned());
        let process = |name: &str, parent: Option<&str>| Listener::Process {
            name: name.into(),
            parent: parent.map(str::to_owned),
        };
        let found = |listener: Option<Listener>, app: bool, standard: bool, answers: bool| {
            location(
                listener.as_ref(),
                app,
                standard,
                answers,
                dir(),
                Some("127.0.0.1:11500".into()),
            )
        };
        assert_eq!(
            found(
                Some(process("ollama.exe", Some("ollama app.exe"))),
                true,
                true,
                true
            ),
            ModelsLocation::DesktopApp { dir: dir() }
        );
        assert_eq!(
            found(
                Some(process("OLLAMA.EXE", Some("powershell.exe"))),
                true,
                true,
                true
            ),
            ModelsLocation::Manual {
                dir: dir(),
                host_env: Some("127.0.0.1:11500".into())
            }
        );
        assert_eq!(
            found(Some(process("ollama.exe", None)), false, true, true),
            ModelsLocation::Manual {
                dir: dir(),
                host_env: Some("127.0.0.1:11500".into())
            }
        );
        assert_eq!(
            found(Some(process("wslrelay.exe", None)), true, true, true),
            ModelsLocation::Container
        );
        assert_eq!(
            found(Some(process("python.exe", None)), true, true, true),
            ModelsLocation::Unknown { running: true }
        );
        assert_eq!(
            found(Some(Listener::Hidden), true, true, true),
            ModelsLocation::Unknown { running: true }
        );
        // Rien n'écoute : l'app de bureau installée servira, sur son port.
        assert_eq!(
            found(None, true, true, false),
            ModelsLocation::DesktopApp { dir: dir() }
        );
        assert_eq!(
            found(None, true, false, false),
            ModelsLocation::Unknown { running: false }
        );
        assert_eq!(
            found(None, false, true, true),
            ModelsLocation::Unknown { running: true }
        );
    }
}
