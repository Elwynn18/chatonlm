//! Détection du serveur Ollama, pour l'écran d'accueil.
//!
//! La sonde HTTP fait foi : si l'adresse répond comme Ollama, tout va bien.
//! Sinon, l'écran doit dire pourquoi et quoi faire :
//! - à une adresse locale où rien n'écoute, le binaire et le service systemd
//!   choisissent le conseil (installer, démarrer le service, lancer
//!   `ollama serve`, lire les journaux) ;
//! - si quelque chose écoute mais que l'échange échoue, le détail est montré ;
//! - une adresse qui répond, elle, comme Ollama est proposée en remplacement
//!   (la même en http, l'origine sans chemin, 127.0.0.1:11434).

use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;
use url::Url;

use super::client::{self, Probe};
use super::host::{self, Service, Unit};

/// Port par défaut d'Ollama.
const STANDARD_PORT: u16 = 11434;

/// Adresse locale standard, essayée quand l'adresse réglée ne répond pas.
const STANDARD_LOCAL_URL: &str = "http://127.0.0.1:11434";

/// Délai maximal de la lecture des faits de la machine (D-Bus compris).
const HOST_FACTS_TIMEOUT: Duration = Duration::from_secs(3);

/// Délai d'une connexion TCP de contrôle (« quelque chose écoute-t-il ? »).
const LISTEN_CHECK_TIMEOUT: Duration = Duration::from_millis(500);

/// Durée pendant laquelle un résultat sert aux vérifications qui attendaient
/// la même adresse (relance automatique et clic sur Retry simultanés).
const SHARED_RESULT_AGE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ServerState {
    /// Ollama répond.
    Running,
    /// L'adresse répond en HTTP, mais pas comme Ollama.
    NotOllama,
    /// L'adresse refuse l'accès (HTTP 401, 403 ou 407).
    AccessDenied,
    /// Adresse distante sans réponse utilisable.
    RemoteUnreachable,
    /// Quelque chose écoute à cette adresse locale, mais l'échange échoue
    /// (https sur le port HTTP d'Ollama, connexion coupée...).
    ConnectionFailed,
    /// Ni binaire ni service sur cette machine.
    NotInstalled,
    /// Service systemd présent mais arrêté.
    ServiceStopped,
    /// Service systemd en échec, ou qui redémarre en boucle après des
    /// plantages.
    ServiceFailed,
    /// Service systemd en cours de démarrage.
    ServiceStarting,
    /// Ollama installé, mais rien n'écoute à l'adresse réglée : `ollama serve`.
    NoService,
    /// Pas de réponse : délai dépassé, service actif qui n'écoute pas à cette
    /// adresse, ou proxy local dont la cible ne répond pas (HTTP 502 à 504).
    NotResponding,
    /// L'Ollama intégré démarre (ou redémarre après un plantage).
    BuiltinStarting,
    /// L'Ollama intégré a échoué (`detail`, journal `log_path`) ; relancé
    /// seulement par `restart_builtin_ollama`.
    BuiltinFailed,
}

/// Service systemd d'Ollama.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServiceInfo {
    /// Service de l'utilisateur (`systemctl --user`), pas du système.
    pub user: bool,
    /// Faux seulement si l'unité est désactivée : `systemctl enable` est alors
    /// proposé.
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerStatus {
    /// Adresse vérifiée.
    pub url: String,
    pub state: ServerState,
    /// Version d'Ollama, quand il répond.
    pub version: Option<String>,
    /// Adresse qui répond comme Ollama, quand l'adresse réglée ne répond pas.
    pub alternative_url: Option<String>,
    /// Valeur de `OLLAMA_HOST` pour lancer le serveur à l'adresse réglée, quand
    /// `ollama serve` ne l'écouterait pas sans réglage mais le peut (http, sans
    /// chemin, port non standard et non privilégié).
    pub host_env: Option<String>,
    /// Code HTTP reçu, quand la réponse n'est pas celle d'Ollama.
    pub http_status: Option<u16>,
    /// Détail technique d'un échec de connexion.
    pub detail: Option<String>,
    /// Service systemd d'Ollama sur cette machine, pour une adresse locale.
    pub service: Option<ServiceInfo>,
    /// L'état décrit l'Ollama intégré (`url` : son adresse).
    pub builtin: bool,
    /// L'Ollama intégré peut servir (binaire livré) : « Use the built-in
    /// Ollama » peut être proposé.
    pub builtin_available: bool,
    /// Adresse réglée dans Settings ou par `OLLAMA_HOST` : jamais remplacée
    /// d'office par l'intégré (bouton « Use the built-in Ollama »).
    pub explicit_address: bool,
    /// Journal de l'Ollama intégré, quand l'état le décrit.
    pub log_path: Option<String>,
}

