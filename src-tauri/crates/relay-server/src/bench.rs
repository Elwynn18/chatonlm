//! Banc R0 : compare les fournisseurs de recherche (Brave, Mojeek, Ceramic,
//! Linkup) sur un jeu fixe de 60 requêtes (20 en français, 20 en espagnol,
//! 20 en anglais ; actualité, pratique, technique, local), lancé SUR le
//! serveur (`chatonlm-relay bench`) : les clés ne quittent jamais
//! `secrets.toml`.
//!
//! Mesures : latence, nombre de résultats, réponses vides, titres et
//! extraits présents, extraits supplémentaires de Brave, domaines distincts
//! dans les 5 premiers, accord entre fournisseurs (domaines communs dans les
//! 5 premiers), part des domaines du pays (`.fr`, `.es`), coût estimé. Le
//! résumé ne garde aucun résultat (conditions de Brave : pas de stockage) ni
//! aucune requête d'utilisateur (le jeu est fixe et public) ; `--show`
//! affiche seulement, pour une notation à la main, les 3 premiers titres et
//! domaines de chaque fournisseur, sans rien écrire.
//!
//! Ceramic et Linkup, d'après leur documentation officielle (relevée le
//! 10 nov. 2026, jamais essayés faute de clé) :
//!
//! - **Ceramic** : `POST https://api.ceramic.ai/search`, `Authorization:
//!   Bearer …`, corps `{"query", "maxResults" (1 à 20)}` (aucun autre champ
//!   permis, ni langue ni pays) ; réponse `result.results[] {title, url,
//!   description}`.
//! - **Linkup** : `POST https://api.linkup.so/v1/search`, `Authorization:
//!   Bearer …`, corps `{"q", "depth": "standard", "outputType":
//!   "searchResults", "maxResults"}` ; réponse `results[] {type ("text" ou
//!   "image"), name, url, content}`. 429 : débit ou crédit épuisé.

