//! Chef de la carte graphique (propre à ChatonLM, lot 3 du lien VS Code) :
//! un seul décideur pour le modèle de la conversation et celui de
//! l'autocomplétion de l'éditeur. Contrat : `.tools/research/91-lot3-gpu-
//! completion-contract.md` ; faits d'Ollama : rapport 87-c § 2.
//!
//! - L'autocomplétion ne charge JAMAIS un modèle d'elle-même : une requête
//!   n'est transmise que si le modèle est déjà dans `/api/ps` (sinon Ollama
//!   attendrait la fin de la réponse en cours, puis chasserait le modèle de
//!   la conversation). Seul « Préparer » charge, et seulement si les deux
//!   tiennent ensemble ou si la conversation n'est pas en mémoire.
//! - Options figées par modèle : `num_ctx` 4 096 et `keep_alive` toujours
//!   plus court que celui de la conversation ([`keep_alive_beside`] : Ollama
//!   l'évince en premier) ; même modèle que la conversation : les options de
//!   la conversation. Une proposition n'est envoyée que si le modèle est en
//!   mémoire AVEC ces options (sinon Ollama le rechargerait).
//! - Décision par constat (`/api/ps`), estimation seulement pour prévoir
//!   ([`plan`]) ; vaut pour l'Ollama de la machine comme pour l'intégré.
//! - Pendant qu'une réponse de conversation s'écrit, ou qu'un autre travail
//!   de l'app tient Ollama ([`activity`] : Correction, Étude, polissage,
//!   chargement…), l'autocomplétion attend (calcul partagé) ; « Préparer »
//!   s'arrête aussi.
//! - Avant une image (sd-server, qui prend la VRAM libre), le modèle
//!   d'autocomplétion est déchargé ([`GpuChief::before_image`]).

pub mod activity;
pub mod fim;
pub mod plan;
pub mod suggest;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::{Notify, watch};

use crate::chat::Server;
use crate::hub::fit;
use crate::hub::hardware::Machine;
use crate::i18n::t;
use crate::ollama::models::{self, ModelInfo};
use crate::ollama::running::{self, RunningModel, same_model};
use plan::{GpuPlan, Loaded, Memory, Reason, Step};

/// Contexte de l'autocomplétion (même valeur à chaque requête).
pub const COMPLETION_CONTEXT: u32 = 4096;
/// Durée en mémoire du modèle d'autocomplétion, au plus (voir
/// [`keep_alive_beside`]).
pub const COMPLETION_KEEP_ALIVE: i64 = 600;
/// Défaut d'Ollama sans `keep_alive` ni `OLLAMA_KEEP_ALIVE` : 5 min.
const SERVER_KEEP_ALIVE: i64 = 300;
/// Un modèle qui quitte la mémoire dans moins que cela ne sert plus : la
/// proposition arriverait après son départ et le rechargerait.
const EXPIRY_MARGIN: Duration = Duration::from_secs(3);
/// Relecture de « quelque chose d'autre tient Ollama » pendant « Préparer ».
const BUSY_CHECK: Duration = Duration::from_millis(50);
/// Liste des modèles installés gardée.
const TAGS_KEEP: Duration = Duration::from_secs(10);
/// Délai de `/api/ps`.
const PS_TIMEOUT: Duration = Duration::from_secs(2);
/// Requêtes de la mesure (après une de chauffe).
const MEASURES: usize = 3;
/// Jetons générés par une mesure.
const MEASURED_TOKENS: u32 = 32;

/// Ce que le chef lit de l'app au moment d'agir.
pub struct Inputs {
    pub server: Server,
    /// Modèle de la conversation (réglage).
    pub chat_model: String,
    /// Serveur sur cette machine.
    pub local: bool,
    /// Une réponse de conversation s'écrit.
    pub busy: Box<dyn Fn() -> bool + Send + Sync>,
    /// Machine du serveur (`None` : distant ou illisible).
    pub machine: Option<Machine>,
}

/// Refus d'une demande de l'éditeur.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// L'autocomplétion attend (raison donnée à l'éditeur).
    Paused(Reason),
    /// Texte traduit.
    Refused(String),
}