/// Faits sur la machine, quand l'adresse est locale et ne répond pas.
#[derive(Clone, Debug)]
pub struct HostFacts {
    pub binary_found: bool,
    pub unit: Unit,
}

/// Ce que la vérification a observé en plus de la sonde.
#[derive(Clone, Debug, Default)]
pub struct Observations {
    pub facts: Option<HostFacts>,
    /// Quelque chose accepte les connexions TCP à l'adresse (mesuré après un
    /// échec local autre qu'un refus).
    pub listening: bool,
    pub alternative_url: Option<String>,
}

/// Vérifie `url`. Les vérifications simultanées de la même adresse partagent
/// un résultat (la seconde attend la première).
pub async fn check(url: &str) -> ServerStatus {
    static LAST: tokio::sync::Mutex<Option<(Instant, ServerStatus)>> =
        tokio::sync::Mutex::const_new(None);
    let mut last = LAST.lock().await;
    if let Some((at, status)) = last.as_ref()
        && status.url == url
        && at.elapsed() < SHARED_RESULT_AGE
    {
        return status.clone();
    }
    let status = check_now(url, &standard_local_url()).await;
    *last = Some((Instant::now(), status.clone()));
    status
}

async fn check_now(url: &str, standard_url: &str) -> ServerStatus {
    let parsed = Url::parse(url).ok();
    let probe = client::probe(url).await;
    if matches!(probe, Probe::Ollama { .. }) {
        return decide(url, parsed.as_ref(), probe, Observations::default());
    }

    let local = parsed.as_ref().is_some_and(client::is_local);
    // Table des écoutes du système (Windows) ; `None` sous Linux, où la
    // sonde suffit.
    let listener = match parsed.as_ref().filter(|_| local) {
        Some(parsed) => local_listener(parsed).await,
        None => None,
    };
    let probe = settle(probe, local, listener);
    let (facts, listening, alternative_url) = tokio::join!(
        async {
            if local {
                Some(host_facts().await)
            } else {
                None
            }
        },
        async {
            match &parsed {
                Some(parsed) if local && matches!(probe, Probe::Failed(_)) => match listener {
                    Some(listening) => listening,
                    None => accepts_connections(parsed).await,
                },
                _ => false,
            }
        },
        find_alternative(parsed.as_ref(), &probe, standard_url),
    );
    let seen = Observations {
        facts,
        listening,
        alternative_url,
    };
    decide(url, parsed.as_ref(), probe, seen)
}

/// Quelque chose écoute-t-il sur le port de cette adresse locale, d'après
/// le système ? `None` : pas de réponse sûre (toujours sous Linux).
async fn local_listener(url: &Url) -> Option<bool> {
    let port = url.port_or_known_default()?;
    tauri::async_runtime::spawn_blocking(move || host::local_listener(port))
        .await
        .ok()
        .flatten()
}

/// Sonde corrigée par ce que dit le système. Sous Windows, une connexion
/// vers un port fermé de la boucle locale met jusqu'à 2 s à échouer (la
/// pile réessaie après le refus) : la sonde rend alors un délai dépassé, ou
/// un échec, là où Linux refuse aussitôt. Quand le système affirme que rien
/// n'écoute sur le port d'une adresse locale, c'est un refus : sans cela,
/// « rien d'installé » ne serait jamais reconnu et l'Ollama intégré ne
/// démarrerait pas seul. `listening` inconnu (Linux) : sonde inchangée.
pub(crate) fn settle(probe: Probe, local: bool, listening: Option<bool>) -> Probe {
    match (&probe, local, listening) {
        (Probe::Timeout | Probe::Failed(_), true, Some(false)) => Probe::Refused,
        _ => probe,
    }
}

