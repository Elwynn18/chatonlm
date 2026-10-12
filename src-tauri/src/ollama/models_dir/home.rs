//! Dossier `.ollama` du service système d'Ollama, que `ProtectHome=` lui
//! cache (Settings > General, bouton « Fix »).
//!
//! `ollama serve` signe ses requêtes à ollama.com (modèles du registre,
//! compte : `/api/me`) avec sa clé `$HOME/.ollama/id_ed25519`. L'unité
//! d'Arch a `ProtectHome=yes` : avec un `HOME` sous `/home` (`User=` et
//! `HOME=` d'un `override.conf`), la clé est illisible (« open
//! /home/<user>/.ollama/id_ed25519: permission denied » à chaque
//! téléchargement du registre, 503 « account unavailable » pour le compte) ;
//! avec `ProtectHome=tmpfs` sans `BindPaths=` du dossier, `ollama serve` ne
//! démarre même plus (il ne peut pas créer sa clé). D'où :
//! - une détection pure ([`hidden_home`]), à partir des réglages du service lus
//!   par D-Bus ([`ServiceDetails`](super::host::ServiceDetails)) ;
//! - une correction par le script embarqué `home.sh`, lancé en root par
//!   `pkexec --disable-internal-agent /bin/sh -c <script> chatonlm-home
//!   ollama.service <dossier>` (une invite, rien d'installé, arguments jamais
//!   interpolés) : drop-in `zz-chatonlm-home.conf` (`ProtectHome=tmpfs` +
//!   `BindPaths=` de ce seul dossier, créé avec les droits du service s'il
//!   manque), daemon-reload, redémarrage, vérification dans l'espace de noms
//!   du service, restauration en cas d'échec ; sans pkexec ni agent
//!   d'authentification, les mêmes commandes à copier.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};

// Types lus par la page : communs aux deux systèmes (`models_dir.rs`).
pub use super::{HiddenHome, HomeFixFailure, HomeFixOutcome};
use super::{
    MAX_DETAIL, ModelsLocation, UNIT, clean_path, read_limited, shell_quote, shell_word,
    unit_escape,
};

/// Script exécuté en root par pkexec, passé tel quel dans argv.
pub const SCRIPT: &str = include_str!("home.sh");

/// Drop-in écrit par le script : après `override.conf` (`systemctl edit`) et
/// les `zz-home.conf` écrits à la main, avant `zz-chatonlm-models.conf`.
pub const DROPIN_NAME: &str = "zz-chatonlm-home.conf";

/// Dossiers que `ProtectHome=` masque.
const HIDDEN_ROOTS: [&str; 3] = ["/home", "/root", "/run/user"];

/// Dossier masqué par `ProtectHome=` qui contient `path` (`/home`, `/root`
/// ou `/run/user`), comme `in_home` des scripts.
pub fn hidden_root(path: &str) -> Option<&'static str> {
    hidden_root_in(path, None)
}

/// [`hidden_root`], avec `test_home` traité comme `/home` (essais seulement :
/// le `CHATONLM_TEST_HOME` des scripts, que lit la détection imposée).
fn hidden_root_in(path: &str, test_home: Option<&str>) -> Option<&'static str> {
    let under = |root: &str| {
        path.strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    HIDDEN_ROOTS
        .iter()
        .copied()
        .find(|root| under(root))
        .or_else(|| {
            test_home
                .map(|home| home.trim_end_matches('/'))
                .filter(|home| !home.is_empty() && under(home))
                .map(|_| "/home")
        })
}

/// Dossier `.ollama` du service : `$HOME/.ollama`, avec le `HOME` que
/// systemd lui donne (`Environment=`, sinon répertoire de `User=`) ; `None`
/// sans `HOME` (service root sans `User=` : systemd ne pose pas `HOME`).
/// Comme `service_ollama_dir` des scripts.
pub fn service_ollama_dir(
    env_home: Option<&str>,
    user: &str,
    user_home: Option<&str>,
) -> Option<String> {
    let home = env_home
        .filter(|home| !home.is_empty())
        .or_else(|| user_home.filter(|_| !user.is_empty()))?;
    let home = home.strip_suffix('/').unwrap_or(home);
    (!home.is_empty()).then(|| format!("{home}/.ollama"))
}