use crate::config::{ProviderConfig, ProvidersConfig, Secret, Secrets};
use crate::providers::{ProviderClient, ProviderError, RawResult, send};
use relay_proto::api::{Provider, Search};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Résultats demandés à chaque fournisseur (l'app en demande 5 par défaut,
/// 10 au plus).
pub const MAX_RESULTS: u8 = 10;
/// Écart entre deux requêtes d'un même fournisseur (Mojeek Startup : 5 par
/// seconde ; les offres d'essai sont souvent à 1 par seconde).
pub const PACE: Duration = Duration::from_millis(1_200);
/// Domaines comparés : les 5 premiers (ce que l'app montre par défaut).
const TOP: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BenchProvider {
    Brave,
    Mojeek,
    Ceramic,
    Linkup,
}

impl BenchProvider {
    pub const ALL: [BenchProvider; 4] = [
        BenchProvider::Brave,
        BenchProvider::Mojeek,
        BenchProvider::Ceramic,
        BenchProvider::Linkup,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BenchProvider::Brave => "brave",
            BenchProvider::Mojeek => "mojeek",
            BenchProvider::Ceramic => "ceramic",
            BenchProvider::Linkup => "linkup",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    News,
    Practical,
    Technical,
    Local,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Category::News => "news",
            Category::Practical => "practical",
            Category::Technical => "technical",
            Category::Local => "local",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BenchQuery {
    pub lang: &'static str,
    pub country: &'static str,
    pub category: Category,
    pub text: &'static str,
}

const fn q(
    lang: &'static str,
    country: &'static str,
    category: Category,
    text: &'static str,
) -> BenchQuery {
    BenchQuery {
        lang,
        country,
        category,
        text,
    }
}

use Category::{Local, News, Practical, Technical};

/// Jeu fixe, rangé par tours (catégorie, puis langue) : les `n` premières
/// requêtes couvrent déjà les trois langues et les quatre catégories
/// (`--queries 12` pour un premier essai).
#[rustfmt::skip]
pub const QUERIES: [BenchQuery; 60] = [
    // Tour 1
    q("fr", "FR", News, "résultats élections législatives"),
    q("es", "ES", News, "últimas noticias economía España"),
    q("en", "US", News, "latest news climate summit"),
    q("fr", "FR", Practical, "recette pâte à crêpes sans lait"),
    q("es", "ES", Practical, "cómo quitar manchas de café de la ropa"),
    q("en", "US", Practical, "how to change a bike tire"),
    q("fr", "FR", Technical, "différence entre TCP et UDP"),
    q("es", "ES", Technical, "cómo funciona un transformador eléctrico"),
    q("en", "US", Technical, "rust borrow checker lifetime error explained"),
    q("fr", "FR", Local, "horaires déchetterie Toulouse"),
    q("es", "ES", Local, "mejores tapas en Sevilla centro"),
    q("en", "US", Local, "public libraries open sunday in Chicago"),
    // Tour 2
    q("fr", "FR", News, "grève SNCF prévisions trafic"),
    q("es", "ES", News, "resultados liga española jornada"),
    q("en", "US", News, "stock market today"),
    q("fr", "FR", Practical, "comment déclarer ses impôts en ligne"),
    q("es", "ES", Practical, "requisitos para renovar el DNI"),
    q("en", "US", Practical, "best way to store fresh herbs"),
    q("fr", "FR", Technical, "installer python sous windows 11"),
    q("es", "ES", Technical, "qué es una red neuronal convolucional"),
    q("en", "US", Technical, "postgres vacuum full vs vacuum"),
    q("fr", "FR", Local, "marché de Noël Strasbourg dates"),
    q("es", "ES", Local, "horario museo del Prado"),
    q("en", "US", Local, "farmers market Portland Oregon"),
    // Tour 3
    q("fr", "FR", News, "prix de l'électricité hausse"),
    q("es", "ES", News, "noticias inteligencia artificial regulación europea"),
    q("en", "GB", News, "premier league results this weekend"),
    q("fr", "FR", Practical, "délai de préavis location meublée"),
    q("es", "ES", Practical, "cuánto tiempo hervir un huevo duro"),
    q("en", "GB", Practical, "how to bleed a radiator"),
    q("fr", "FR", Technical, "erreur git detached HEAD"),
    q("es", "ES", Technical, "diferencia entre RAM y almacenamiento SSD"),
    q("en", "US", Technical, "kubernetes pod crashloopbackoff causes"),
    q("fr", "FR", Local, "randonnée facile autour d'Annecy"),
    q("es", "MX", Local, "clima en Ciudad de México esta semana"),
    q("en", "GB", Local, "train times London to Edinburgh"),
    // Tour 4
    q("fr", "FR", News, "réforme des retraites dernières annonces"),
    q("es", "ES", News, "incendios forestales actualidad"),
    q("en", "US", News, "nasa artemis mission update"),
    q("fr", "FR", Practical, "faire un CV étudiant"),
    q("es", "ES", Practical, "cómo plantar tomates en maceta"),
    q("en", "US", Practical, "how long does paint take to dry"),
    q("fr", "FR", Technical, "fonctionnement d'une pompe à chaleur"),
    q("es", "ES", Technical, "configurar router wifi canal 5 GHz"),
    q("en", "US", Technical, "javascript promise all vs allsettled"),
    q("fr", "BE", Local, "musées gratuits Bruxelles premier dimanche"),
    q("es", "AR", Local, "subte Buenos Aires horarios"),
    q("en", "CA", Local, "ski resorts near Vancouver"),
    // Tour 5
    q("fr", "FR", News, "Tour de France parcours"),
    q("es", "ES", News, "precio de la vivienda alquiler"),
    q("en", "US", News, "election polls latest"),
    q("fr", "CA", Practical, "comment déneiger son toit sans danger"),
    q("es", "ES", Practical, "ejercicios para dolor de espalda"),
    q("en", "AU", Practical, "sunscreen spf 30 vs 50"),
    q("fr", "FR", Technical, "qu'est-ce que le protocole HTTPS"),
    q("es", "ES", Technical, "error 404 qué significa"),
    q("en", "US", Technical, "linux check disk usage command"),
    q("fr", "FR", Local, "boulangerie ouverte lundi Lyon"),
    q("es", "ES", Local, "playas de Valencia con bandera azul"),
    q("en", "AU", Local, "best beaches near Sydney"),
];

/// Un fournisseur au banc.
pub struct Target {
    pub provider: BenchProvider,
    /// Brave et Mojeek : le client du relais lui-même (mêmes paramètres que
    /// la production).
    client: Option<ProviderClient>,
    /// Brave : même client sans `extra_snippets`, si l'offre les refuse.
    fallback: Option<ProviderClient>,
    base_url: String,
    key: Secret,
    http: reqwest::Client,
    pub cost_per_1000_eur: f64,
}

/// Valeurs par défaut des fournisseurs du banc seul.
pub struct BenchDefaults {
    pub ceramic_url: String,
    pub linkup_url: String,
    pub timeout: Duration,
}

impl Default for BenchDefaults {
    fn default() -> Self {
        Self {
            ceramic_url: "https://api.ceramic.ai".into(),
            linkup_url: "https://api.linkup.so".into(),
            // Linkup « standard » peut prendre plusieurs secondes.
            timeout: Duration::from_secs(20),
        }
    }
}

/// Coûts estimés (euros les 1 000, comptés large) : 0,05 $ chez Ceramic
/// (direct), 5 € chez Linkup (« standard »). Brave et Mojeek : ceux de la
/// configuration.
const CERAMIC_COST_PER_1000_EUR: f64 = 0.05;
const LINKUP_COST_PER_1000_EUR: f64 = 5.0;

/// Fournisseurs dont la clé est présente (une clé absente : sauté).
pub fn targets(
    providers: &ProvidersConfig,
    secrets: &Secrets,
    defaults: &BenchDefaults,
) -> Result<Vec<Target>, String> {
    let http = reqwest::Client::builder()
        .timeout(defaults.timeout)
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("chatonlm-relay/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "cannot build the HTTP client".to_owned())?;
    let mut out = Vec::new();
    let relay_client = |provider: Provider, config: &ProviderConfig, key: &Secret, extra| {
        let config = ProviderConfig {
            extra_snippets: extra,
            ..config.clone()
        };
        ProviderClient::new(provider, &config, key.clone())
    };
    if !secrets.brave_api_key.is_empty() {
        let key = &secrets.brave_api_key;
        out.push(Target {
            provider: BenchProvider::Brave,
            client: Some(relay_client(Provider::Brave, &providers.brave, key, true)?),
            fallback: Some(relay_client(Provider::Brave, &providers.brave, key, false)?),
            base_url: String::new(),
            key: Secret::default(),
            http: http.clone(),
            cost_per_1000_eur: providers.brave.cost_per_1000_eur,
        });
    }
    if !secrets.mojeek_api_key.is_empty() {
        let key = &secrets.mojeek_api_key;
        out.push(Target {
            provider: BenchProvider::Mojeek,
            client: Some(relay_client(
                Provider::Mojeek,
                &providers.mojeek,
                key,
                false,
            )?),
            fallback: None,
            base_url: String::new(),
            key: Secret::default(),
            http: http.clone(),
            cost_per_1000_eur: providers.mojeek.cost_per_1000_eur,
        });
    }
    for (provider, key, url, cost) in [
        (
            BenchProvider::Ceramic,
            &secrets.ceramic_api_key,
            &defaults.ceramic_url,
            CERAMIC_COST_PER_1000_EUR,
        ),
        (
            BenchProvider::Linkup,
            &secrets.linkup_api_key,
            &defaults.linkup_url,
            LINKUP_COST_PER_1000_EUR,
        ),
    ] {
        if !key.is_empty() {
            out.push(Target {
                provider,
                client: None,
                fallback: None,
                base_url: url.trim_end_matches('/').to_owned(),
                key: key.clone(),
                http: http.clone(),
                cost_per_1000_eur: cost,
            });
        }
    }
    Ok(out)
}

impl Target {
    /// Une requête ; `extra_off` : Brave a refusé `extra_snippets`.
    async fn search(
        &self,
        query: &BenchQuery,
        extra_off: bool,
    ) -> Result<Vec<RawResult>, ProviderError> {
        let search = Search {
            query: query.text.to_owned(),
            max_results: MAX_RESULTS,
            lang: Some(query.lang.to_owned()),
            country: Some(query.country.to_owned()),
        };
        match (self.provider, &self.client, &self.fallback) {
            (BenchProvider::Brave, Some(_), Some(fallback)) if extra_off => {
                fallback.search_raw(&search).await
            }
            (_, Some(client), _) => client.search_raw(&search).await,
            (BenchProvider::Ceramic, None, _) => {
                let request = self
                    .http
                    .post(format!("{}/search", self.base_url))
                    .bearer_auth(self.key.expose())
                    .header("Content-Type", "application/json")
                    .body(json!({ "query": query.text, "maxResults": MAX_RESULTS }).to_string());
                parse_ceramic(&json_of(&send(request).await?)?)
            }
            (BenchProvider::Linkup, None, _) => {
                let request = self
                    .http
                    .post(format!("{}/v1/search", self.base_url))
                    .bearer_auth(self.key.expose())
                    .header("Content-Type", "application/json")
                    .body(
                        json!({
                            "q": query.text,
                            "depth": "standard",
                            "outputType": "searchResults",
                            "maxResults": MAX_RESULTS,
                        })
                        .to_string(),
                    );
                parse_linkup(&json_of(&send(request).await?)?)
            }
            _ => Err(ProviderError::Rejected),
        }
    }
}

fn json_of(body: &[u8]) -> Result<Value, ProviderError> {
    serde_json::from_slice(body).map_err(|_| ProviderError::Malformed)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

pub fn parse_ceramic(value: &Value) -> Result<Vec<RawResult>, ProviderError> {
    let results = value
        .get("result")
        .and_then(|result| result.get("results"))
        .ok_or(ProviderError::Malformed)?
        .as_array()
        .ok_or(ProviderError::Malformed)?;
    Ok(results
        .iter()
        .map(|item| RawResult {
            title: text(&item["title"]),
            url: text(&item["url"]),
            snippets: vec![text(&item["description"])],
        })
        .collect())
}

pub fn parse_linkup(value: &Value) -> Result<Vec<RawResult>, ProviderError> {
    let results = value
        .get("results")
        .ok_or(ProviderError::Malformed)?
        .as_array()
        .ok_or(ProviderError::Malformed)?;
    Ok(results
        .iter()
        // Les images n'ont ni extrait ni place dans la réponse de l'app.
        .filter(|item| item["type"].as_str().unwrap_or("text") == "text")
        .map(|item| RawResult {
            title: text(&item["name"]),
            url: text(&item["url"]),
            snippets: vec![text(&item["content"])],
        })
        .collect())
}

/// Hôte sans `www.`, en minuscules ; `None` pour une adresse illisible.
fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_owned())
}

/// Mesure d'une requête (rien du contenu n'est gardé, sauf les hôtes des
/// 5 premiers pour l'accord entre fournisseurs, oubliés à la fin).
struct Sample {
    query: usize,
    provider: BenchProvider,
    millis: u64,
    outcome: Result<SampleResults, ProviderError>,
}

struct SampleResults {
    count: usize,
    titled: usize,
    with_snippet: usize,
    snippet_chars: usize,
    with_extra: usize,
    hosts: Vec<String>,
    country_hosts: usize,
}

fn measure(raw: &[RawResult], country: &str) -> SampleResults {
    let mut hosts = Vec::new();
    for item in raw {
        if let Some(host) = host(&item.url)
            && !hosts.contains(&host)
            && hosts.len() < TOP
        {
            hosts.push(host);
        }
    }
    let suffix = format!(".{}", country.to_ascii_lowercase());
    SampleResults {
        count: raw.len(),
        titled: raw.iter().filter(|r| !r.title.trim().is_empty()).count(),
        with_snippet: raw
            .iter()
            .filter(|r| r.snippets.first().is_some_and(|s| !s.trim().is_empty()))
            .count(),
        snippet_chars: raw
            .iter()
            .map(|r| r.snippets.first().map_or(0, |s| s.chars().count()))
            .sum(),
        with_extra: raw
            .iter()
            .filter(|r| r.snippets.iter().skip(1).any(|s| !s.trim().is_empty()))
            .count(),
        country_hosts: hosts.iter().filter(|h| h.ends_with(&suffix)).count(),
        hosts,
    }
}

/// Réglages d'un passage.
pub struct Options {
    pub pace: Duration,
    /// Affiche (stderr) les 3 premiers titres et domaines, pour une notation
    /// à la main ; jamais écrit dans le résumé.
    pub show: bool,
}

/// Passe les requêtes à tous les fournisseurs (en parallèle pour une même
/// requête, `pace` entre deux requêtes) et rend le résumé.
pub async fn run(targets: Vec<Target>, queries: &[BenchQuery], options: &Options) -> Report {
    let targets: Vec<Arc<Target>> = targets.into_iter().map(Arc::new).collect();
    let mut samples = Vec::new();
    // Brave : extra_snippets refusé par l'offre (vu à la première requête).
    let extra_off = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut extra_refused = false;
    for (index, query) in queries.iter().enumerate() {
        if index > 0 && !options.pace.is_zero() {
            tokio::time::sleep(options.pace).await;
        }
        let mut tasks = tokio::task::JoinSet::new();
        for target in &targets {
            let target = target.clone();
            let query = *query;
            let extra_off = extra_off.clone();
            tasks.spawn(async move {
                let off = extra_off.load(std::sync::atomic::Ordering::SeqCst);
                let started = Instant::now();
                let mut outcome = target.search(&query, off).await;
                let mut refused = false;
                // Un refus de Brave avec `extra_snippets` : une fois sans, et
                // sans pour la suite si cela passe.
                if target.provider == BenchProvider::Brave
                    && !off
                    && outcome == Err(ProviderError::Rejected)
                {
                    let retry = target.search(&query, true).await;
                    if retry.is_ok() {
                        extra_off.store(true, std::sync::atomic::Ordering::SeqCst);
                        refused = true;
                    }
                    outcome = retry;
                }
                let millis = started.elapsed().as_millis() as u64;
                (target.provider, millis, outcome, refused)
            });
        }
        let mut answers = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(answer) = joined {
                answers.push(answer);
            }
        }
        answers.sort_by_key(|(provider, ..)| *provider);
        if options.show {
            eprintln!(
                "#{:02} [{} {}] {}",
                index + 1,
                query.lang,
                query.category.as_str(),
                query.text
            );
        }
        for (provider, millis, outcome, refused) in answers {
            extra_refused |= refused;
            if options.show {
                show(provider, &outcome);
            }
            samples.push(Sample {
                query: index,
                provider,
                millis,
                outcome: outcome.map(|raw| measure(&raw, query.country)),
            });
        }
    }
    let costs: BTreeMap<BenchProvider, f64> = targets
        .iter()
        .map(|t| (t.provider, t.cost_per_1000_eur))
        .collect();
    summarize(queries, &samples, &costs, extra_refused)
}

fn show(provider: BenchProvider, outcome: &Result<Vec<RawResult>, ProviderError>) {
    match outcome {
        Ok(raw) if raw.is_empty() => eprintln!("  {:<8} (no result)", provider.as_str()),
        Ok(raw) => {
            for item in raw.iter().take(3) {
                let title: String = crate::providers::clean(&item.title)
                    .chars()
                    .take(70)
                    .collect();
                eprintln!(
                    "  {:<8} {title} — {}",
                    provider.as_str(),
                    host(&item.url).unwrap_or_default()
                );
            }
        }
        Err(error) => eprintln!("  {:<8} error: {}", provider.as_str(), error.as_str()),
    }
}

/// Mesures agrégées d'un fournisseur (pour tout le jeu, une langue ou une
/// catégorie).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Aggregate {
    pub sent: usize,
    pub ok: usize,
    pub errors: BTreeMap<&'static str, usize>,
    pub latency_p50_ms: u64,
    pub latency_p90_ms: u64,
    pub latency_max_ms: u64,
    /// Moyenne par réponse réussie.
    pub results_mean: f64,
    /// Réponses réussies sans aucun résultat.
    pub empty: usize,
    /// Part des résultats avec un titre, avec un extrait.
    pub titled_ratio: f64,
    pub snippet_ratio: f64,
    pub snippet_chars_mean: f64,
    /// Part des résultats avec au moins un extrait supplémentaire (Brave).
    pub extra_snippet_ratio: f64,
    /// Domaines distincts dans les 5 premiers (moyenne).
    pub distinct_hosts_top5: f64,
    /// Part des domaines des 5 premiers sous le domaine du pays (`.fr`…),
    /// requêtes hors anglais seulement.
    pub country_host_ratio: Option<f64>,
    /// Requêtes peut-être facturées (réussies, sans réponse, illisibles).
    pub billed: usize,
    pub cost_eur: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderReport {
    pub provider: BenchProvider,
    pub cost_per_1000_eur: f64,
    pub all: Aggregate,
    pub by_lang: BTreeMap<&'static str, Aggregate>,
    pub by_category: BTreeMap<&'static str, Aggregate>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub queries: usize,
    pub langs: BTreeMap<&'static str, usize>,
    pub providers: Vec<ProviderReport>,
    /// Domaines communs dans les 5 premiers, en moyenne sur les requêtes où
    /// les deux ont répondu : `[a, b, moyenne, requêtes]`.
    pub agreement: Vec<(BenchProvider, BenchProvider, f64, usize)>,
    /// Brave a refusé `extra_snippets` (offre) : mesuré sans.
    pub brave_extra_snippets_refused: bool,
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

fn aggregate<'a>(
    samples: impl Iterator<Item = &'a Sample>,
    queries: &[BenchQuery],
    cost_per_1000_eur: f64,
) -> Aggregate {
    let mut out = Aggregate::default();
    let mut latencies = Vec::new();
    let (mut results, mut titled, mut snippets, mut chars, mut extra) = (0, 0, 0, 0, 0);
    let (mut hosts, mut country_hosts, mut country_total) = (0, 0, 0);
    for sample in samples {
        out.sent += 1;
        match &sample.outcome {
            Ok(r) => {
                out.ok += 1;
                out.billed += 1;
                latencies.push(sample.millis);
                results += r.count;
                titled += r.titled;
                snippets += r.with_snippet;
                chars += r.snippet_chars;
                extra += r.with_extra;
                hosts += r.hosts.len();
                if r.count == 0 {
                    out.empty += 1;
                }
                if queries[sample.query].lang != "en" {
                    country_hosts += r.country_hosts;
                    country_total += r.hosts.len();
                }
            }
            Err(error) => {
                if error.billed() {
                    out.billed += 1;
                }
                *out.errors.entry(error.as_str()).or_default() += 1;
            }
        }
    }
    latencies.sort_unstable();
    out.latency_p50_ms = percentile(&latencies, 0.5);
    out.latency_p90_ms = percentile(&latencies, 0.9);
    out.latency_max_ms = latencies.last().copied().unwrap_or(0);
    out.results_mean = ratio(results, out.ok);
    out.titled_ratio = ratio(titled, results);
    out.snippet_ratio = ratio(snippets, results);
    out.snippet_chars_mean = ratio(chars, snippets);
    out.extra_snippet_ratio = ratio(extra, results);
    out.distinct_hosts_top5 = ratio(hosts, out.ok - out.empty);
    out.country_host_ratio = (country_total > 0).then(|| ratio(country_hosts, country_total));
    out.cost_eur = out.billed as f64 * cost_per_1000_eur / 1000.0;
    out
}

fn summarize(
    queries: &[BenchQuery],
    samples: &[Sample],
    costs: &BTreeMap<BenchProvider, f64>,
    brave_extra_snippets_refused: bool,
) -> Report {
    let mut langs = BTreeMap::new();
    for query in queries {
        *langs.entry(query.lang).or_default() += 1;
    }
    let mut providers = Vec::new();
    for (&provider, &cost) in costs {
        let of = |keep: &dyn Fn(&BenchQuery) -> bool| {
            aggregate(
                samples
                    .iter()
                    .filter(|s| s.provider == provider && keep(&queries[s.query])),
                queries,
                cost,
            )
        };
        let by_lang = langs
            .keys()
            .map(|&lang| (lang, of(&|q| q.lang == lang)))
            .collect();
        let by_category = [News, Practical, Technical, Local]
            .into_iter()
            .map(|c| (c.as_str(), of(&|q| q.category == c)))
            .collect();
        providers.push(ProviderReport {
            provider,
            cost_per_1000_eur: cost,
            all: of(&|_| true),
            by_lang,
            by_category,
        });
    }
    let mut agreement = Vec::new();
    let names: Vec<BenchProvider> = costs.keys().copied().collect();
    for (i, &a) in names.iter().enumerate() {
        for &b in &names[i + 1..] {
            let (mut shared, mut count) = (0, 0);
            for query in 0..queries.len() {
                let hosts = |p: BenchProvider| {
                    samples
                        .iter()
                        .find(|s| s.query == query && s.provider == p)
                        .and_then(|s| s.outcome.as_ref().ok())
                        .filter(|r| r.count > 0)
                        .map(|r| r.hosts.iter().cloned().collect::<HashSet<_>>())
                };
                if let (Some(x), Some(y)) = (hosts(a), hosts(b)) {
                    shared += x.intersection(&y).count();
                    count += 1;
                }
            }
            agreement.push((a, b, ratio(shared, count), count));
        }
    }
    Report {
        queries: queries.len(),
        langs,
        providers,
        agreement,
        brave_extra_snippets_refused,
    }
}

fn pct(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

impl Report {
    /// Résumé lisible (aucune requête, aucun résultat).
    pub fn text(&self) -> String {
        let mut out = String::new();
        let langs: Vec<String> = self.langs.iter().map(|(l, n)| format!("{l} {n}")).collect();
        let _ = writeln!(
            out,
            "ChatonLM relay bench: {} queries ({}), {MAX_RESULTS} results asked",
            self.queries,
            langs.join(", ")
        );
        let _ = writeln!(
            out,
            "\nprovider  ok/sent  p50ms  p90ms  maxms  results  empty  titles  snippets  chars  extra  hosts@5  ccTLD  cost EUR  EUR/1000"
        );
        for p in &self.providers {
            let a = &p.all;
            let _ = writeln!(
                out,
                "{:<8}  {:>3}/{:<3}  {:>5}  {:>5}  {:>5}  {:>7.1}  {:>5}  {:>6}  {:>8}  {:>5.0}  {:>5}  {:>7.1}  {:>5}  {:>8.3}  {:>8.2}",
                p.provider.as_str(),
                a.ok,
                a.sent,
                a.latency_p50_ms,
                a.latency_p90_ms,
                a.latency_max_ms,
                a.results_mean,
                a.empty,
                pct(a.titled_ratio),
                pct(a.snippet_ratio),
                a.snippet_chars_mean,
                pct(a.extra_snippet_ratio),
                a.distinct_hosts_top5,
                a.country_host_ratio.map_or("-".into(), pct),
                a.cost_eur,
                p.cost_per_1000_eur,
            );
        }
        let errors: Vec<String> = self
            .providers
            .iter()
            .filter(|p| !p.all.errors.is_empty())
            .map(|p| {
                let list: Vec<String> = p
                    .all
                    .errors
                    .iter()
                    .map(|(k, n)| format!("{k} ×{n}"))
                    .collect();
                format!("{}: {}", p.provider.as_str(), list.join(", "))
            })
            .collect();
        if !errors.is_empty() {
            let _ = writeln!(out, "\nerrors: {}", errors.join("; "));
        }
        let groups =
            |title: &str, pick: &dyn Fn(&ProviderReport) -> &BTreeMap<&'static str, Aggregate>| {
                let mut block = format!("\n{title} (results mean / empty / p50 ms / errors)\n");
                for p in &self.providers {
                    let cells: Vec<String> = pick(p)
                        .iter()
                        .map(|(k, a)| {
                            format!(
                                "{k} {:.1}/{}/{}/{}",
                                a.results_mean,
                                a.empty,
                                a.latency_p50_ms,
                                a.sent - a.ok
                            )
                        })
                        .collect();
                    let _ = writeln!(block, "{:<8}  {}", p.provider.as_str(), cells.join("  "));
                }
                block
            };
        out.push_str(&groups("by language", &|p| &p.by_lang));
        out.push_str(&groups("by category", &|p| &p.by_category));
        if !self.agreement.is_empty() {
            let _ = writeln!(out, "\nagreement (shared hosts in the top 5, mean)");
            for (a, b, mean, count) in &self.agreement {
                let _ = writeln!(
                    out,
                    "{:<8} ~ {:<8} {mean:.1} over {count} queries",
                    a.as_str(),
                    b.as_str()
                );
            }
        }
        if self.brave_extra_snippets_refused {
            let _ = writeln!(
                out,
                "\nbrave: extra_snippets refused by the plan, measured without"
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_set_is_balanced_and_interleaved() {
        let mut count = BTreeMap::new();
        for query in &QUERIES {
            *count.entry((query.lang, query.category)).or_insert(0) += 1;
            assert!(!query.text.trim().is_empty());
            assert!(query.text.split_whitespace().count() <= 50, "Ceramic");
            assert_eq!(query.country.len(), 2);
        }
        assert_eq!(count.len(), 12);
        assert!(count.values().all(|&n| n == 5), "{count:?}");
        // Les 12 premières couvrent toutes les langues et catégories.
        let first: HashSet<_> = QUERIES[..12].iter().map(|q| (q.lang, q.category)).collect();
        assert_eq!(first.len(), 12);
        let unique: HashSet<_> = QUERIES.iter().map(|q| q.text).collect();
        assert_eq!(unique.len(), 60);
    }

    #[test]
    fn ceramic_and_linkup_replies_are_read() {
        let ceramic = json!({"requestId": "x", "result": {"results": [
            {"title": "T", "url": "https://a.example/", "description": "D"}
        ], "searchMetadata": {"executionTime": 0.1}, "totalResults": 1}});
        assert_eq!(
            parse_ceramic(&ceramic).unwrap(),
            [RawResult {
                title: "T".into(),
                url: "https://a.example/".into(),
                snippets: vec!["D".into()],
            }]
        );
        assert_eq!(parse_ceramic(&json!({})), Err(ProviderError::Malformed));
        let linkup = json!({"results": [
            {"type": "image", "name": "I", "url": "https://i.example/x.png"},
            {"type": "text", "name": "N", "url": "https://b.example/", "content": "C", "favicon": ""}
        ]});
        let raw = parse_linkup(&linkup).unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(
            (raw[0].title.as_str(), raw[0].snippets[0].as_str()),
            ("N", "C")
        );
        assert_eq!(
            parse_linkup(&json!({"results": 1})),
            Err(ProviderError::Malformed)
        );
    }

    #[test]
    fn measures_and_percentiles() {
        let raw = vec![
            RawResult {
                title: "a".into(),
                url: "https://www.Exemple.fr/1".into(),
                snippets: vec!["abc".into(), "extra".into()],
            },
            RawResult {
                title: " ".into(),
                url: "https://exemple.fr/2".into(),
                snippets: vec![String::new()],
            },
            RawResult {
                title: "c".into(),
                url: "https://other.com/".into(),
                snippets: vec!["de".into()],
            },
        ];
        let m = measure(&raw, "FR");
        assert_eq!(
            (m.count, m.titled, m.with_snippet, m.with_extra),
            (3, 2, 2, 1)
        );
        assert_eq!(m.snippet_chars, 5);
        assert_eq!(m.hosts, ["exemple.fr", "other.com"]);
        assert_eq!(m.country_hosts, 1);
        assert_eq!(percentile(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 0.5), 5);
        assert_eq!(percentile(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 0.9), 9);
        assert_eq!(percentile(&[], 0.9), 0);
    }

    /// Le parcours de la bibliothèque : `--show` n'est qu'un affichage, la
    /// réponse ne garde aucun titre ni aucune adresse.
    #[test]
    fn the_report_keeps_no_result_and_no_query() {
        let queries = &QUERIES[..2];
        let ok = |query, hosts: &[&str]| Sample {
            query,
            provider: BenchProvider::Mojeek,
            millis: 100,
            outcome: Ok(SampleResults {
                count: hosts.len(),
                titled: hosts.len(),
                with_snippet: hosts.len(),
                snippet_chars: 10,
                with_extra: 0,
                hosts: hosts.iter().map(|h| h.to_string()).collect(),
                country_hosts: 0,
            }),
        };
        let samples = vec![
            ok(0, &["secret-host.example"]),
            Sample {
                query: 1,
                provider: BenchProvider::Mojeek,
                millis: 9_000,
                outcome: Err(ProviderError::Lost),
            },
        ];
        let costs = BTreeMap::from([(BenchProvider::Mojeek, 2.5)]);
        let report = summarize(queries, &samples, &costs, false);
        let all = &report.providers[0].all;
        assert_eq!((all.sent, all.ok, all.billed), (2, 1, 2));
        assert_eq!(all.errors.get("no reply"), Some(&1));
        assert!((all.cost_eur - 0.005).abs() < 1e-9);
        let text = report.text();
        let json = serde_json::to_string(&report).unwrap();
        for out in [&text, &json] {
            assert!(!out.contains("secret-host"), "{out}");
            assert!(!queries.iter().any(|q| out.contains(q.text)), "{out}");
        }
    }
}