/// Décision, sans entrée-sortie : testable pour chaque combinaison.
pub fn decide(url: &str, parsed: Option<&Url>, probe: Probe, seen: Observations) -> ServerStatus {
    let mut status = ServerStatus {
        url: url.to_owned(),
        state: ServerState::Running,
        version: None,
        alternative_url: None,
        host_env: None,
        http_status: None,
        detail: None,
        service: None,
        builtin: false,
        builtin_available: false,
        explicit_address: false,
        log_path: None,
    };
    let local = parsed.is_some_and(client::is_local);

    status.state = match (probe, local) {
        (Probe::Ollama { version }, _) => {
            status.version = Some(version);
            return status;
        }
        (Probe::NotOllama { status: code }, _) => {
            status.http_status = (code != 200).then_some(code);
            match code {
                401 | 403 | 407 => ServerState::AccessDenied,
                502..=504 if local => ServerState::NotResponding,
                502..=504 => ServerState::RemoteUnreachable,
                _ => ServerState::NotOllama,
            }
        }
        (Probe::Timeout | Probe::Refused, false) => ServerState::RemoteUnreachable,
        (Probe::Failed(detail), false) => {
            status.detail = Some(detail);
            ServerState::RemoteUnreachable
        }
        (Probe::Timeout, true) => ServerState::NotResponding,
        (Probe::Failed(detail), true) if seen.listening => {
            status.detail = Some(detail);
            ServerState::ConnectionFailed
        }
        // Adresse locale où rien n'écoute : l'installation choisit le conseil.
        (Probe::Refused | Probe::Failed(_), true) => {
            let state = nothing_listening(parsed, seen.facts.as_ref());
            if state == ServerState::NoService {
                status.host_env = parsed.and_then(serve_host);
            }
            state
        }
    };

    status.service = seen.facts.as_ref().and_then(|facts| match &facts.unit {
        Unit::Loaded(service) => Some(ServiceInfo {
            user: service.user,
            enabled: service.unit_file_state != "disabled",
        }),
        _ => None,
    });
    status.alternative_url = seen.alternative_url;
    status
}

fn nothing_listening(parsed: Option<&Url>, facts: Option<&HostFacts>) -> ServerState {
    let unknown = HostFacts {
        binary_found: false,
        unit: Unit::Unknown,
    };
    let facts = facts.unwrap_or(&unknown);
    // Le service écoute sur 11434 sans chemin : ailleurs, il ne sert à rien.
    let custom_address = parsed
        .is_none_or(|url| url.port_or_known_default() != Some(STANDARD_PORT) || url.path() != "/");
    match (&facts.unit, facts.binary_found) {
        (Unit::NotFound | Unit::Unknown, false) => ServerState::NotInstalled,
        _ if custom_address => ServerState::NoService,
        (Unit::Loaded(service), _) => service_state(service),
        (_, true) => ServerState::NoService,
    }
}

fn service_state(service: &Service) -> ServerState {
    match (service.active_state.as_str(), service.sub_state.as_str()) {
        ("failed", _) => ServerState::ServiceFailed,
        // Restart=always (unité du script officiel) : après un plantage, le
        // service redémarre sans fin et ne passe jamais par « failed ».
        ("activating", "auto-restart" | "auto-restart-queued") => ServerState::ServiceFailed,
        ("activating" | "reloading", _) => ServerState::ServiceStarting,
        ("active", _) => ServerState::NotResponding,
        _ => ServerState::ServiceStopped,
    }
}

/// `hôte:port` à donner à `OLLAMA_HOST` pour que `ollama serve` écoute à cette
/// adresse : seulement en http, sans chemin, hors du port standard (inutile)
/// et des ports privilégiés (refusés sans droits).
pub(crate) fn serve_host(url: &Url) -> Option<String> {
    let port = url.port_or_known_default()?;
    let host = url.host_str()?;
    let eligible =
        url.scheme() == "http" && url.path() == "/" && port >= 1024 && port != STANDARD_PORT;
    eligible.then(|| format!("{host}:{port}"))
}