/// Le service ne voit pas `dir` : `ProtectHome=` yes (true) ou tmpfs, `dir`
/// sous `/home`, `/root` ou `/run/user`, et, avec tmpfs, aucun `BindPaths=`
/// ni `BindReadOnlyPaths=` (`binds` : leurs destinations) sur `dir` ou un
/// parent (avec yes, systemd ne peut rien monter dessous). `read-only` :
/// lisible, rien à signaler. `test_home` : voir [`hidden_root_in`].
fn is_hidden_in(dir: &str, protect_home: &str, binds: &[String], test_home: Option<&str>) -> bool {
    if hidden_root_in(dir, test_home).is_none() {
        return false;
    }
    match protect_home {
        "yes" | "true" => true,
        "tmpfs" => !binds.iter().any(|bind| covers(bind, dir)),
        _ => false,
    }
}

/// Destinations des montages `binds` (source, destination) dont la source
/// existe : un montage dont la source a disparu (`.ollama` effacé après la
/// correction) ne rend rien, et systemd refuse alors de démarrer le service
/// (226/NAMESPACE).
pub fn visible_binds(binds: &[(String, String)], exists: impl Fn(&str) -> bool) -> Vec<String> {
    binds
        .iter()
        .filter(|(source, _)| exists(source))
        .map(|(_, destination)| destination.clone())
        .collect()
}

