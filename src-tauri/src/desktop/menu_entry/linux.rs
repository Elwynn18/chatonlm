//! Inscription au menu des applications d'une AppImage (étape 18a, choix 3a
//! de l'utilisateur). Les paquets installent eux-mêmes leur fichier
//! `.desktop` ; une AppImage n'installe rien. Lancée depuis une AppImage
//! (`APPIMAGE`, chemin absolu d'un fichier existant), ChatonLM écrit donc
//! `$XDG_DATA_HOME/applications/<identifiant>.desktop` (Exec et TryExec : le
//! chemin de l'AppImage) et son icône 256 px sous
//! `$XDG_DATA_HOME/icons/hicolor/256x256/apps/<identifiant>.png`, à chaque
//! démarrage tant que le réglage le veut : un fichier qui ne change pas n'est
//! pas réécrit, une AppImage déplacée met le chemin à jour. Le même fichier
//! sert au portail des raccourcis globaux (`register_host_app`) et à KDE pour
//! associer la fenêtre (app_id Wayland = identifiant) à son entrée.
//!
//! Icône écrite, remplacée (nouvelle version de ChatonLM) ou retirée : les
//! bureaux en sont prévenus ([`mark_theme_changed`], [`announce_icon_change`]),
//! sinon ils gardent celle qu'ils ont déjà chargée.
//!
//! Un fichier sans la clé [`MARKER`] (copie personnalisée, autre
//! installation) n'est jamais écrasé ni supprimé. Écritures atomiques
//! (fichier caché du même dossier, puis renommage).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tauri::AppHandle;

use super::MenuEntryStatus;

/// Entrée des paquets (écrite par `packaging/`), reprise telle quelle sauf
/// Exec, TryExec, Icon et StartupWMClass.
const TEMPLATE: &str = include_str!("../../../../packaging/linux/com.chatonlm.chatonlm.desktop");

/// Icône du menu, écrite sous le nom de l'identifiant.
const ICON: &[u8] = include_bytes!("../../../icons/256x256.png");

/// Clé ajoutée aux fichiers écrits par ChatonLM : seuls ceux-là sont mis à
/// jour ou supprimés.
const MARKER: &str = "X-ChatonLM-AppImage=true";

/// Groupe principal d'un fichier `.desktop`.
const MAIN_GROUP: &str = "[Desktop Entry]";

/// Caractères qui obligent à mettre un argument de l'Exec entre guillemets
/// (spécification Desktop Entry, « The Exec key »).
const RESERVED: &[char] = &[
    ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')',
    '`',
];

/// Fichiers de l'entrée sous `XDG_DATA_HOME`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub desktop: PathBuf,
    pub icon: PathBuf,
}

impl Paths {
    pub fn new(data_home: &Path, identifier: &str) -> Self {
        Self {
            desktop: data_home
                .join("applications")
                .join(format!("{identifier}.desktop")),
            icon: data_home
                .join("icons/hicolor/256x256/apps")
                .join(format!("{identifier}.png")),
        }
    }
}

/// Ce que l'écriture ou le retrait a fait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Entrée ou icône écrite (nouvelle, ou chemin de l'AppImage changé).
    Written,
    /// Entrée et icône retirées (au moins l'une des deux existait).
    Removed,
    /// Rien à faire.
    Unchanged,
    /// Un fichier sans [`MARKER`] porte ce nom : laissé tel quel.
    Foreign,
}

/// Où écrire, si ChatonLM tourne depuis une AppImage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    appimage: PathBuf,
    identifier: String,
    paths: Paths,
}

impl Target {
    /// D'après l'environnement : `None` hors AppImage (ou sans dossier de
    /// données utilisable).
    pub fn current(identifier: &str) -> Option<Self> {
        Self::from_env(
            identifier,
            std::env::var_os("APPIMAGE"),
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("HOME"),
        )
    }

