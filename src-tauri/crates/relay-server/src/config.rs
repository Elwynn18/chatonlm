//! Configuration (TOML, lisible par tous) et secrets (TOML à part, lisible
//! par le relais seul : `LoadCredential=` de systemd). Exemples commentés :
//! `packaging/relay/config.example.toml` et `secrets.example.toml`.

use relay_proto::pow::PowParams;

/// Mémoire d'un hachage de la preuve, côté relais : il en vérifie au plus deux
/// à la fois, dans 64 Mio (voir `server.rs`).
pub const MAX_SERVER_POW_MEMORY_KIB: u32 = 64 * 1024;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Adresse d'écoute : la boucle locale, derrière nginx.
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// Base SQLite : clés inscrites (sans IP) et budget du mois.
    pub database: PathBuf,
    /// Adresse du client lue dans `X-Real-IP` (posée par nginx), seulement
    /// quand la connexion vient de la boucle locale.
    #[serde(default = "yes")]
    pub trust_proxy: bool,
    /// Versions de l'app plus anciennes refusées (`outdated`).
    #[serde(default)]
    pub min_app_version: Option<String>,
    /// Interrupteur : recherche en pause (`paused`).
    #[serde(default)]
    pub paused: bool,
    /// Clés bannies (base64url).
    #[serde(default)]
    pub banned_keys: Vec<String>,
    /// Clés oubliées après ce nombre de jours sans usage.
    #[serde(default = "default_key_retention_days")]
    pub key_retention_days: u64,
    #[serde(default)]
    pub quota: QuotaConfig,
    #[serde(default)]
    pub pow: PowConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub tables: TablesConfig,
}

/// Plafonds d'un compteur par réseau, du plus étroit au plus large : IPv6
/// /56, /48, /32 ; IPv4 /24, /16 (le troisième ne sert pas en IPv4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Tiers(pub [u32; 3]);