/// Durée en mémoire du modèle d'autocomplétion à côté de la conversation
/// (secondes) : toujours plus courte que celle de la conversation, pour
/// qu'Ollama l'évince en premier (il choisit parmi les modèles inactifs
/// celui dont la durée est la plus courte) : 80 % de la sienne, 10 min au
/// plus. `chat` : réglage de la conversation (0 : rien d'envoyé, donc
/// `server_default`) ; négatif : gardée chargée.
pub fn keep_alive_beside(chat: i64, server_default: i64) -> i64 {
    let chat = if chat == 0 { server_default } else { chat };
    if chat < 0 {
        return COMPLETION_KEEP_ALIVE;
    }
    (chat * 4 / 5).clamp(1, COMPLETION_KEEP_ALIVE)
}

/// Durée d'Ollama sans `keep_alive` (secondes) : `OLLAMA_KEEP_ALIVE` (que
/// l'Ollama intégré hérite de l'app ; pour celui de la machine, meilleure
/// indication connue), sinon 5 min. Forme d'Ollama : secondes entières, ou
/// durée de Go (« 10m », « 1h30m », « -1s »).
pub fn server_keep_alive(env: Option<&str>) -> i64 {
    env.and_then(parse_keep_alive).unwrap_or(SERVER_KEEP_ALIVE)
}

fn parse_keep_alive(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<i64>() {
        return Some(seconds);
    }
    let (negative, mut rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number: f64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let factor = match &rest[..unit] {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 1e-3,
            "us" | "µs" => 1e-6,
            "ns" => 1e-9,
            _ => return None,
        };
        rest = &rest[unit..];
        total += number * factor;
    }
    let seconds = total.ceil() as i64;
    Some(if negative { -seconds } else { seconds })
}

/// Résultat de `completion.measure`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measure {
    pub rating: fim::Rating,
    pub first_token_ms: u64,
    pub tokens_per_second: f64,
    pub partial_gpu: bool,
}

/// État du chef (géré par Tauri, partagé par le pont et le peintre).
pub struct GpuChief {
    /// Modèle d'autocomplétion de l'éditeur (dernier préparé ou demandé).
    completion: Mutex<Option<String>>,
    /// (conversation, autocomplétion) constatés ne tenant pas ensemble.
    exclusive: Mutex<Option<(String, String)>>,
    /// Modèles installés : (lue à, adresse, liste).
    tags: tokio::sync::Mutex<Option<(Instant, String, Vec<ModelInfo>)>>,
    /// Ce que `/api/show` dit d'un modèle, par (adresse, modèle).
    shown: Mutex<HashMap<(String, String), Shown>>,
    /// Taille constatée dans `/api/ps`, par (adresse, modèle) : estimation
    /// d'un modèle que `/api/tags` ne connaît pas.
    seen: Mutex<HashMap<(String, String), u64>>,
    /// Dernier plan, pour les éditeurs qui écoutent.
    plan: watch::Sender<Option<GpuPlan>>,
    /// Quelque chose a changé (chargement, déchargement) : replanifier.
    poke: Notify,
    /// Dernier serveur et modèle de conversation vus.
    last: Mutex<Option<(String, String)>>,
    /// Le sondage de `/api/ps` tourne.
    pub polling: AtomicBool,
    /// Machine gardée par le sondage : (lue à, machine).
    machine: tokio::sync::Mutex<Option<(Instant, Option<Machine>)>>,
    /// Dernière proposition de téléchargement montrée (`models.offerPull`).
    offered: Mutex<Option<Instant>>,
}

impl Default for GpuChief {
    fn default() -> Self {
        Self {
            completion: Mutex::new(None),
            exclusive: Mutex::new(None),
            tags: tokio::sync::Mutex::new(None),
            shown: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            plan: watch::channel(None).0,
            poke: Notify::new(),
            last: Mutex::new(None),
            polling: AtomicBool::new(false),
            machine: tokio::sync::Mutex::new(None),
            offered: Mutex::new(None),
        }
    }
}

/// Ce que `/api/show` dit d'un modèle (gardé).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    /// Capacité `insert` (gabarit FIM).
    insert: bool,
    /// Tourne ailleurs (ollama.com).
    remote: bool,
    /// Contexte maximal du modèle.
    max_context: Option<u32>,
}