    fn from_env(
        identifier: &str,
        appimage: Option<OsString>,
        xdg_data_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Option<Self> {
        let appimage = appimage_from(appimage)?;
        let data_home = data_home_from(xdg_data_home, home)?;
        Some(Self {
            appimage,
            identifier: identifier.to_owned(),
            paths: Paths::new(&data_home, identifier),
        })
    }

    /// Écrit l'entrée (`enabled`) ou la retire, puis prévient les bureaux si
    /// l'icône a changé (même quand l'entrée, elle, n'a pas pu être écrite).
    pub fn apply(&self, enabled: bool) -> io::Result<Outcome> {
        let before = fs::read(&self.paths.icon).ok();
        let outcome = if enabled {
            install(&self.paths, &self.appimage, &self.identifier)
        } else {
            remove(&self.paths)
        };
        if fs::read(&self.paths.icon).ok() != before {
            mark_theme_changed(&self.paths.icon);
            // Jamais dans les essais unitaires : le bus serait celui du
            // bureau du développeur (e2e/menu-entry.test.mjs le vérifie).
            #[cfg(not(test))]
            announce_icon_change();
        }
        outcome
    }
}

/// Marque le thème d'icônes (`…/icons/hicolor`, trois dossiers au-dessus de
/// l'icône) comme changé : sa date de modification est celle que GTK et les
/// outils de thèmes comparent à leur cache (`icon-theme.cache`), et renommer
/// un fichier dans `256x256/apps` ne la change pas. Sans cela, un cache plus
/// ancien que l'icône continue de servir.
fn mark_theme_changed(icon: &Path) {
    let Some(theme) = icon.ancestors().nth(3) else {
        return;
    };
    let marked =
        fs::File::open(theme).and_then(|theme| theme.set_modified(std::time::SystemTime::now()));
    if let Err(error) = marked {
        eprintln!(
            "chatonlm: icon theme {} not marked as changed: {error}",
            theme.display()
        );
    }
}

/// Prévient KDE que des icônes ont changé : signal de `KIconLoader`, celui
/// d'un changement de thème d'icônes. Sans lui, Plasma garde en mémoire
/// l'icône qu'il a déjà affichée (barre des tâches) jusqu'à la fin de la
/// session, même après une mise à jour de ChatonLM. Sans effet ailleurs.
/// Bloquant : à appeler hors du runtime asynchrone.
#[cfg(not(test))]
fn announce_icon_change() {
    /// `KIconLoader::Group` : le premier suffit, tout récepteur relit son thème.
    const DESKTOP_GROUP: i32 = 0;
    let sent = zbus::blocking::Connection::session().and_then(|connection| {
        connection.emit_signal(
            None::<&str>,
            "/KIconLoader",
            "org.kde.KIconLoader",
            "iconChanged",
            &(DESKTOP_GROUP,),
        )
    });
    if let Err(error) = sent {
        eprintln!("chatonlm: icon change not announced to the desktop: {error}");
    }
}

/// Chemin de l'AppImage : absolu, vers un fichier existant.
fn appimage_from(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// `$XDG_DATA_HOME` s'il est absolu, sinon `$HOME/.local/share`
/// (spécification XDG Base Directory : valeur vide ou relative ignorée).
fn data_home_from(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute =
        |value: Option<OsString>| value.map(PathBuf::from).filter(|path| path.is_absolute());
    absolute(xdg_data_home).or_else(|| absolute(home).map(|home| home.join(".local/share")))
}

/// Valeur de type « string » d'un fichier `.desktop` : la barre oblique
/// inverse s'écrit `\\` (règle appliquée avant celles de l'Exec).
fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\")
}

/// Argument de l'Exec : entre guillemets s'il contient un caractère réservé
/// (`"`, `` ` ``, `$` et `\` alors précédés de `\`), `%` doublé (codes de
/// champ), puis échappé comme une chaîne.
fn exec_argument(word: &str) -> String {
    let mut argument = String::with_capacity(word.len() + 2);
    if word.is_empty() || word.contains(RESERVED) {
        argument.push('"');
        for c in word.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                argument.push('\\');
            }
            argument.push(c);
        }
        argument.push('"');
    } else {
        argument.push_str(word);
    }
    escape_string(&argument.replace('%', "%%"))
}