/// La source d'un montage existe (liens suivis, comme systemd) ; illisible
/// (dossier d'un autre compte) : supposée présente.
pub fn source_exists(path: &str) -> bool {
    !matches!(std::fs::metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

/// `bind` est `dir` ou l'un de ses parents.
fn covers(bind: &str, dir: &str) -> bool {
    let bind = bind.trim_end_matches('/');
    dir == bind
        || dir
            .strip_prefix(bind)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Dossier `.ollama` caché du service système auquel l'app parle ; `None`
/// pour tout autre lancement (service de l'utilisateur, à la main,
/// conteneur, autre machine), ou un dossier visible.
pub fn hidden_home(
    location: &ModelsLocation,
    ollama_dir: Option<&str>,
    protect_home: &str,
    binds: &[String],
) -> Option<HiddenHome> {
    hidden_home_in(location, ollama_dir, protect_home, binds, None)
}

/// [`hidden_home`] ; `test_home` : voir [`hidden_root_in`].
pub(super) fn hidden_home_in(
    location: &ModelsLocation,
    ollama_dir: Option<&str>,
    protect_home: &str,
    binds: &[String],
    test_home: Option<&str>,
) -> Option<HiddenHome> {
    let ModelsLocation::SystemService {
        user,
        running,
        can_elevate,
        ..
    } = location
    else {
        return None;
    };
    // Un HOME que systemd interpréterait (" \ : $ `...) : ni drop-in ni
    // commandes possibles.
    let dir = ollama_dir.filter(|dir| clean_path(dir))?;
    if !is_hidden_in(dir, protect_home, binds, test_home) {
        return None;
    }
    Some(HiddenHome {
        dir: dir.to_owned(),
        hidden_root: hidden_root_in(dir, test_home)?.to_owned(),
        user: user.clone(),
        running: *running,
        can_elevate: *can_elevate,
        commands: (user != "root").then(|| sudo_commands(dir, user, *running)),
    })
}

/// Drop-in écrit par le script, octet pour octet.
pub fn dropin_text(dir: &str) -> String {
    format!(
        "# Written by ChatonLM (Settings > General): Ollama's own folder.\n[Service]\nProtectHome=tmpfs\nBindPaths=\"{}\"\n",
        unit_escape(dir)
    )
}

/// Ce que fait le script, à lancer avec sudo : dossier créé par le service,
/// même drop-in, rechargement et redémarrage.
pub fn sudo_commands(dir: &str, user: &str, running: bool) -> String {
    let lines: Vec<String> = dropin_text(dir).lines().map(shell_quote).collect();
    let reload = if running {
        "sudo systemctl daemon-reload && sudo systemctl restart ollama.service"
    } else {
        "sudo systemctl daemon-reload"
    };
    format!(
        "sudo -u {user} mkdir -p {dir}\n\
         sudo mkdir -p /etc/systemd/system/ollama.service.d\n\
         printf '%s\\n' {lines} | sudo tee /etc/systemd/system/ollama.service.d/{DROPIN_NAME} > /dev/null\n\
         {reload}",
        user = shell_word(user),
        dir = shell_quote(dir),
        lines = lines.join(" "),
    )
}

// --- Exécution ------------------------------------------------------------------

/// Ligne `result changed=… restarted=…` : (changed, restarted).
pub fn parse_result(line: &str) -> Option<(bool, bool)> {
    let rest = line.strip_prefix("result ")?;
    let (mut changed, mut restarted) = (None, None);
    for pair in rest.split(' ') {
        match pair.split_once('=')? {
            ("changed", value) => changed = Some(value == "yes"),
            ("restarted", value) => restarted = Some(value == "yes"),
            _ => {}
        }
    }
    Some((changed?, restarted?))
}

/// Fin d'après le code de sortie de pkexec (celui du script, ou 126/127 de
/// pkexec), sa sortie d'erreur et la ligne `result`.
pub fn outcome(code: Option<i32>, detail: &str, result: Option<(bool, bool)>) -> HomeFixOutcome {
    let failed = |reason, restored| HomeFixOutcome::Failed {
        reason,
        restored,
        detail: detail.to_owned(),
    };
    match code {
        Some(0) => match result {
            Some((changed, restarted)) => HomeFixOutcome::Done { changed, restarted },
            None => failed(HomeFixFailure::Unexpected, false),
        },
        Some(126) => HomeFixOutcome::Cancelled,
        Some(127) if detail.contains("No authentication agent") => {
            failed(HomeFixFailure::NoAgent, false)
        }
        Some(127) => failed(HomeFixFailure::NotAuthorized, false),
        Some(11) => failed(HomeFixFailure::UnitChanged, false),
        Some(12) => failed(HomeFixFailure::NotYours, false),
        Some(15) => failed(HomeFixFailure::CannotCreate, false),
        Some(22) => failed(HomeFixFailure::ConfigFailed, true),
        Some(23) => failed(HomeFixFailure::Overridden, true),
        Some(24) => failed(HomeFixFailure::StartFailed, true),
        Some(30) => failed(HomeFixFailure::RestoreFailed, false),
        _ => failed(HomeFixFailure::Unexpected, false),
    }
}

/// Correction à exécuter.
#[derive(Clone, Debug)]
pub struct FixRequest {
    pub pkexec: PathBuf,
    pub shell: PathBuf,
    /// `$HOME/.ollama` du service (le script le recalcule et le compare).
    pub dir: String,
    /// Variables ajoutées à l'environnement (essais).
    pub envs: Vec<(OsString, OsString)>,
}

/// Lance le script par pkexec. `on_restart` est appelé quand le script va
/// redémarrer le service (les réponses en cours s'arrêtent alors
/// proprement). Le processus n'est jamais tué : le script root doit aller au
/// bout.
pub async fn fix(request: FixRequest, on_restart: impl FnMut()) -> HomeFixOutcome {
    fix_with(request, on_restart, |_| {}).await
}

/// [`fix`], avec chaque ligne de la sortie standard remise à `on_line`
/// (essais).
pub(super) async fn fix_with(
    request: FixRequest,
    mut on_restart: impl FnMut(),
    mut on_line: impl FnMut(&str),
) -> HomeFixOutcome {
    let unexpected = |detail: String| HomeFixOutcome::Failed {
        reason: HomeFixFailure::Unexpected,
        restored: false,
        detail,
    };
    let mut command = tokio::process::Command::new(&request.pkexec);
    command
        .arg("--disable-internal-agent")
        .arg(&request.shell)
        .arg("-c")
        .arg(SCRIPT)
        .arg("chatonlm-home")
        .arg(UNIT)
        // En dernier : la fin de la ligne affichée par la boîte polkit.
        .arg(&request.dir)
        .envs(request.envs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Variables du banc d'essai : jamais transmises en production (le vrai
    // pkexec les retirerait de toute façon).
    #[cfg(not(any(debug_assertions, feature = "e2e")))]
    for name in [
        "CHATONLM_TEST_ROOT",
        "CHATONLM_TEST_BIN",
        "CHATONLM_TEST_HOME",
    ] {
        command.env_remove(name);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return unexpected(format!(
                "could not start {}: {error}",
                request.pkexec.display()
            ));
        }
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return unexpected("no output from pkexec".into());
    };
    let mut result = None;
    let follow = async {
        // Lignes lues en octets : un chemin non UTF-8 n'arrête pas la lecture.
        let mut lines = BufReader::new(stdout).split(b'\n');
        while let Ok(Some(line)) = lines.next_segment().await {
            let text = String::from_utf8_lossy(&line);
            let text = text.strip_suffix('\r').unwrap_or(&text);
            on_line(text);
            if text == "step restart" {
                on_restart();
            } else if let Some(parsed) = parse_result(text) {
                result = Some(parsed);
            }
        }
    };
    let ((), detail) = tokio::join!(follow, read_limited(stderr, MAX_DETAIL));
    let code = child.wait().await.ok().and_then(|status| status.code());
    outcome(code, &detail, result)
}

#[cfg(test)]
mod tests;