/// Mémoire du plan : VRAM s'il y a une carte, sinon RAM.
pub fn memory_of(machine: Option<&Machine>) -> Option<Memory> {
    let machine = machine?;
    if machine.gpus.is_empty() {
        return Some(Memory {
            total: machine.ram_total,
            free: Some(machine.ram_available),
            gpu: false,
        });
    }
    let total = machine.gpus.iter().map(|gpu| gpu.vram_total).sum();
    let free = machine
        .gpus
        .iter()
        .map(|gpu| gpu.vram_free)
        .sum::<Option<u64>>();
    Some(Memory {
        total,
        free,
        gpu: true,
    })
}

impl GpuChief {
    pub fn completion(&self) -> Option<String> {
        self.completion.lock().clone()
    }

    pub fn set_completion(&self, model: &str) {
        let model = model.trim();
        if !model.is_empty() {
            *self.completion.lock() = Some(model.to_owned());
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<GpuPlan>> {
        self.plan.subscribe()
    }

    /// Éditeurs à l'écoute du plan.
    pub fn listeners(&self) -> usize {
        self.plan.receiver_count()
    }

    /// Une proposition de téléchargement peut être montrée (aucune depuis
    /// `gap`) ; elle compte comme montrée.
    pub fn may_offer(&self, gap: Duration) -> bool {
        let mut offered = self.offered.lock();
        if offered.is_some_and(|at| at.elapsed() < gap) {
            return false;
        }
        *offered = Some(Instant::now());
        true
    }

    /// Un chargement ou un déchargement vient d'avoir lieu.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    pub async fn poked(&self) {
        self.poke.notified().await;
    }

    /// Machine gardée au plus `keep` (la détection lance `nvidia-smi`).
    pub async fn machine_within<F>(&self, keep: Duration, detect: F) -> Option<Machine>
    where
        F: std::future::Future<Output = Option<Machine>>,
    {
        let mut kept = self.machine.lock().await;
        if let Some((at, machine)) = kept.as_ref()
            && at.elapsed() < keep
        {
            return machine.clone();
        }
        let machine = detect.await;
        *kept = Some((Instant::now(), machine.clone()));
        machine
    }

    /// Options figées du modèle d'autocomplétion : celles de la conversation
    /// pour le même modèle (sinon chaque changement le rechargerait) ; sinon
    /// 4 096 de contexte et une durée plus courte que la conversation
    /// (stable tant que le réglage ne change pas : `keep_alive` ne recharge
    /// pas le modèle).
    pub fn fixed(inputs: &Inputs, model: &str) -> fim::Fixed {
        if same_model(model, &inputs.chat_model) {
            fim::Fixed {
                num_ctx: inputs.server.context_length,
                keep_alive: inputs.server.keep_alive,
            }
        } else {
            let server = server_keep_alive(std::env::var("OLLAMA_KEEP_ALIVE").ok().as_deref());
            fim::Fixed {
                num_ctx: COMPLETION_CONTEXT,
                keep_alive: keep_alive_beside(inputs.server.keep_alive, server),
            }
        }
    }

    /// Modèles installés (gardés [`TAGS_KEEP`], relus si `fresh`).
    async fn installed(&self, url: &str, fresh: bool) -> Option<Vec<ModelInfo>> {
        let mut tags = self.tags.lock().await;
        if !fresh
            && let Some((at, at_url, list)) = tags.as_ref()
            && at.elapsed() < TAGS_KEEP
            && at_url == url
        {
            return Some(list.clone());
        }
        let list = models::list(url).await.ok()?;
        *tags = Some((Instant::now(), url.to_owned(), list.clone()));
        Some(list)
    }

    /// Ce que `/api/show` dit du modèle (gardé) ; `None` si le modèle est
    /// inconnu du serveur.
    async fn shown(&self, url: &str, model: &str) -> Option<Shown> {
        let key = (url.to_owned(), model.to_ascii_lowercase());
        if let Some(known) = self.shown.lock().get(&key) {
            return Some(*known);
        }
        let details = models::show(url, model).await.ok()?;
        let known = Shown {
            insert: details.capabilities.iter().any(|c| c == "insert"),
            remote: details.remote,
            max_context: details.max_context,
        };
        self.shown.lock().insert(key, known);
        Some(known)
    }

    /// Le modèle est en mémoire avec d'autres options que celles de
    /// l'autocomplétion (contexte) : une proposition le rechargerait
    /// (`needsReload` d'Ollama) ; même règle que l'envoi d'un message.
    async fn loaded_otherwise(
        &self,
        inputs: &Inputs,
        model: &str,
        running: &[RunningModel],
    ) -> bool {
        if !running.iter().any(|m| same_model(&m.name, model)) {
            return false;
        }
        let max_context = self
            .shown(&inputs.server.url, model)
            .await
            .and_then(|shown| shown.max_context);
        crate::chat::needs_loading(
            model,
            running,
            Self::fixed(inputs, model).num_ctx,
            max_context,
        )
    }

    /// Mémoire estimée d'un modèle installé (ou déjà vu en mémoire, ou
    /// proposé) avec ce contexte.
    fn estimate(
        &self,
        url: &str,
        list: Option<&[ModelInfo]>,
        model: &str,
        context: u32,
    ) -> Option<u64> {
        if let Some(info) = list
            .unwrap_or_default()
            .iter()
            .find(|info| same_model(&info.model, model))
        {
            let parameters = crate::ollama::auto::parameters(&info.parameter_size)
                .map(|parameters| parameters as u64);
            return Some(suggest::estimate(info.size, parameters, context));
        }
        let seen = self
            .seen
            .lock()
            .get(&(url.to_owned(), model.to_ascii_lowercase()))
            .copied();
        if seen.is_some() {
            return seen;
        }
        suggest::known(model)
            .map(|known| suggest::estimate(known.weights, Some(known.parameters), context))
    }

    /// Plan du moment (publié aux éditeurs à l'écoute s'il a changé).
    pub async fn plan(&self, inputs: &Inputs) -> GpuPlan {
        let url = inputs.server.url.as_str();
        let running = running::list_within(url, PS_TIMEOUT).await.ok();
        self.plan_with(inputs, running.as_deref()).await
    }

    async fn plan_with(&self, inputs: &Inputs, running: Option<&[RunningModel]>) -> GpuPlan {
        let url = inputs.server.url.as_str();
        let chat = Some(inputs.chat_model.trim())
            .filter(|model| !model.is_empty() && !models::is_cloud_name(model));
        let completion = self.completion();
        *self.last.lock() = Some((url.to_owned(), inputs.chat_model.clone()));
        // Tailles constatées : estimation des modèles hors de `/api/tags`.
        for loaded in running.unwrap_or_default() {
            for name in [&loaded.name, &models::display_name(&loaded.name)] {
                self.seen
                    .lock()
                    .insert((url.to_owned(), name.to_ascii_lowercase()), loaded.size);
            }
        }
        let memory = memory_of(inputs.machine.as_ref());
        let list = if inputs.local {
            self.installed(url, false).await
        } else {
            None
        };
        let vram = memory
            .filter(|memory| memory.gpu)
            .map_or(0, |memory| memory.total);
        let chat_context = if inputs.server.context_length > 0 {
            inputs.server.context_length
        } else {
            fit::default_context(vram)
        };
        let chat_estimate =
            chat.and_then(|chat| self.estimate(url, list.as_deref(), chat, chat_context));
        let completion_estimate = completion.as_deref().and_then(|model| {
            let context = match Self::fixed(inputs, model).num_ctx {
                0 => chat_context,
                context => context,
            };
            self.estimate(url, list.as_deref(), model, context)
        });
        let completion_stale = match (completion.as_deref(), running) {
            (Some(model), Some(running)) if inputs.local => {
                self.loaded_otherwise(inputs, model, running).await
            }
            _ => false,
        };
        let seen_exclusive = match (&*self.exclusive.lock(), chat, completion.as_deref()) {
            (Some((seen_chat, seen_completion)), Some(chat), Some(completion)) => {
                same_model(seen_chat, chat) && same_model(seen_completion, completion)
            }
            _ => false,
        };
        let plan = plan::decide(&plan::Facts {
            running,
            chat,
            completion: completion.as_deref(),
            memory,
            chat_estimate,
            completion_estimate,
            completion_stale,
            busy: (inputs.busy)(),
            local: inputs.local,
            seen_exclusive,
        });
        // Les deux de nouveau ensemble, entiers : le constat d'éviction tombe.
        if plan.mode == plan::Mode::Together
            && plan.completion.as_ref().is_some_and(|c| c.loaded)
            && plan.chat.as_ref().is_some_and(|c| c.loaded)
        {
            *self.exclusive.lock() = None;
        }
        self.plan.send_if_modified(|kept| {
            if kept.as_ref() == Some(&plan) {
                false
            } else {
                *kept = Some(plan.clone());
                true
            }
        });
        plan
    }

    /// Modèle utilisable par l'autocomplétion sur ce serveur : local, pas
    /// du cloud.
    fn local_model(inputs: &Inputs, model: &str) -> Result<(), Refusal> {
        if !inputs.local || models::is_cloud_name(model) {
            return Err(Refusal::Paused(Reason::NotLocal));
        }
        Ok(())
    }

    /// « Préparer » : charge le modèle d'autocomplétion si le plan le
    /// permet, puis constate. Les deux ne tiennent pas ensemble : il est
    /// déchargé (et la conversation chassée, rechargée).
    pub async fn prepare(&self, inputs: &Inputs, model: &str) -> Result<GpuPlan, Refusal> {
        let model = model.trim();
        Self::local_model(inputs, model)?;
        let url = inputs.server.url.clone();
        // Inconnu de `/api/show` : pas installé (jamais téléchargé ici).
        match self.shown(&url, model).await {
            None => return Err(Refusal::Paused(Reason::NotInstalled)),
            Some(Shown { remote: true, .. }) => return Err(Refusal::Paused(Reason::NotLocal)),
            Some(_) => {}
        }
        self.set_completion(model);
        let before = self.plan(inputs).await;
        match plan::prepare_step(&before) {
            Step::Ready => return Ok(before),
            Step::Paused(reason) => return Err(Refusal::Paused(reason)),
            Step::Load => {}
        }
        let fixed = Self::fixed(inputs, model);
        // Une réponse (ou un autre travail) commence : le chargement est
        // lâché (connexion fermée : Ollama saute une requête encore en
        // file), sinon Ollama chasserait la conversation à la fin de sa
        // réponse pour le faire.
        let loaded = tokio::select! {
            loaded = running::try_load(&url, model, fixed.num_ctx, fixed.keep_alive) => loaded,
            () = until_busy(&*inputs.busy) => {
                self.poke();
                return Err(Refusal::Paused(Reason::ChatBusy));
            }
        };
        self.poke();
        if let Err(failure) = loaded {
            return Err(Refusal::Refused(
                crate::error::AppError::from(failure).to_string(),
            ));
        }
        let after = self.plan(inputs).await;
        match plan::after_load(&before, &after) {
            // Le constat d'éviction ne tombe que si les deux se voient
            // ensemble, entiers (`plan_with`) : chargé seul, rien n'est
            // prouvé.
            Loaded::Ready => Ok(after),
            Loaded::Exclusive { reload_chat } => {
                *self.exclusive.lock() = Some((inputs.chat_model.clone(), model.to_owned()));
                let _ = running::unload(&url, model).await;
                if reload_chat {
                    // La conversation, chassée, revient avec ses options
                    // (le prochain message ne la rechargerait qu'alors).
                    let server = inputs.server.clone();
                    let chat = inputs.chat_model.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = running::load(
                            &server.url,
                            &chat,
                            server.context_length,
                            server.keep_alive,
                        )
                        .await
                        {
                            // i18n-ignore : journal.
                            eprintln!("chatonlm: chat model not reloaded: {error}");
                        }
                    });
                }
                self.poke();
                self.plan(inputs).await;
                Err(Refusal::Paused(Reason::Exclusive))
            }
            Loaded::DoesNotFit => {
                let _ = running::unload(&url, model).await;
                self.poke();
                self.plan(inputs).await;
                Err(Refusal::Paused(Reason::DoesNotFit))
            }
            Loaded::Missing => Err(Refusal::Paused(Reason::Exclusive)),
        }
    }

    /// Le modèle peut servir maintenant (local, aucune réponse en cours,
    /// déjà en mémoire avec les options de l'autocomplétion, pas sur le
    /// point d'en sortir) : manière de demander le milieu, options figées et
    /// contexte du modèle chargé. Rien n'est chargé ici.
    async fn ready_for(&self, inputs: &Inputs, model: &str) -> Result<Ready, Refusal> {
        Self::local_model(inputs, model)?;
        if (inputs.busy)() {
            return Err(Refusal::Paused(Reason::ChatBusy));
        }
        self.set_completion(model);
        let url = inputs.server.url.as_str();
        let listed = running::list_within(url, PS_TIMEOUT).await.ok();
        let Some(running) = listed
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|m| same_model(&m.name, model))
        else {
            return Err(Refusal::Paused(Reason::NotLoaded));
        };
        let leaving = running.expires_at.is_some_and(|at| {
            let soon = std::time::SystemTime::now() + EXPIRY_MARGIN;
            let soon = soon
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| {
                    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                });
            at < soon
        });
        let Some(shown) = self.shown(url, model).await else {
            return Err(Refusal::Paused(Reason::NotInstalled));
        };
        if shown.remote {
            return Err(Refusal::Paused(Reason::NotLocal));
        }
        // Chargé avec un autre contexte (bouton Load, autre client, ancien
        // réglage) : la proposition le rechargerait ; seul « Préparer »
        // recharge.
        let fixed = Self::fixed(inputs, model);
        let listed = listed.as_deref().unwrap_or_default();
        if leaving || crate::chat::needs_loading(model, listed, fixed.num_ctx, shown.max_context) {
            return Err(Refusal::Paused(Reason::NotLoaded));
        }
        let way = fim::way(model, shown.insert)
            .ok_or_else(|| Refusal::Refused(t!("bridge.completion.noFim", model = model)))?;
        let loaded_context = running.context_length.filter(|context| *context > 0);
        let mut fixed = fixed;
        if fixed.num_ctx == 0
            && let Some(context) = loaded_context
        {
            // Contexte du serveur (même modèle que la conversation, réglage
            // « Default ») : celui du modèle chargé, demandé tel quel. Rien
            // d'envoyé, Ollama rechargerait un modèle chargé avec un contexte
            // explicite (`needsReload` ne garde le chargé que si les deux
            // sont automatiques) ; le même nombre ne recharge jamais.
            fixed.num_ctx = context;
        }
        let context = loaded_context.unwrap_or(match fixed.num_ctx {
            0 => COMPLETION_CONTEXT,
            context => context,
        });
        Ok(Ready {
            way,
            fixed,
            context,
        })
    }

    /// Une proposition (`complete.fim`) ; jamais de chargement.
    pub async fn complete(
        &self,
        inputs: &Inputs,
        params: &fim::FimParams,
    ) -> Result<fim::Completed, Refusal> {
        let model = params.model.trim();
        let ready = self.ready_for(inputs, model).await?;
        let multiline = params.multiline();
        let tokens = params.tokens();
        // Préfixe et suffixe ramenés au contexte du modèle chargé.
        let (prefix, suffix) = fim::fit(
            &params.prefix,
            &params.suffix,
            fim::budget(ready.context, tokens),
        );
        let body = fim::body(
            model,
            prefix,
            suffix,
            fim::way_for(ready.way, suffix),
            ready.fixed,
            tokens,
            multiline,
        );
        // Relu juste avant l'envoi : un travail commencé pendant les
        // vérifications passe d'abord.
        if (inputs.busy)() {
            return Err(Refusal::Paused(Reason::ChatBusy));
        }
        fim::complete(&inputs.server.url, &body, multiline, fim::family(model))
            .await
            .map_err(|fim::FimFailure::Failed(message)| {
                Refusal::Refused(t!("bridge.completion.failed", detail = message))
            })
    }

    /// Mesure de la vitesse : une requête de chauffe, puis [`MEASURES`],
    /// durées lues dans les réponses d'Ollama. Le modèle doit rester en
    /// mémoire entre deux requêtes (sinon la suivante le rechargerait).
    pub async fn measure(&self, inputs: &Inputs, model: &str) -> Result<Measure, Refusal> {
        let model = model.trim();
        let mut speeds = Vec::with_capacity(MEASURES);
        for round in 0..=MEASURES {
            let Ready { way, fixed, .. } = self.ready_for(inputs, model).await?;
            let (prefix, suffix) = fim::measure_prompt(round);
            let mut body = fim::body(model, &prefix, &suffix, way, fixed, MEASURED_TOKENS, true);
            // Assez de jetons pour mesurer : seuls les jetons de fin arrêtent.
            body["options"]["stop"] =
                serde_json::json!(fim::family(model).map_or(&[][..], |family| family.stops()));
            let durations = fim::measured(&inputs.server.url, &body).await.map_err(
                |fim::FimFailure::Failed(message)| {
                    Refusal::Refused(t!("bridge.completion.failed", detail = message))
                },
            )?;
            if round > 0 {
                speeds.push(fim::speed(&durations));
            }
        }
        speeds.sort_by_key(|(first, _)| *first);
        let (first_token_ms, _) = speeds[speeds.len() / 2];
        let mut rates: Vec<f64> = speeds.iter().map(|(_, speed)| *speed).collect();
        rates.sort_by(f64::total_cmp);
        let tokens_per_second = rates[rates.len() / 2];
        let plan = self.plan(inputs).await;
        let partial_gpu = plan
            .completion
            .as_ref()
            .is_some_and(|completion| completion.partial_gpu);
        Ok(Measure {
            rating: fim::rate(first_token_ms, tokens_per_second),
            first_token_ms,
            tokens_per_second: (tokens_per_second * 10.0).round() / 10.0,
            partial_gpu,
        })
    }

    /// Propositions de modèles pour cette machine.
    pub async fn suggest(&self, inputs: &Inputs) -> suggest::Suggestions {
        let installed: Vec<String> = if inputs.local {
            self.installed(&inputs.server.url, true)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|info| info.model)
                .collect()
        } else {
            Vec::new()
        };
        suggest::suggest(inputs.machine.as_ref(), &installed, COMPLETION_CONTEXT)
    }

    /// Avant une image : le modèle d'autocomplétion quitte la mémoire
    /// (sd-server ne prend que la VRAM libre), sauf s'il sert aussi la
    /// conversation. Sans autocomplétion, rien.
    pub async fn before_image(&self) {
        let Some(completion) = self.completion() else {
            return;
        };
        let Some((url, chat)) = self.last.lock().clone() else {
            return;
        };
        if same_model(&completion, &chat) {
            return;
        }
        let loaded = running::list_within(&url, PS_TIMEOUT)
            .await
            .ok()
            .is_some_and(|list| list.iter().any(|m| same_model(&m.name, &completion)));
        if loaded {
            if let Err(error) = running::unload(&url, &completion).await {
                // i18n-ignore : journal.
                eprintln!("chatonlm: completion model not unloaded before an image: {error}");
            }
            self.poke();
        }
    }

    /// Pour les essais : serveur et modèle de conversation connus.
    #[cfg(test)]
    pub fn remember(&self, url: &str, chat: &str) {
        *self.last.lock() = Some((url.to_owned(), chat.to_owned()));
    }

    /// Le sondage s'arrête (plus personne n'écoute) ; vrai s'il doit
    /// pourtant continuer (un éditeur arrivé entre-temps).
    pub fn stop_polling(&self) -> bool {
        self.polling.store(false, Ordering::SeqCst);
        self.listeners() > 0
            && self
                .polling
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }
}

/// Modèle prêt à servir une proposition.
struct Ready {
    way: fim::Way,
    fixed: fim::Fixed,
    /// Contexte du modèle chargé (jetons).
    context: u32,
}

/// Se termine dès que `busy` est vrai.
async fn until_busy(busy: &(dyn Fn() -> bool + Send + Sync)) {
    while !busy() {
        tokio::time::sleep(BUSY_CHECK).await;
    }
}

#[cfg(test)]
mod tests;