/// Entrée de `template` pour l'AppImage `appimage` : Exec (premier mot de
/// chaque ligne `Exec=`, arguments gardés) et TryExec = l'AppImage, Icon et
/// StartupWMClass = l'identifiant, et la clé [`MARKER`]. `None` si le chemin
/// ne peut pas s'écrire dans un fichier `.desktop` (caractère de contrôle)
/// ou si le modèle n'a pas d'Exec dans son groupe principal.
fn render(template: &str, appimage: &str, identifier: &str) -> Option<String> {
    if appimage.chars().any(char::is_control) {
        return None;
    }
    let program = exec_argument(appimage);
    let mut lines = Vec::new();
    let mut in_main = false;
    let mut main_exec = false;
    for line in template.lines() {
        if line.starts_with('[') {
            in_main = line.trim_end() == MAIN_GROUP;
            lines.push(line.to_owned());
        } else if let Some(command) = line.strip_prefix("Exec=") {
            let arguments = command.split_once(' ').map_or("", |(_, rest)| rest);
            lines.push(if arguments.is_empty() {
                format!("Exec={program}")
            } else {
                format!("Exec={program} {arguments}")
            });
            if in_main && !main_exec {
                main_exec = true;
                lines.push(format!("TryExec={}", escape_string(appimage)));
                lines.push(MARKER.to_owned());
            }
        } else if line.starts_with("TryExec=") || line.starts_with("X-ChatonLM-AppImage=") {
            // Remplacées à côté de l'Exec du groupe principal.
        } else if line.starts_with("Icon=") {
            lines.push(format!("Icon={identifier}"));
        } else if line.starts_with("StartupWMClass=") {
            lines.push(format!("StartupWMClass={identifier}"));
        } else {
            lines.push(line.to_owned());
        }
    }
    main_exec.then(|| lines.join("\n") + "\n")
}

/// Le fichier a été écrit par ChatonLM.
fn is_ours(content: &[u8]) -> bool {
    content
        .split(|&byte| byte == b'\n')
        .any(|line| line.trim_ascii() == MARKER.as_bytes())
}

/// Écrit l'entrée de `appimage` et l'icône, sauf si un fichier qui n'est pas
/// de ChatonLM porte déjà ce nom.
pub fn install(paths: &Paths, appimage: &Path, identifier: &str) -> io::Result<Outcome> {
    let content = appimage
        .to_str()
        .and_then(|appimage| render(TEMPLATE, appimage, identifier))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "AppImage path not usable in a desktop entry: {}",
                    appimage.display()
                ),
            )
        })?;
    install_content(paths, &content, ICON)
}

fn install_content(paths: &Paths, content: &str, icon: &[u8]) -> io::Result<Outcome> {
    match fs::read(&paths.desktop) {
        Ok(current) if !is_ours(&current) => return Ok(Outcome::Foreign),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // Icône d'abord : le menu la trouve dès que l'entrée apparaît.
    let icon_written = write_if_changed(&paths.icon, icon)?;
    let entry_written = write_if_changed(&paths.desktop, content.as_bytes())?;
    Ok(if icon_written || entry_written {
        Outcome::Written
    } else {
        Outcome::Unchanged
    })
}

/// Retire l'entrée et l'icône (fichiers de ChatonLM seulement).
pub fn remove(paths: &Paths) -> io::Result<Outcome> {
    let entry_removed = match fs::read(&paths.desktop) {
        Ok(current) if !is_ours(&current) => return Ok(Outcome::Foreign),
        Ok(_) => remove_if_exists(&paths.desktop)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let icon_removed = remove_if_exists(&paths.icon)?;
    Ok(if entry_removed || icon_removed {
        Outcome::Removed
    } else {
        Outcome::Unchanged
    })
}

fn remove_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Écrit `bytes` si le fichier ne les contient pas déjà ; vrai s'il a été
/// écrit.
fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(false);
    }
    write_atomic(path, bytes)?;
    Ok(true)
}