async fn host_facts() -> HostFacts {
    #[cfg(feature = "e2e")]
    if let Some(facts) = injected_facts() {
        return facts;
    }
    let gather = tauri::async_runtime::spawn_blocking(|| HostFacts {
        binary_found: host::find_binary().is_some(),
        unit: host::ollama_unit(),
    });
    match tokio::time::timeout(HOST_FACTS_TIMEOUT, gather).await {
        Ok(Ok(facts)) => facts,
        // Bus figé ou tâche perdue : le service reste inconnu.
        _ => HostFacts {
            binary_found: host::find_binary().is_some(),
            unit: Unit::Unknown,
        },
    }
}

/// Quelque chose accepte-t-il les connexions TCP à cette adresse ?
async fn accepts_connections(url: &Url) -> bool {
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let Ok(addresses) = tokio::net::lookup_host((host, port)).await else {
        return false;
    };
    for address in addresses {
        let connect = tokio::net::TcpStream::connect(address);
        if let Ok(Ok(_)) = tokio::time::timeout(LISTEN_CHECK_TIMEOUT, connect).await {
            return true;
        }
    }
    false
}

/// Adresse de remplacement qui répond comme Ollama, par ordre de préférence :
/// la même en http (Ollama ne fait pas de https), l'origine sans chemin
/// (adresse copiée avec un chemin qui donne 404), puis l'adresse locale
/// standard. Les candidates sont sondées en parallèle.
async fn find_alternative(url: Option<&Url>, probe: &Probe, standard_url: &str) -> Option<String> {
    let url = url?;
    let mut candidates = Vec::new();
    if url.scheme() == "https" && client::is_local(url) {
        let mut http = url.clone();
        if http.set_scheme("http").is_ok() {
            candidates.push(base(&http));
        }
    }
    if matches!(probe, Probe::NotOllama { status: 404 }) && url.path() != "/" {
        let mut origin = url.clone();
        origin.set_path("");
        candidates.push(base(&origin));
    }
    if !is_standard_local(url) {
        candidates.push(standard_url.to_owned());
    }
    let configured = base(url);
    candidates.retain(|candidate| *candidate != configured);
    candidates.dedup();

    let (first, second, third) = tokio::join!(
        answers(candidates.first()),
        answers(candidates.get(1)),
        answers(candidates.get(2)),
    );
    first.or(second).or(third)
}

async fn answers(candidate: Option<&String>) -> Option<String> {
    let candidate = candidate?;
    matches!(client::probe(candidate).await, Probe::Ollama { .. }).then(|| candidate.clone())
}

fn base(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_owned()
}