impl Tiers {
    fn is_valid(&self) -> bool {
        self.0[0] >= 1 && self.0.windows(2).all(|w| w[0] <= w[1])
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct QuotaConfig {
    /// Recherches par installation et par jour (UTC).
    pub daily_per_key: u32,
    /// Pendant les 24 premières heures d'une clé (égal au précédent : sans
    /// effet).
    pub new_key_daily: u32,
    /// Par réseau, en cascade ([`Tiers`]), en mémoire seulement.
    pub per_ip_daily_searches: Tiers,
    pub per_ip_daily_registrations: Tiers,
    pub per_ip_daily_challenges: Tiers,
    /// Toutes requêtes confondues, par minute.
    pub per_ip_per_minute: Tiers,
    /// Requêtes signées par clé et par minute (état, recherches).
    pub per_key_per_minute: u32,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            daily_per_key: 50,
            new_key_daily: 50,
            // Un /48 vaut un peu plus qu'un abonné, un /32 un opérateur ou un
            // hébergeur : de quoi servir ses clients, pas une ferme de clés.
            per_ip_daily_searches: Tiers([1_000, 3_000, 10_000]),
            per_ip_daily_registrations: Tiers([3, 10, 50]),
            per_ip_daily_challenges: Tiers([30, 100, 500]),
            per_ip_per_minute: Tiers([120, 600, 3_000]),
            per_key_per_minute: 30,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PowConfig {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub bits: u8,
    pub count: u8,
    /// Durée de validité d'un défi.
    pub challenge_ttl_secs: u64,
}

impl Default for PowConfig {
    /// Mesuré le 8 nov. 2026 : 16 ms par hachage à 32 Mio sur un Ryzen 7
    /// 3700X (build optimisé), environ le double sur un portable moyen ;
    /// 4 × 2⁵ = 128 hachages attendus, soit 2 s ici et 4 s environ sur un
    /// portable ; 4 hachages (≈ 65 ms) pour vérifier.
    fn default() -> Self {
        Self {
            memory_kib: 32 * 1024,
            iterations: 1,
            parallelism: 1,
            bits: 5,
            count: 4,
            challenge_ttl_secs: 600,
        }
    }
}

impl PowConfig {
    pub fn params(&self) -> PowParams {
        PowParams {
            memory_kib: self.memory_kib,
            iterations: self.iterations,
            parallelism: self.parallelism,
            bits: self.bits,
            count: self.count,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BudgetConfig {
    /// Plafond mensuel global, en euros (pris sur les dons).
    pub monthly_limit_eur: f64,
    /// Avertissement dans le journal à cette part du plafond.
    pub alert_ratio: f64,
    /// Enveloppe du jour : plafond mensuel × cette part ÷ jours du mois. Un
    /// jour ne dépense pas plus de 3 jours moyens : une vague d'abus coûte une
    /// journée de recherche, pas le reste du mois. Au-delà, `budget_exhausted`
    /// jusqu'à minuit UTC. 1 au moins ; le nombre de jours du mois (31) ou
    /// plus : sans effet.
    pub daily_share: f64,
    /// Part de l'enveloppe du jour qu'un réseau peut dépenser, en cascade
    /// comme les quotas par réseau : IPv6 [/56, /48, /32], IPv4 [/24, /16, —].
    /// Une douzaine de clés d'un même réseau ne coupent plus la recherche de
    /// tout le monde jusqu'à minuit (au-delà : `rate_limited` pour ce réseau
    /// seulement). Entre 0 et 1, jamais décroissant ; 1 : sans effet.
    pub network_share: Shares,
}

/// Parts d'un plafond par réseau, du plus étroit au plus large (comme
/// [`Tiers`]).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct Shares(pub [f64; 3]);

impl Shares {
    fn is_valid(&self) -> bool {
        self.0
            .iter()
            .all(|share| share.is_finite() && *share > 0.0 && *share <= 1.0)
            && self.0.windows(2).all(|w| w[0] <= w[1])
    }

    /// Plafonds de chaque niveau pour une enveloppe de `envelope`.
    pub fn of(&self, envelope: u64) -> [u64; 3] {
        self.0.map(|share| (envelope as f64 * share).floor() as u64)
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            monthly_limit_eur: 30.0,
            alert_ratio: 0.8,
            daily_share: 3.0,
            // 3 € par jour (défauts) : 0,30 € par /56 ou /24 (60 recherches
            // Brave, plus que le quota d'une installation), 0,60 € par /48 ou
            // /16, 0,75 € par /32. Vider l'enveloppe demande au moins quatre
            // /32 IPv6 ou cinq /16 IPv4 distincts.
            network_share: Shares([0.1, 0.2, 0.25]),
        }
    }
}

/// Entrées au plus de chaque table en mémoire (voir `limits.rs` : 7/8 d'une
/// puissance de deux par génération, et toutes ensemble sous 64 Mio). Les
/// défauts sont aussi les maximums permis : la mémoire du relais reste sous
/// le `MemoryMax=` de son unité quoi qu'on règle.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TablesConfig {
    /// Compteurs par minute des réseaux (deux générations).
    pub network_minute: usize,
    /// Compteurs du jour des réseaux, recherches et inscriptions.
    pub network_daily: usize,
    /// Défis du jour par réseau (une génération ; pleine, refuse les défis).
    pub challenges: usize,
    /// Quotas du jour et cadence par clé (chacun deux générations).
    pub keys: usize,
    /// Nonces retenus contre le rejeu.
    pub replay: usize,
    /// Clés inscrites gardées en mémoire (deux générations ; les autres sont
    /// relues dans la base au besoin).
    pub registered_keys: usize,
}

impl Default for TablesConfig {
    fn default() -> Self {
        Self {
            network_minute: 229_376,
            network_daily: 458_752,
            challenges: 114_688,
            keys: 229_376,
            replay: 114_688,
            registered_keys: 57_344,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProvidersConfig {
    /// Ordre d'essai (le suivant sert de secours).
    pub order: Vec<String>,
    pub brave: ProviderConfig,
    pub mojeek: ProviderConfig,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            order: vec!["brave".into(), "mojeek".into()],
            brave: ProviderConfig {
                base_url: "https://api.search.brave.com".into(),
                // 5 $ les 1 000 (officiel, nov. 2026), compté large en euros.
                cost_per_1000_eur: 5.0,
                ..ProviderConfig::default()
            },
            mojeek: ProviderConfig {
                base_url: "https://api.mojeek.com".into(),
                // 2 £ les 1 000 (offre Startup), compté large.
                cost_per_1000_eur: 2.5,
                ..ProviderConfig::default()
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProviderConfig {
    pub base_url: String,
    pub cost_per_1000_eur: f64,
    pub timeout_secs: u64,
    /// Brave : extraits supplémentaires (jusqu'à 5 par résultat), plus de
    /// matière pour le modèle ; à essayer au banc R0 (offre et prix).
    pub extra_snippets: bool,
    /// Après un échec, fournisseur évité pendant ce délai.
    pub cooldown_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            cost_per_1000_eur: 5.0,
            timeout_secs: 8,
            extra_snippets: false,
            cooldown_secs: 30,
        }
    }
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8790))
}

fn default_key_retention_days() -> u64 {
    90
}

fn yes() -> bool {
    true
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let config: Config =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Valeurs cohérentes, sinon le relais ne démarre pas.
    pub fn validate(&self) -> Result<(), String> {
        // Mêmes bornes que l'app (coût total compris : un relais mal réglé
        // ferait sinon calculer chaque app pendant des heures).
        if !self.pow.params().is_valid() {
            return Err("pow: settings out of bounds (cost included)".into());
        }
        if self.pow.memory_kib > MAX_SERVER_POW_MEMORY_KIB {
            return Err(format!(
                "pow.memory_kib: {MAX_SERVER_POW_MEMORY_KIB} at most (memory of the relay)"
            ));
        }
        for (name, tiers) in [
            ("per_ip_daily_searches", self.quota.per_ip_daily_searches),
            (
                "per_ip_daily_registrations",
                self.quota.per_ip_daily_registrations,
            ),
            (
                "per_ip_daily_challenges",
                self.quota.per_ip_daily_challenges,
            ),
            ("per_ip_per_minute", self.quota.per_ip_per_minute),
        ] {
            if !tiers.is_valid() {
                return Err(format!(
                    "quota.{name}: three limits, at least 1, never decreasing"
                ));
            }
        }
        if !(self.budget.daily_share.is_finite() && self.budget.daily_share >= 1.0) {
            return Err("budget.daily_share: 1 or more".into());
        }
        if !self.budget.network_share.is_valid() {
            return Err(
                "budget.network_share: three shares above 0 and up to 1, never decreasing".into(),
            );
        }
        let most = TablesConfig::default();
        for (name, value, max, min) in [
            (
                "network_minute",
                self.tables.network_minute,
                most.network_minute,
                2,
            ),
            (
                "network_daily",
                self.tables.network_daily,
                most.network_daily,
                2,
            ),
            ("challenges", self.tables.challenges, most.challenges, 1),
            ("keys", self.tables.keys, most.keys, 2),
            ("replay", self.tables.replay, most.replay, 1),
            (
                "registered_keys",
                self.tables.registered_keys,
                most.registered_keys,
                2,
            ),
        ] {
            if !(min..=max).contains(&value) {
                return Err(format!("tables.{name}: between {min} and {max}"));
            }
        }
        if self.pow.challenge_ttl_secs == 0 || self.pow.challenge_ttl_secs > 3_600 {
            return Err("pow.challenge_ttl_secs: between 1 and 3600".into());
        }
        if !(self.budget.monthly_limit_eur.is_finite() && self.budget.monthly_limit_eur >= 0.0) {
            return Err("budget.monthly_limit_eur: a positive number".into());
        }
        for name in &self.providers.order {
            let provider = match name.as_str() {
                "brave" => &self.providers.brave,
                "mojeek" => &self.providers.mojeek,
                other => return Err(format!("providers.order: unknown provider {other:?}")),
            };
            if !(provider.cost_per_1000_eur.is_finite() && provider.cost_per_1000_eur > 0.0) {
                return Err(format!(
                    "providers.{name}.cost_per_1000_eur: a positive number"
                ));
            }
            let url = url::Url::parse(&provider.base_url)
                .map_err(|_| format!("providers.{name}.base_url: not an address"))?;
            if !matches!(url.scheme(), "https" | "http") {
                return Err(format!("providers.{name}.base_url: http(s) only"));
            }
        }
        if let Some(version) = &self.min_app_version
            && relay_proto::version::AppVersion::parse(version).is_none()
        {
            return Err("min_app_version: x.y.z".into());
        }
        for key in &self.banned_keys {
            if relay_proto::sign::PublicKey::from_b64(key).is_none() {
                return Err("banned_keys: base64url public keys".into());
            }
        }
        Ok(())
    }
}

/// Valeur secrète : jamais affichée (`Debug` masqué, pas de `Display`).
#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_empty() {
            "Secret(<empty>)"
        } else {
            "Secret(***)"
        })
    }
}

/// Fichier des secrets.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    #[serde(default)]
    pub brave_api_key: Secret,
    #[serde(default)]
    pub mojeek_api_key: Secret,
    /// Ceramic et Linkup : banc R0 seulement (`chatonlm-relay bench`), le
    /// relais ne les emploie pas.
    #[serde(default)]
    pub ceramic_api_key: Secret,
    #[serde(default)]
    pub linkup_api_key: Secret,
    /// Clé du sceau des défis (64 caractères hexadécimaux). Absente : tirée au
    /// démarrage (les défis en cours deviennent alors invalides à chaque
    /// redémarrage, sans autre effet).
    #[serde(default)]
    pub challenge_key: Secret,
}