/// Fichier caché du même dossier, puis renommage : le menu ne lit jamais un
/// fichier à moitié écrit.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid path: {}", path.display()),
        ));
    };
    fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{}.chatonlm-tmp", name.to_string_lossy()));
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Démarrage (avant la liaison des raccourcis globaux, qui a besoin du
/// fichier) : entrée écrite ou mise à jour si ChatonLM tourne depuis une
/// AppImage et que le réglage la veut ; rien sinon (un retrait est
/// respecté).
pub async fn at_startup(app: &AppHandle, enabled: bool) {
    if !enabled {
        return;
    }
    let Some(target) = Target::current(&app.config().identifier) else {
        return;
    };
    let path = target.paths.desktop.clone();
    match tauri::async_runtime::spawn_blocking(move || target.apply(true)).await {
        Ok(Ok(Outcome::Written)) => {
            eprintln!(
                "chatonlm: applications menu entry written: {}",
                path.display()
            );
        }
        Ok(Ok(Outcome::Foreign)) => eprintln!(
            "chatonlm: {} was not written by ChatonLM, left as is",
            path.display()
        ),
        Ok(Ok(_)) => {}
        Ok(Err(error)) => eprintln!("chatonlm: applications menu entry not written: {error}"),
        Err(error) => eprintln!("chatonlm: applications menu entry not written: {error}"),
    }
}

/// État de la ligne de Settings › General.
pub fn status(app: &AppHandle, enabled: bool) -> MenuEntryStatus {
    MenuEntryStatus {
        available: Target::current(&app.config().identifier).is_some(),
        enabled,
    }
}

/// Réglage changé : entrée écrite ou retirée tout de suite (hors AppImage,
/// rien). Erreur : la raison du système, en anglais (affichée dans le
/// message traduit de la commande).
pub async fn apply(app: &AppHandle, enabled: bool) -> Result<MenuEntryStatus, String> {
    let target = Target::current(&app.config().identifier);
    let available = target.is_some();
    if let Some(target) = target {
        tauri::async_runtime::spawn_blocking(move || target.apply(enabled))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    }
    Ok(MenuEntryStatus { available, enabled })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "com.chatonlm.chatonlm";

    /// Valeur relue comme un lanceur conforme à la spécification : chaîne
    /// déséchappée, puis arguments de l'Exec (guillemets, échappements,
    /// codes de champ).
    fn parse_exec(value: &str) -> Vec<String> {
        let mut unescaped = String::new();
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('\\') => unescaped.push('\\'),
                    Some('s') => unescaped.push(' '),
                    Some(other) => {
                        unescaped.push('\\');
                        unescaped.push(other);
                    }
                    None => panic!("trailing backslash in {value:?}"),
                }
            } else {
                unescaped.push(c);
            }
        }
        let mut arguments = Vec::new();
        let mut current = String::new();
        let mut started = false;
        let mut chars = unescaped.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    started = true;
                    loop {
                        match chars.next().expect("unterminated quote") {
                            '"' => break,
                            '\\' => {
                                let escaped = chars.next().expect("dangling escape");
                                assert!(
                                    matches!(escaped, '"' | '`' | '$' | '\\'),
                                    "invalid escape \\{escaped} in {value:?}"
                                );
                                current.push(escaped);
                            }
                            '%' => {
                                assert_eq!(chars.next(), Some('%'), "field code in {value:?}");
                                current.push('%');
                            }
                            other => current.push(other),
                        }
                    }
                }
                ' ' => {
                    if started {
                        arguments.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                '%' => {
                    started = true;
                    match chars.next() {
                        Some('%') => current.push('%'),
                        Some(code) => {
                            // Code de champ (%U, %F...) : gardé tel quel.
                            current.push('%');
                            current.push(code);
                        }
                        None => panic!("trailing % in {value:?}"),
                    }
                }
                other => {
                    assert!(
                        !RESERVED.contains(&other),
                        "unquoted reserved {other:?} in {value:?}"
                    );
                    started = true;
                    current.push(other);
                }
            }
        }
        if started {
            arguments.push(current);
        }
        arguments
    }

    fn key<'a>(content: &'a str, name: &str) -> Vec<&'a str> {
        let prefix = format!("{name}=");
        content
            .lines()
            .filter_map(|line| line.strip_prefix(prefix.as_str()))
            .collect()
    }

    #[test]
    fn paths_follow_the_xdg_specification() {
        let home = || Some(OsString::from("/home/me"));
        assert_eq!(
            data_home_from(None, home()),
            Some(PathBuf::from("/home/me/.local/share"))
        );
        assert_eq!(
            data_home_from(Some("".into()), home()),
            Some(PathBuf::from("/home/me/.local/share"))
        );
        // Relatif : ignoré.
        assert_eq!(
            data_home_from(Some("data".into()), home()),
            Some(PathBuf::from("/home/me/.local/share"))
        );
        assert_eq!(
            data_home_from(Some("/x/data".into()), home()),
            Some(PathBuf::from("/x/data"))
        );
        assert_eq!(data_home_from(None, None), None);
        assert_eq!(data_home_from(None, Some("me".into())), None);
        assert_eq!(
            Paths::new(Path::new("/x/data"), ID),
            Paths {
                desktop: "/x/data/applications/com.chatonlm.chatonlm.desktop".into(),
                icon: "/x/data/icons/hicolor/256x256/apps/com.chatonlm.chatonlm.png".into(),
            }
        );
    }

    #[test]
    fn only_an_absolute_path_to_an_existing_file_is_an_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ChatonLM.AppImage");
        fs::write(&file, b"").unwrap();
        let target = |appimage: Option<OsString>| {
            Target::from_env(ID, appimage, Some(dir.path().into()), None)
        };
        assert_eq!(target(None), None);
        assert_eq!(target(Some("".into())), None);
        assert_eq!(target(Some("ChatonLM.AppImage".into())), None);
        assert_eq!(target(Some(dir.path().join("missing").into())), None);
        // Un dossier n'est pas une AppImage.
        assert_eq!(target(Some(dir.path().into())), None);
        let found = target(Some(file.clone().into())).unwrap();
        assert_eq!(found.appimage, file);
        assert_eq!(found.paths, Paths::new(dir.path(), ID));
        // Sans dossier de données : rien.
        assert_eq!(Target::from_env(ID, Some(file.into()), None, None), None);
    }

    #[test]
    fn exec_arguments_are_quoted_and_escaped_as_the_specification_says() {
        assert_eq!(
            exec_argument("/opt/ChatonLM.AppImage"),
            "/opt/ChatonLM.AppImage"
        );
        assert_eq!(
            exec_argument("/home/me/My Apps/ChatonLM.AppImage"),
            "\"/home/me/My Apps/ChatonLM.AppImage\""
        );
        assert_eq!(
            exec_argument("/home/me/L'app.AppImage"),
            "\"/home/me/L'app.AppImage\""
        );
        // `$` échappé dans les guillemets, puis la barre échappée comme chaîne.
        assert_eq!(exec_argument("/a/$HOME"), "\"/a/\\\\$HOME\"");
        assert_eq!(exec_argument("/a/100%"), "/a/100%%");
        assert_eq!(exec_argument("/a b/100%"), "\"/a b/100%%\"");
        assert_eq!(exec_argument("/a/\\b"), "\"/a/\\\\\\\\b\"");
        assert_eq!(exec_argument("/a/`b`\""), "\"/a/\\\\`b\\\\`\\\\\"\"");
        for path in [
            "/opt/ChatonLM.AppImage",
            "/home/me/My Apps/ChatonLM.AppImage",
            "/home/me/L'app$(x).AppImage",
            "/home/me/100% sûr/ChatonLM.AppImage",
            "/home/me/%U/a\\b`c\"d;e|f&g*h?i#j~k<l>m.AppImage",
            "/home/me/$HOME",
        ] {
            assert_eq!(parse_exec(&exec_argument(path)), [path], "{path}");
            // Arguments du modèle gardés après le programme.
            assert_eq!(
                parse_exec(&format!("{} --dictate %U", exec_argument(path))),
                [path, "--dictate", "%U"],
                "{path}"
            );
        }
    }

    #[test]
    fn the_package_entry_becomes_the_appimage_entry() {
        let appimage = "/home/me/Apps/ChatonLM 0.9 (beta)$.AppImage";
        let content = render(TEMPLATE, appimage, ID).unwrap();
        let exec = key(&content, "Exec");
        assert_eq!(exec.len(), 1, "{content}");
        assert_eq!(parse_exec(exec[0]), [appimage]);
        // TryExec : une chaîne simple (pas de guillemets).
        assert_eq!(key(&content, "TryExec"), [appimage]);
        assert_eq!(key(&content, "Icon"), [ID]);
        assert_eq!(key(&content, "StartupWMClass"), [ID]);
        assert_eq!(key(&content, "X-ChatonLM-AppImage"), ["true"]);
        assert!(is_ours(content.as_bytes()));
        // Le reste du modèle (noms traduits...) est gardé.
        assert_eq!(key(&content, "Name"), ["ChatonLM"]);
        for line in TEMPLATE.lines().filter(|line| {
            !["Exec=", "TryExec=", "Icon=", "StartupWMClass="]
                .iter()
                .any(|key| line.starts_with(key))
        }) {
            assert!(content.lines().any(|kept| kept == line), "{line}");
        }
        assert!(content.lines().any(|line| line == MAIN_GROUP));
        // Un chemin avec une barre oblique inverse est relu tel quel.
        let content = render(TEMPLATE, "/a\\b.AppImage", ID).unwrap();
        assert_eq!(parse_exec(key(&content, "Exec")[0]), ["/a\\b.AppImage"]);
        assert_eq!(key(&content, "TryExec"), ["/a\\\\b.AppImage"]);
    }

    #[test]
    fn actions_keep_their_arguments_and_old_keys_are_replaced() {
        let template = "[Desktop Entry]\nName=ChatonLM\nExec=chatonlm %U\nTryExec=chatonlm\n\
            X-ChatonLM-AppImage=false\nIcon=chatonlm\n\n[Desktop Action dictate]\n\
            Name=Dictate\nExec=chatonlm --dictate\n";
        let content = render(template, "/opt/L.AppImage", ID).unwrap();
        assert_eq!(
            key(&content, "Exec"),
            ["/opt/L.AppImage %U", "/opt/L.AppImage --dictate"]
        );
        assert_eq!(key(&content, "TryExec"), ["/opt/L.AppImage"]);
        assert_eq!(key(&content, "X-ChatonLM-AppImage"), ["true"]);
        // TryExec et marqueur dans le groupe principal.
        let action = content.find("[Desktop Action dictate]").unwrap();
        assert!(content.find("TryExec=").unwrap() < action);
        assert!(content.find(MARKER).unwrap() < action);
    }

    #[test]
    fn unusable_paths_and_templates_give_nothing() {
        assert_eq!(render(TEMPLATE, "/a\nb.AppImage", ID), None);
        assert_eq!(render(TEMPLATE, "/a\tb.AppImage", ID), None);
        // Exec seulement dans une action : pas d'entrée principale lançable.
        assert_eq!(
            render(
                "[Desktop Entry]\nName=L\n[Desktop Action a]\nExec=chatonlm\n",
                "/a",
                ID
            ),
            None
        );
    }

    fn appimage(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"").unwrap();
        path
    }

    /// Icône d'une version précédente remplacée, puis retirée : le thème
    /// d'icônes est marqué changé à chaque fois, jamais quand rien ne change.
    #[test]
    fn a_changed_icon_marks_the_icon_theme_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data home");
        let target = Target {
            appimage: appimage(dir.path(), "ChatonLM.AppImage"),
            identifier: ID.to_owned(),
            paths: Paths::new(&data, ID),
        };
        let theme = data.join("icons/hicolor");
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
        let age = |theme: &Path| {
            fs::File::open(theme)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        };
        let modified = |theme: &Path| fs::metadata(theme).unwrap().modified().unwrap();

        fs::create_dir_all(target.paths.icon.parent().unwrap()).unwrap();
        fs::write(&target.paths.icon, b"icon of an older ChatonLM").unwrap();
        age(&theme);
        assert_eq!(target.apply(true).unwrap(), Outcome::Written);
        assert_eq!(fs::read(&target.paths.icon).unwrap(), ICON);
        assert!(modified(&theme) > long_ago);

        // Rien ne change : la date du thème non plus.
        age(&theme);
        assert_eq!(target.apply(true).unwrap(), Outcome::Unchanged);
        assert_eq!(modified(&theme), long_ago);

        // Seul le chemin de l'AppImage change : l'icône reste la même.
        let moved = Target {
            appimage: appimage(dir.path(), "ChatonLM-moved.AppImage"),
            ..target.clone()
        };
        assert_eq!(moved.apply(true).unwrap(), Outcome::Written);
        assert_eq!(modified(&theme), long_ago);

        assert_eq!(moved.apply(false).unwrap(), Outcome::Removed);
        assert!(modified(&theme) > long_ago);
    }

    #[test]
    fn the_entry_is_written_kept_updated_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data home");
        let paths = Paths::new(&data, ID);
        let first = appimage(dir.path(), "ChatonLM's 100% $app.AppImage");

        assert_eq!(install(&paths, &first, ID).unwrap(), Outcome::Written);
        let content = fs::read_to_string(&paths.desktop).unwrap();
        assert_eq!(
            parse_exec(key(&content, "Exec")[0]),
            [first.to_str().unwrap()]
        );
        assert_eq!(fs::read(&paths.icon).unwrap(), ICON);
        // Aucun fichier temporaire laissé.
        let names = |dir: &Path| -> Vec<String> {
            fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(&data.join("applications")), [format!("{ID}.desktop")]);

        // Même contenu : rien n'est réécrit (date gardée).
        let written = fs::metadata(&paths.desktop).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(install(&paths, &first, ID).unwrap(), Outcome::Unchanged);
        assert_eq!(
            fs::metadata(&paths.desktop).unwrap().modified().unwrap(),
            written
        );

        // AppImage déplacée : chemin mis à jour.
        let moved = appimage(dir.path(), "ChatonLM-moved.AppImage");
        assert_eq!(install(&paths, &moved, ID).unwrap(), Outcome::Written);
        let content = fs::read_to_string(&paths.desktop).unwrap();
        assert_eq!(key(&content, "TryExec"), [moved.to_str().unwrap()]);

        // Icône effacée à la main : réécrite.
        fs::remove_file(&paths.icon).unwrap();
        assert_eq!(install(&paths, &moved, ID).unwrap(), Outcome::Written);
        assert!(paths.icon.is_file());

        // Retrait : entrée et icône supprimées, puis rien à faire.
        assert_eq!(remove(&paths).unwrap(), Outcome::Removed);
        assert!(!paths.desktop.exists() && !paths.icon.exists());
        assert_eq!(remove(&paths).unwrap(), Outcome::Unchanged);
    }

    #[test]
    fn a_file_without_the_marker_is_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path(), ID);
        let program = appimage(dir.path(), "ChatonLM.AppImage");
        fs::create_dir_all(paths.desktop.parent().unwrap()).unwrap();
        let foreign = "[Desktop Entry]\nName=Mine\nExec=/usr/bin/chatonlm\n";
        fs::write(&paths.desktop, foreign).unwrap();
        fs::create_dir_all(paths.icon.parent().unwrap()).unwrap();
        fs::write(&paths.icon, b"my icon").unwrap();

        assert_eq!(install(&paths, &program, ID).unwrap(), Outcome::Foreign);
        assert_eq!(remove(&paths).unwrap(), Outcome::Foreign);
        assert_eq!(fs::read_to_string(&paths.desktop).unwrap(), foreign);
        assert_eq!(fs::read(&paths.icon).unwrap(), b"my icon");
        // Marqueur à une autre valeur : pas le nôtre non plus.
        fs::write(
            &paths.desktop,
            format!("{foreign}X-ChatonLM-AppImage=false\n"),
        )
        .unwrap();
        assert_eq!(install(&paths, &program, ID).unwrap(), Outcome::Foreign);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        // Le renommage échoue : un dossier non vide porte ce nom.
        let target = dir.path().join("entry.desktop");
        fs::create_dir_all(target.join("inside")).unwrap();
        assert!(write_atomic(&target, b"content").is_err());
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [OsString::from("entry.desktop")]);
        // Dossier des applications impossible à lire : erreur, rien d'écrit.
        let blocked = tempfile::tempdir().unwrap();
        fs::write(blocked.path().join("applications"), b"").unwrap();
        let paths = Paths::new(blocked.path(), ID);
        let program = appimage(dir.path(), "ChatonLM.AppImage");
        assert!(install(&paths, &program, ID).is_err());
        assert!(!paths.icon.exists());
    }
}