/// Équivalent de http://127.0.0.1:11434 (localhost, ::1).
fn is_standard_local(url: &Url) -> bool {
    url.scheme() == "http"
        && url.port_or_known_default() == Some(STANDARD_PORT)
        && url.path() == "/"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

fn standard_local_url() -> String {
    // Les tests de l'app réelle ne dépendent pas d'un Ollama de la machine.
    #[cfg(feature = "e2e")]
    if let Ok(url) = std::env::var("CHATONLM_E2E_STANDARD_URL") {
        return url;
    }
    STANDARD_LOCAL_URL.to_owned()
}

/// Faits imposés par les tests de l'app réelle (fonctionnalité `e2e`), par
/// exemple `CHATONLM_E2E_HOST_FACTS="binary=yes;unit=system:inactive:dead:disabled"`.
#[cfg(feature = "e2e")]
fn injected_facts() -> Option<HostFacts> {
    let value = std::env::var("CHATONLM_E2E_HOST_FACTS").ok()?;
    let facts = parse_facts(&value);
    if facts.is_none() {
        eprintln!("chatonlm: CHATONLM_E2E_HOST_FACTS invalide, ignoré : {value}");
    }
    facts
}

/// `binary=yes|no;unit=none|unknown|<system|user>:<ActiveState>:<SubState>:<UnitFileState>`.
#[cfg(any(test, feature = "e2e"))]
fn parse_facts(value: &str) -> Option<HostFacts> {
    let mut facts = HostFacts {
        binary_found: false,
        unit: Unit::NotFound,
    };
    for part in value.split(';') {
        match part.split_once('=')? {
            ("binary", "yes") => facts.binary_found = true,
            ("binary", "no") => facts.binary_found = false,
            ("unit", "none") => facts.unit = Unit::NotFound,
            ("unit", "unknown") => facts.unit = Unit::Unknown,
            ("unit", unit) => {
                let fields: Vec<&str> = unit.split(':').collect();
                let [scope, active, sub, file] = fields.as_slice() else {
                    return None;
                };
                let user = match *scope {
                    "system" => false,
                    "user" => true,
                    _ => return None,
                };
                facts.unit = Unit::Loaded(Service {
                    user,
                    active_state: (*active).to_owned(),
                    sub_state: (*sub).to_owned(),
                    unit_file_state: (*file).to_owned(),
                });
            }
            _ => return None,
        }
    }
    Some(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake_ollama::{FakeOllama, Mode};

    const LOCAL: &str = "http://localhost:11434";
    /// Port fermé : sert d'adresse standard dans les tests, pour ne pas
    /// dépendre d'un Ollama de la machine.
    const NO_STANDARD: &str = "http://127.0.0.1:1";

    fn facts(binary_found: bool, unit: Unit) -> HostFacts {
        HostFacts { binary_found, unit }
    }

    fn service(active: &str, sub: &str, file: &str) -> Unit {
        Unit::Loaded(Service {
            user: false,
            active_state: active.into(),
            sub_state: sub.into(),
            unit_file_state: file.into(),
        })
    }

    fn loaded(active: &str) -> Unit {
        service(active, "dead", "enabled")
    }

    fn status_for(url: &str, probe: Probe, seen: Observations) -> ServerStatus {
        let parsed = Url::parse(url).unwrap();
        decide(url, Some(&parsed), probe, seen)
    }

    fn state(url: &str, probe: Probe, facts: Option<HostFacts>) -> ServerState {
        let seen = Observations {
            facts,
            ..Observations::default()
        };
        status_for(url, probe, seen).state
    }

    #[test]
    fn answering_servers() {
        let running = status_for(
            LOCAL,
            Probe::Ollama {
                version: "0.35.1".into(),
            },
            Observations::default(),
        );
        assert_eq!(running.state, ServerState::Running);
        assert_eq!(running.version.as_deref(), Some("0.35.1"));
        let other = status_for(
            LOCAL,
            Probe::NotOllama { status: 200 },
            Observations::default(),
        );
        assert_eq!(other.state, ServerState::NotOllama);
        assert_eq!(other.http_status, None);
    }

    #[test]
    fn http_errors_are_told_apart() {
        let cases = [
            (LOCAL, 401, ServerState::AccessDenied),
            (LOCAL, 403, ServerState::AccessDenied),
            ("https://example.com", 407, ServerState::AccessDenied),
            (LOCAL, 502, ServerState::NotResponding),
            ("https://example.com", 503, ServerState::RemoteUnreachable),
            (LOCAL, 404, ServerState::NotOllama),
            (LOCAL, 500, ServerState::NotOllama),
        ];
        for (url, code, expected) in cases {
            let status = status_for(
                url,
                Probe::NotOllama { status: code },
                Observations::default(),
            );
            assert_eq!(status.state, expected, "{url} {code}");
            assert_eq!(status.http_status, Some(code));
        }
    }

    #[test]
    fn a_closed_local_port_is_a_refusal_when_the_system_says_so() {
        // Windows : délai dépassé ou échec, alors que rien n'écoute.
        for probe in [Probe::Timeout, Probe::Failed("os error 10060".into())] {
            assert_eq!(settle(probe, true, Some(false)), Probe::Refused);
        }
        // Quelque chose écoute : la sonde dit vrai.
        assert_eq!(settle(Probe::Timeout, true, Some(true)), Probe::Timeout);
        // Système muet (Linux), adresse distante, autres réponses : inchangé.
        assert_eq!(settle(Probe::Timeout, true, None), Probe::Timeout);
        assert_eq!(settle(Probe::Timeout, false, Some(false)), Probe::Timeout);
        assert_eq!(settle(Probe::Refused, true, Some(false)), Probe::Refused);
        let ollama = Probe::Ollama {
            version: "0.40.0".into(),
        };
        assert_eq!(settle(ollama.clone(), true, Some(false)), ollama);
        assert_eq!(
            settle(Probe::NotOllama { status: 404 }, true, Some(false)),
            Probe::NotOllama { status: 404 }
        );
        // Conséquence : « rien d'installé » est reconnu malgré le délai.
        let seen = Observations {
            facts: Some(facts(false, Unit::NotFound)),
            ..Observations::default()
        };
        let status = status_for(LOCAL, settle(Probe::Timeout, true, Some(false)), seen);
        assert_eq!(status.state, ServerState::NotInstalled);
    }

    #[test]
    fn remote_address_without_answer() {
        for probe in [Probe::Refused, Probe::Timeout, Probe::Failed("dns".into())] {
            assert_eq!(
                state("http://192.168.1.2:11434", probe, None),
                ServerState::RemoteUnreachable
            );
        }
        let status = status_for(
            "http://192.168.1.2:11434",
            Probe::Failed("connection failed: dns error".into()),
            Observations::default(),
        );
        assert_eq!(
            status.detail.as_deref(),
            Some("connection failed: dns error")
        );
    }

    #[test]
    fn local_installation_states() {
        let cases = [
            (facts(false, Unit::NotFound), ServerState::NotInstalled),
            (facts(false, Unit::Unknown), ServerState::NotInstalled),
            (facts(true, loaded("inactive")), ServerState::ServiceStopped),
            (
                facts(false, loaded("inactive")),
                ServerState::ServiceStopped,
            ),
            (
                facts(true, loaded("deactivating")),
                ServerState::ServiceStopped,
            ),
            (facts(true, loaded("failed")), ServerState::ServiceFailed),
            (
                facts(true, service("activating", "auto-restart", "enabled")),
                ServerState::ServiceFailed,
            ),
            (
                facts(true, service("activating", "start", "enabled")),
                ServerState::ServiceStarting,
            ),
            (facts(true, loaded("active")), ServerState::NotResponding),
            (facts(true, Unit::NotFound), ServerState::NoService),
            (facts(true, Unit::Unknown), ServerState::NoService),
        ];
        for (facts, expected) in cases {
            assert_eq!(
                state(LOCAL, Probe::Refused, Some(facts.clone())),
                expected,
                "{facts:?}"
            );
        }
    }

    #[test]
    fn service_details_drive_the_commands() {
        let seen = |unit| Observations {
            facts: Some(facts(true, unit)),
            ..Observations::default()
        };
        let disabled = status_for(
            LOCAL,
            Probe::Refused,
            seen(service("inactive", "dead", "disabled")),
        );
        assert_eq!(
            disabled.service,
            Some(ServiceInfo {
                user: false,
                enabled: false
            })
        );
        let user = Unit::Loaded(Service {
            user: true,
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            unit_file_state: "enabled".into(),
        });
        let user = status_for(LOCAL, Probe::Refused, seen(user));
        assert_eq!(user.state, ServerState::ServiceStopped);
        assert_eq!(
            user.service,
            Some(ServiceInfo {
                user: true,
                enabled: true
            })
        );
        // Délai dépassé : journalctl n'a de sens qu'avec un service.
        let timeout = status_for(LOCAL, Probe::Timeout, seen(loaded("active")));
        assert_eq!(timeout.state, ServerState::NotResponding);
        assert!(timeout.service.is_some());
    }

    #[test]
    fn something_listening_but_failing_is_not_a_missing_server() {
        let seen = Observations {
            facts: Some(facts(false, Unit::NotFound)),
            listening: true,
            alternative_url: None,
        };
        let status = status_for("https://localhost:11434", Probe::Failed("tls".into()), seen);
        assert_eq!(status.state, ServerState::ConnectionFailed);
        assert_eq!(status.detail.as_deref(), Some("tls"));
        // Rien n'écoute : conseils d'installation, sans détail.
        let quiet = state(
            "https://localhost:11434",
            Probe::Failed("tls".into()),
            Some(facts(false, Unit::NotFound)),
        );
        assert_eq!(quiet, ServerState::NotInstalled);
    }

    #[test]
    fn custom_address_asks_for_ollama_serve_with_ollama_host() {
        let url = "http://127.0.0.1:11435";
        let seen = Observations {
            facts: Some(facts(true, loaded("inactive"))),
            listening: false,
            alternative_url: Some(STANDARD_LOCAL_URL.into()),
        };
        let status = status_for(url, Probe::Refused, seen);
        assert_eq!(status.state, ServerState::NoService);
        assert_eq!(status.host_env.as_deref(), Some("127.0.0.1:11435"));
        assert_eq!(status.alternative_url.as_deref(), Some(STANDARD_LOCAL_URL));
        // Service sans binaire trouvé : le service écoute ailleurs.
        assert_eq!(
            state(url, Probe::Refused, Some(facts(false, loaded("inactive")))),
            ServerState::NoService
        );
        // Sans rien d'installé, Ollama reste à installer.
        assert_eq!(
            state(url, Probe::Refused, Some(facts(false, Unit::NotFound))),
            ServerState::NotInstalled
        );
    }

    #[test]
    fn ollama_host_only_where_ollama_serve_can_listen() {
        let host_env = |url: &str| {
            let seen = Observations {
                facts: Some(facts(true, Unit::NotFound)),
                ..Observations::default()
            };
            status_for(url, Probe::Refused, seen).host_env
        };
        assert_eq!(
            host_env("http://localhost:8080").as_deref(),
            Some("localhost:8080")
        );
        assert_eq!(host_env(LOCAL), None);
        assert_eq!(host_env("http://localhost"), None);
        assert_eq!(host_env("https://localhost:8443"), None);
        assert_eq!(host_env("http://localhost:8080/ollama"), None);
    }

    #[test]
    fn facts_for_the_real_app_tests() {
        let parsed = parse_facts("binary=yes;unit=user:activating:auto-restart:disabled").unwrap();
        assert!(parsed.binary_found);
        assert_eq!(
            parsed.unit,
            Unit::Loaded(Service {
                user: true,
                active_state: "activating".into(),
                sub_state: "auto-restart".into(),
                unit_file_state: "disabled".into(),
            })
        );
        assert!(matches!(
            parse_facts("binary=no;unit=none").unwrap().unit,
            Unit::NotFound
        ));
        assert!(parse_facts("binary=maybe").is_none());
        assert!(parse_facts("unit=system:inactive").is_none());
    }

    #[tokio::test]
    async fn check_against_a_fake_server() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let status = check(&server.url()).await;
        assert_eq!(status.state, ServerState::Running);
        assert_eq!(status.url, server.url());
    }

    #[tokio::test]
    async fn https_to_ollama_suggests_http() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let https = server.url().replace("http://", "https://");
        let status = check_now(&https, NO_STANDARD).await;
        assert_eq!(status.state, ServerState::ConnectionFailed);
        assert!(status.detail.is_some());
        assert_eq!(status.alternative_url, Some(server.url()));
    }

    #[tokio::test]
    async fn closed_connections_are_reported_as_such() {
        let server = FakeOllama::start(Mode::Close).await.unwrap();
        let status = check_now(&server.url(), NO_STANDARD).await;
        assert_eq!(status.state, ServerState::ConnectionFailed);
        assert_eq!(status.alternative_url, None);
    }

    #[tokio::test]
    async fn a_path_that_gives_404_suggests_the_origin() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let status = check_now(&format!("{}/ollama", server.url()), NO_STANDARD).await;
        assert_eq!(status.state, ServerState::NotOllama);
        assert_eq!(status.http_status, Some(404));
        assert_eq!(status.alternative_url, Some(server.url()));
    }

    #[tokio::test]
    async fn the_standard_address_is_suggested_for_a_silent_remote() {
        let server = FakeOllama::start(Mode::ollama()).await.unwrap();
        let status = check_now("http://192.0.2.1:11434", &server.url()).await;
        assert_eq!(status.state, ServerState::RemoteUnreachable);
        assert_eq!(status.alternative_url, Some(server.url()));
    }
}