impl Secrets {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        // Le message d'erreur de toml cite la ligne fautive : il pourrait
        // contenir une clé. On n'en garde que la position.
        let secrets: Secrets = toml::from_str(&text).map_err(|error| {
            let at = error
                .span()
                .map(|span| format!(" (bytes {}..{})", span.start, span.end))
                .unwrap_or_default();
            format!("{}: invalid secrets file{at}", path.display())
        })?;
        if !secrets.challenge_key.is_empty() && secrets.challenge_key_bytes().is_none() {
            return Err(format!(
                "{}: challenge_key must be 64 hexadecimal characters",
                path.display()
            ));
        }
        Ok(secrets)
    }

    pub fn challenge_key_bytes(&self) -> Option<[u8; 32]> {
        let text = self.challenge_key.expose().trim();
        if text.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(chunk).ok()?;
            out[i] = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_validated_choices() {
        let config: Config = toml::from_str("database = \"/tmp/x.sqlite\"").unwrap();
        config.validate().unwrap();
        assert_eq!(config.quota.daily_per_key, 50);
        assert_eq!(config.budget.monthly_limit_eur, 30.0);
        assert_eq!(config.providers.order, ["brave", "mojeek"]);
        assert!(config.listen.ip().is_loopback());
        assert!(config.pow.params().is_valid());
    }

    #[test]
    fn bad_configs_are_refused() {
        for extra in [
            "providers.order = [\"google\"]",
            "pow.bits = 40",
            // Chaque borne tenue, coût total démesuré (M1).
            "pow.bits = 15\npow.count = 16",
            "pow.memory_kib = 131072",
            "quota.per_ip_daily_registrations = [3, 2, 50]",
            "quota.per_ip_per_minute = [0, 10, 10]",
            "quota.per_ip_daily_searches = 1000",
            "budget.daily_share = 0.5",
            "budget.network_share = [0.1, 0.2, 1.5]",
            "budget.network_share = [0.0, 0.2, 0.25]",
            "budget.network_share = [0.3, 0.2, 0.25]",
            "budget.network_share = 0.2",
            "tables.registered_keys = 1",
            "tables.registered_keys = 10000000",
            "tables.replay = 10000000",
            "tables.keys = 1",
            "budget.monthly_limit_eur = -1.0",
            "min_app_version = \"latest\"",
            "banned_keys = [\"pas une clé\"]",
            "unknown = 1",
        ] {
            let text = format!("database = \"/tmp/x.sqlite\"\n{extra}");
            let parsed: Result<Config, _> = toml::from_str(&text);
            assert!(
                parsed.is_err() || parsed.unwrap().validate().is_err(),
                "{extra}"
            );
        }
    }

    #[test]
    fn secrets_never_show_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.toml");
        std::fs::write(
            &path,
            "brave_api_key = \"BSA-secret-1234\"\nmojeek_api_key = \"MJK-secret-5678\"\n",
        )
        .unwrap();
        let secrets = Secrets::load(&path).unwrap();
        let shown = format!("{secrets:?}");
        assert!(
            !shown.contains("secret-1234") && !shown.contains("secret-5678"),
            "{shown}"
        );
        assert_eq!(secrets.brave_api_key.expose(), "BSA-secret-1234");
        // Fichier illisible : l'erreur ne recopie pas la ligne (donc pas la clé).
        std::fs::write(
            &path,
            "brave_api_key = \"BSA-secret-1234\nmojeek_api_key = 3",
        )
        .unwrap();
        let error = Secrets::load(&path).unwrap_err();
        assert!(!error.contains("secret-1234"), "{error}");
        std::fs::write(&path, "challenge_key = \"abcd\"").unwrap();
        assert!(Secrets::load(&path).is_err());
        std::fs::write(&path, format!("challenge_key = \"{}\"", "0f".repeat(32))).unwrap();
        assert_eq!(
            Secrets::load(&path).unwrap().challenge_key_bytes(),
            Some([0x0f; 32])
        );
    }

    /// Les exemples du déploiement (`packaging/relay/`) restent valables et
    /// gardent les choix validés.
    #[test]
    fn deployment_examples_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packaging/relay");
        let config = Config::load(&dir.join("config.example.toml")).unwrap();
        assert!(config.listen.ip().is_loopback());
        assert_eq!(config.listen.port(), 8790);
        assert_eq!(config.quota.daily_per_key, 50);
        assert_eq!(config.budget.monthly_limit_eur, 30.0);
        assert_eq!(config.budget.daily_share, 3.0);
        assert_eq!(
            config.budget.network_share,
            BudgetConfig::default().network_share
        );
        assert_eq!(config.quota.per_ip_daily_registrations, Tiers([3, 10, 50]));
        assert_eq!(config.pow.params(), PowConfig::default().params());
        assert_eq!(config.tables, TablesConfig::default());
        let secrets = Secrets::load(&dir.join("secrets.example.toml")).unwrap();
        assert!(secrets.brave_api_key.is_empty() && secrets.mojeek_api_key.is_empty());
        let unit = std::fs::read_to_string(dir.join("chatonlm-relay.service")).unwrap();
        for line in [
            "DynamicUser=yes",
            "ProtectSystem=strict",
            "NoNewPrivileges=yes",
            "CapabilityBoundingSet=\n",
            "LoadCredential=secrets:",
        ] {
            assert!(unit.contains(line), "{line}");
        }
        let nginx = std::fs::read_to_string(dir.join("nginx-relay.chatonlm.com.conf")).unwrap();
        assert!(
            nginx.contains("proxy_pass http://127.0.0.1:8790;")
                && nginx.contains("access_log off;")
        );
        // Les refus de nginx (limit_req) arrivent en JSON, comme ceux du relais (M3).
        assert!(nginx.contains("error_page 429 = @rate_limited;"), "{nginx}");
        assert!(
            nginx.contains("default_type application/json;")
                && nginx.contains(r#"return 429 '{"error":{"code":"rate_limited"}}';"#),
            "{nginx}"
        );
    }

    /// Les lignes que le propriétaire du serveur colle (`DEPLOIEMENT.md` § 3)
    /// écrivent un `secrets.toml` que le relais lit, avec les quatre clés
    /// (banc R0 compris) : le format `printf` reste celui des champs.
    #[test]
    fn the_pasted_secrets_lines_produce_a_valid_file() {
        let guide = include_str!("../../../../packaging/relay/DEPLOIEMENT.md");
        let format = guide
            .lines()
            .find_map(|line| line.strip_prefix("printf '"))
            .and_then(|rest| rest.split('\'').next())
            .expect("ligne printf du § 3");
        let values = [
            "BSA-b",
            "mojeek-m",
            "ceramic-c",
            "linkup-l",
            &"ab".repeat(32),
        ];
        let mut text = format.replace("\\n", "\n");
        for value in values {
            text = text.replacen("%s", value, 1);
        }
        assert!(!text.contains("%s"), "{text}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.toml");
        std::fs::write(&path, &text).unwrap();
        let secrets = Secrets::load(&path).unwrap();
        assert_eq!(secrets.brave_api_key.expose(), "BSA-b");
        assert_eq!(secrets.mojeek_api_key.expose(), "mojeek-m");
        assert_eq!(secrets.ceramic_api_key.expose(), "ceramic-c");
        assert_eq!(secrets.linkup_api_key.expose(), "linkup-l");
        assert!(secrets.challenge_key_bytes().is_some());
        // Autant de variables lues que de `%s` (moins la clé tirée sur place).
        let reads = guide.matches("read -rsp '").count();
        assert_eq!(reads + 1, values.len());
    }
}
