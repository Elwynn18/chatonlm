//! Chef de la carte graphique (propre à ChatonLM, lot 3 du lien VS Code) :
//! un seul décideur pour le modèle de la conversation et celui de
//! l'autocomplétion de l'éditeur. Contrat : `.tools/research/91-lot3-gpu-
//! completion-contract.md` ; passage à llama.cpp : contrat 102 § 3 (lot L5a).
//!
//! - L'autocomplétion ne charge JAMAIS un modèle d'elle-même : une requête
//!   n'est transmise que si le modèle est déjà prêt dans le moteur avec les
//!   options de l'autocomplétion ([`crate::engine::Engine::is_ready`] ;
//!   sinon le moteur le relancerait, ou chasserait la conversation). Seul
//!   « Préparer » charge, et seulement si les deux tiennent ensemble ou si la
//!   conversation n'est pas en mémoire.
//! - Options figées par modèle : contexte 4 096 et `keep_alive` toujours
//!   plus court que celui de la conversation ([`keep_alive_beside`] : il
//!   quitte la mémoire le premier) ; même modèle que la conversation : les
//!   réglages de la conversation, résolus comme son ouverture
//!   ([`crate::llm::Access::open`]).
//! - Décision par constat (modèles du moteur, [`engine_facts`]), estimation
//!   seulement pour prévoir ([`plan`]).
//! - Pendant qu'une réponse de conversation s'écrit, ou qu'un autre travail
//!   de l'app tient le moteur ([`activity`] : Correction, Étude, polissage,
//!   chargement…), l'autocomplétion attend (calcul partagé) ; « Préparer »
//!   s'arrête aussi (le chargement qu'il avait lancé est arrêté).
//! - Proposition : `/infill` sur le `Lease` du modèle, qui doit avoir les
//!   jetons FIM (capacité `fim` de la base locale) ; tenu le temps de la
//!   requête, jamais rouvert sous d'autres options (relecture de L3, M10).
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
use crate::engine::{KeepAlive, LoadOptions};
use crate::hub::fit;
use crate::hub::hardware::Machine;
use crate::i18n::t;
use crate::llm::Access;
use crate::local_models::LocalModel;
use crate::local_models::id::same_model;
use plan::{GpuPlan, Loaded, Memory, Reason, RunningModel, Step};

/// Contexte de l'autocomplétion (même valeur à chaque requête).
pub const COMPLETION_CONTEXT: u32 = 4096;
/// Durée en mémoire du modèle d'autocomplétion, au plus (voir
/// [`keep_alive_beside`]).
pub const COMPLETION_KEEP_ALIVE: i64 = 600;
/// Un modèle qui quitte la mémoire dans moins que cela ne sert plus : la
/// proposition arriverait après son départ et le relancerait.
const EXPIRY_MARGIN: Duration = Duration::from_secs(3);
/// Relecture de « quelque chose d'autre tient le moteur » pendant
/// « Préparer ».
const BUSY_CHECK: Duration = Duration::from_millis(50);
/// Liste des modèles installés gardée.
const INSTALLED_KEEP: Duration = Duration::from_secs(10);
/// Requêtes de la mesure (après une de chauffe).
const MEASURES: usize = 3;
/// Jetons générés par une mesure.
const MEASURED_TOKENS: u32 = 32;

/// Ce que le chef lit de l'app au moment d'agir.
pub struct Inputs {
    pub server: Server,
    /// Modèle de la conversation (réglage).
    pub chat_model: String,
    /// Moteur sur cette machine (toujours vrai avec le moteur de l'app ;
    /// faux : l'autocomplétion attend, `notLocal`).
    pub local: bool,
    /// Une réponse de conversation s'écrit.
    pub busy: Box<dyn Fn() -> bool + Send + Sync>,
    /// Machine (`None` : illisible).
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
/// qu'il quitte la mémoire le premier : 80 % de la sienne, 10 min au plus.
/// `chat` : réglage de la conversation (0 : `engine_default`, défaut du
/// moteur) ; négatif : gardée chargée.
pub fn keep_alive_beside(chat: i64, engine_default: i64) -> i64 {
    let chat = if chat == 0 { engine_default } else { chat };
    if chat < 0 {
        return COMPLETION_KEEP_ALIVE;
    }
    (chat * 4 / 5).clamp(1, COMPLETION_KEEP_ALIVE)
}

/// Durée par défaut du moteur (secondes) : celle de `KeepAlive::Default`.
fn engine_keep_alive() -> i64 {
    KeepAlive::Default.duration().map_or(-1, |duration| {
        i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
    })
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
    /// Modèles installés : (lue à, liste).
    installed: tokio::sync::Mutex<Option<(Instant, Vec<LocalModel>)>>,
    /// Taille constatée dans le moteur, par modèle (en minuscules) :
    /// estimation d'un modèle qui n'est plus en mémoire.
    seen: Mutex<HashMap<String, u64>>,
    /// Dernier plan, pour les éditeurs qui écoutent.
    plan: watch::Sender<Option<GpuPlan>>,
    /// Quelque chose a changé (chargement, déchargement) : replanifier.
    poke: Notify,
    /// Dernier accès et modèle de conversation vus (avant une image).
    last: Mutex<Option<(Access, String)>>,
    /// Le sondage du moteur tourne.
    pub polling: AtomicBool,
    /// Machine gardée par le sondage : (lue à, machine).
    machine: tokio::sync::Mutex<Option<(Instant, Option<Machine>)>>,
    /// Dernière proposition de téléchargement montrée (`models.offerPull`).
    offered: Mutex<Option<Instant>>,
    /// Moteur de l'app, attaché au démarrage : avant une image sans plan
    /// encore fait.
    engine: std::sync::OnceLock<crate::engine::Engine>,
}

impl Default for GpuChief {
    fn default() -> Self {
        Self {
            completion: Mutex::new(None),
            exclusive: Mutex::new(None),
            installed: tokio::sync::Mutex::new(None),
            seen: Mutex::new(HashMap::new()),
            plan: watch::channel(None).0,
            poke: Notify::new(),
            last: Mutex::new(None),
            polling: AtomicBool::new(false),
            machine: tokio::sync::Mutex::new(None),
            offered: Mutex::new(None),
            engine: std::sync::OnceLock::new(),
        }
    }
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

/// Options de chargement d'un modèle avec ces options figées, résolues
/// comme [`crate::llm::Access::open`] le fait pour la conversation
/// (contexte imposé, sinon réglage, puis celui repris du modèle, puis
/// moteur) ; `None` : modèle absent de la base locale. Rend aussi le modèle
/// (identifiant canonique, capacités).
async fn resolved(
    access: &Access,
    model: &str,
    fixed: fim::Fixed,
) -> Option<(LocalModel, LoadOptions)> {
    let local = crate::local_models::get(access.store(), access.data_dir(), model)
        .await
        .ok()
        .flatten()?;
    let key = local.id.clone();
    let defaults = access
        .store()
        .call(move |connection| crate::store::model_import::defaults(connection, &key))
        .await
        .ok()
        .flatten();
    let ctx = Some(fixed.num_ctx)
        .filter(|ctx| *ctx > 0)
        .or_else(|| crate::llm::defaults::context(0, defaults.as_ref()));
    let options = LoadOptions {
        ctx,
        ..LoadOptions::from_settings(0, fixed.keep_alive)
    };
    Some((local, options))
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

    /// Attache le moteur de l'app (avant une image sans plan encore fait).
    pub fn attach_engine(&self, engine: crate::engine::Engine) {
        let _ = self.engine.set(engine);
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

    /// Options figées du modèle d'autocomplétion : les réglages de la
    /// conversation pour le même modèle (sinon chaque changement le
    /// relancerait) ; sinon 4 096 de contexte et une durée plus courte que
    /// la conversation (stable tant que le réglage ne change pas :
    /// `keep_alive` ne relance pas le modèle).
    pub fn fixed(inputs: &Inputs, model: &str) -> fim::Fixed {
        let prefs = &inputs.server.prefs;
        if same_model(model, inputs.chat_model.trim()) {
            fim::Fixed {
                num_ctx: prefs.context_length,
                keep_alive: prefs.keep_alive,
            }
        } else {
            fim::Fixed {
                num_ctx: COMPLETION_CONTEXT,
                keep_alive: keep_alive_beside(prefs.keep_alive, engine_keep_alive()),
            }
        }
    }

    /// Modèles installés (gardés [`INSTALLED_KEEP`], relus si `fresh`).
    async fn installed(&self, access: &Access, fresh: bool) -> Vec<LocalModel> {
        let mut kept = self.installed.lock().await;
        if !fresh
            && let Some((at, list)) = kept.as_ref()
            && at.elapsed() < INSTALLED_KEEP
        {
            return list.clone();
        }
        let list = crate::local_models::list(access.store(), access.data_dir())
            .await
            .unwrap_or_default();
        *kept = Some((Instant::now(), list.clone()));
        list
    }

    /// Le modèle est en mémoire, mais pas prêt avec les options de
    /// l'autocomplétion (autre contexte : bouton Load, ancien réglage) : une
    /// proposition le relancerait ; seul « Préparer » le relance.
    async fn loaded_otherwise(
        &self,
        inputs: &Inputs,
        model: &str,
        running: &[RunningModel],
    ) -> bool {
        if !running.iter().any(|m| same_model(&m.name, model)) {
            return false;
        }
        let access = &inputs.server.access;
        match resolved(access, model, Self::fixed(inputs, model)).await {
            Some((local, options)) => !access.engine().is_ready(&local.id, options).await,
            None => false,
        }
    }

    /// Mémoire estimée d'un modèle installé (ou déjà vu en mémoire, ou
    /// proposé) avec ce contexte.
    fn estimate(&self, list: &[LocalModel], model: &str, context: u32) -> Option<u64> {
        if let Some(local) = list.iter().find(|local| same_model(&local.id, model)) {
            let parameters = local
                .params_label
                .as_deref()
                .and_then(crate::engine::auto::parameters)
                .map(|parameters| parameters as u64);
            return Some(suggest::estimate(local.size_bytes, parameters, context));
        }
        let seen = self.seen.lock().get(&model.to_ascii_lowercase()).copied();
        if seen.is_some() {
            return seen;
        }
        suggest::known(model)
            .map(|known| suggest::estimate(known.weights, Some(known.parameters), context))
    }

    /// Plan du moment (publié aux éditeurs à l'écoute s'il a changé).
    pub async fn plan(&self, inputs: &Inputs) -> GpuPlan {
        let running = engine_facts(&inputs.server.access.engine().running());
        self.plan_with(inputs, Some(&running)).await
    }

    async fn plan_with(&self, inputs: &Inputs, running: Option<&[RunningModel]>) -> GpuPlan {
        let access = &inputs.server.access;
        let chat = Some(inputs.chat_model.trim())
            .filter(|model| !model.is_empty() && !crate::llm::ids::is_remote(model));
        let completion = self.completion();
        *self.last.lock() = Some((access.clone(), inputs.chat_model.clone()));
        // Tailles constatées : estimation des modèles sortis de la mémoire.
        for loaded in running.unwrap_or_default() {
            self.seen
                .lock()
                .insert(loaded.name.to_ascii_lowercase(), loaded.size);
        }
        let memory = memory_of(inputs.machine.as_ref());
        let list = if inputs.local {
            self.installed(access, false).await
        } else {
            Vec::new()
        };
        let vram = memory
            .filter(|memory| memory.gpu)
            .map_or(0, |memory| memory.total);
        let chat_context = if inputs.server.prefs.context_length > 0 {
            inputs.server.prefs.context_length
        } else {
            fit::default_context(vram)
        };
        let chat_estimate = chat.and_then(|chat| self.estimate(&list, chat, chat_context));
        let completion_estimate = completion.as_deref().and_then(|model| {
            let context = match Self::fixed(inputs, model).num_ctx {
                0 => chat_context,
                context => context,
            };
            self.estimate(&list, model, context)
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

    /// Modèle utilisable par l'autocomplétion : moteur local, modèle qui
    /// n'est pas distant.
    fn local_model(inputs: &Inputs, model: &str) -> Result<(), Refusal> {
        if !inputs.local || crate::llm::ids::is_remote(model) {
            return Err(Refusal::Paused(Reason::NotLocal));
        }
        Ok(())
    }

    /// Modèle installé et capable de FIM, ou refus.
    async fn installed_fim(access: &Access, model: &str) -> Result<LocalModel, Refusal> {
        let local = crate::local_models::get(access.store(), access.data_dir(), model)
            .await
            .ok()
            .flatten()
            .ok_or(Refusal::Paused(Reason::NotInstalled))?;
        if !local.capabilities.fim {
            return Err(Refusal::Refused(t!(
                "bridge.completion.noFim",
                model = model
            )));
        }
        Ok(local)
    }

    /// « Préparer » : charge le modèle d'autocomplétion si le plan le
    /// permet, puis constate. Les deux ne tiennent pas ensemble : il est
    /// déchargé (et la conversation chassée, rechargée).
    pub async fn prepare(&self, inputs: &Inputs, model: &str) -> Result<GpuPlan, Refusal> {
        let model = model.trim();
        Self::local_model(inputs, model)?;
        let access = inputs.server.access.clone();
        // Absent de la base locale : pas installé (jamais téléchargé ici) ;
        // sans jetons FIM : il ne servirait à rien.
        let local = Self::installed_fim(&access, model).await?;
        self.set_completion(model);
        let before = self.plan(inputs).await;
        match plan::prepare_step(&before) {
            Step::Ready => return Ok(before),
            Step::Paused(reason) => return Err(Refusal::Paused(reason)),
            Step::Load => {}
        }
        let fixed = Self::fixed(inputs, model);
        let options = resolved(&access, model, fixed)
            .await
            .map(|(_, options)| options);
        let Some(options) = options else {
            return Err(Refusal::Paused(Reason::NotInstalled));
        };
        // Une réponse (ou un autre travail) commence : le chargement est
        // arrêté (sinon le modèle prendrait la mémoire pendant la réponse),
        // sauf s'il sert aussi la conversation.
        let loaded = tokio::select! {
            loaded = access.engine().preload(&local.id, options) => loaded,
            () = until_busy(&*inputs.busy) => {
                if !same_model(&local.id, inputs.chat_model.trim()) {
                    access.engine().unload(&local.id).await;
                }
                self.poke();
                return Err(Refusal::Paused(Reason::ChatBusy));
            }
        };
        self.poke();
        if let Err(error) = loaded {
            return Err(Refusal::Refused(error.to_string()));
        }
        let after = self.plan(inputs).await;
        match plan::after_load(&before, &after) {
            // Le constat d'éviction ne tombe que si les deux se voient
            // ensemble, entiers (`plan_with`) : chargé seul, rien n'est
            // prouvé.
            Loaded::Ready => Ok(after),
            Loaded::Exclusive { reload_chat } => {
                *self.exclusive.lock() = Some((inputs.chat_model.clone(), model.to_owned()));
                access.engine().unload(&local.id).await;
                if reload_chat {
                    // La conversation, chassée, revient avec ses réglages
                    // (le prochain message ne la rechargerait qu'alors).
                    let chat = inputs.chat_model.clone();
                    let access = access.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = crate::llm::models::load(&access, &chat).await {
                            // i18n-ignore : journal.
                            crate::app_log!("chatonlm: chat model not reloaded: {error:?}");
                        }
                    });
                }
                self.poke();
                self.plan(inputs).await;
                Err(Refusal::Paused(Reason::Exclusive))
            }
            Loaded::DoesNotFit => {
                access.engine().unload(&local.id).await;
                self.poke();
                self.plan(inputs).await;
                Err(Refusal::Paused(Reason::DoesNotFit))
            }
            Loaded::Missing => Err(Refusal::Paused(Reason::Exclusive)),
        }
    }

    /// Le modèle peut servir maintenant (local, aucune réponse en cours,
    /// déjà prêt avec les options de l'autocomplétion, pas sur le point de
    /// sortir de la mémoire, avec ses jetons FIM) : options de chargement à
    /// tenir, famille et contexte du modèle chargé. Rien n'est chargé ici.
    async fn ready_for(&self, inputs: &Inputs, model: &str) -> Result<Ready, Refusal> {
        Self::local_model(inputs, model)?;
        if (inputs.busy)() {
            return Err(Refusal::Paused(Reason::ChatBusy));
        }
        self.set_completion(model);
        let access = &inputs.server.access;
        let running = access.engine().running();
        let Some(running) = running
            .iter()
            .find(|m| same_model(&m.id, model) && m.state == crate::engine::ModelState::Ready)
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
        let local = Self::installed_fim(access, model).await?;
        // Chargé avec un autre contexte (bouton Load, ancien réglage) : la
        // proposition le relancerait ; seul « Préparer » relance.
        let Some((_, options)) = resolved(access, model, Self::fixed(inputs, model)).await else {
            return Err(Refusal::Paused(Reason::NotInstalled));
        };
        if leaving || !access.engine().is_ready(&local.id, options).await {
            return Err(Refusal::Paused(Reason::NotLoaded));
        }
        Ok(Ready {
            id: local.id.clone(),
            options,
            family: fim::family(&local.repo).or_else(|| fim::family(&local.id)),
            context: running.context_length,
        })
    }

    /// Tient le modèle prêt (son `Lease`) le temps d'une requête, pris
    /// seulement s'il est encore prêt avec ces options : déchargé ou évincé
    /// depuis [`Self::ready_for`], refus `NotLoaded`, jamais de relance ni
    /// d'attente (relecture L5a, M1).
    async fn lease(access: &Access, ready: &Ready) -> Result<crate::engine::Lease, Refusal> {
        access
            .engine()
            .lease_if_ready(&ready.id, ready.options)
            .await
            .ok_or(Refusal::Paused(Reason::NotLoaded))
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
        let request = fim::request(prefix, suffix, tokens, multiline, ready.family);
        // Relu juste avant l'envoi : un travail commencé pendant les
        // vérifications passe d'abord.
        if (inputs.busy)() {
            return Err(Refusal::Paused(Reason::ChatBusy));
        }
        let lease = Self::lease(&inputs.server.access, &ready).await?;
        let done = fim::complete(&lease.endpoint, &request, multiline, ready.family).await;
        drop(lease);
        done.map_err(|fim::FimFailure::Failed(message)| {
            Refusal::Refused(t!("bridge.completion.failed", detail = message))
        })
    }

    /// Mesure de la vitesse : une requête de chauffe, puis [`MEASURES`],
    /// mesures lues dans les réponses du serveur. Le modèle doit rester prêt
    /// entre deux requêtes (sinon la suivante le relancerait).
    pub async fn measure(&self, inputs: &Inputs, model: &str) -> Result<Measure, Refusal> {
        let model = model.trim();
        let mut speeds = Vec::with_capacity(MEASURES);
        for round in 0..=MEASURES {
            let ready = self.ready_for(inputs, model).await?;
            let (prefix, suffix) = fim::measure_prompt(round);
            let mut request = fim::request(&prefix, &suffix, MEASURED_TOKENS, true, ready.family);
            // Assez de jetons pour mesurer : seuls les jetons de fin arrêtent.
            request.stop = ready.family.map_or_else(Vec::new, |family| {
                family
                    .stops()
                    .iter()
                    .map(|stop| (*stop).to_owned())
                    .collect()
            });
            let lease = Self::lease(&inputs.server.access, &ready).await?;
            let timings = fim::measured(&lease.endpoint, &request).await;
            drop(lease);
            let timings = timings.map_err(|fim::FimFailure::Failed(message)| {
                Refusal::Refused(t!("bridge.completion.failed", detail = message))
            })?;
            if round > 0 {
                speeds.push(fim::speed(&timings));
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

    /// Propositions de modèles pour cette machine ; installés : ceux de la
    /// base locale, plus `local` (donnés par le pont).
    pub async fn suggest(&self, inputs: &Inputs, local: &[String]) -> suggest::Suggestions {
        let installed: Vec<String> = if inputs.local {
            self.installed(&inputs.server.access, true)
                .await
                .into_iter()
                .map(|model| model.id)
                .chain(local.iter().cloned())
                .collect()
        } else {
            Vec::new()
        };
        suggest::suggest(inputs.machine.as_ref(), &installed, COMPLETION_CONTEXT)
    }

    /// Avant une image : le modèle d'autocomplétion quitte la mémoire
    /// (sd-server ne prend que la VRAM libre), sauf s'il sert aussi la
    /// conversation. Sans autocomplétion, rien. Sans plan encore fait (le
    /// modèle de conversation est inconnu) : sur le moteur attaché, et
    /// seulement s'il ne sert aucune requête.
    pub async fn before_image(&self) {
        let Some(completion) = self.completion() else {
            return;
        };
        let last = self.last.lock().clone();
        let (engine, chat) = match &last {
            Some((access, chat)) => (access.engine().clone(), chat.clone()),
            None => match self.engine.get() {
                Some(engine) => (engine.clone(), String::new()),
                None => return,
            },
        };
        if same_model(&completion, chat.trim()) {
            return;
        }
        let loaded = engine
            .running()
            .into_iter()
            .find(|model| same_model(&model.id, &completion));
        if let Some(model) = loaded
            && (last.is_some() || !model.in_use)
        {
            engine.unload(&model.id).await;
            self.poke();
        }
    }

    /// Pour les essais : accès et modèle de conversation connus.
    #[cfg(test)]
    pub fn remember(&self, access: &Access, chat: &str) {
        *self.last.lock() = Some((access.clone(), chat.to_owned()));
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

/// Les modèles du moteur dans la forme des constats du plan : nom = identifiant, mémoire et VRAM lues
/// dans le journal, contexte du processus, échéance ; les modèles en
/// chargement n'y sont pas. Toutes les couches sur la carte : entièrement
/// sur la carte (`size_vram` = `size`), même si
/// llama.cpp garde quelques tampons en mémoire vive ; sinon la VRAM lue (un
/// partage se voit, `size_vram` < `size`).
pub fn engine_facts(models: &[crate::engine::RunningModel]) -> Vec<RunningModel> {
    models
        .iter()
        .filter(|model| model.state == crate::engine::ModelState::Ready)
        .map(|model| RunningModel {
            name: model.id.clone(),
            size: model.size_bytes,
            size_vram: match (model.gpu_layers, model.total_layers) {
                (Some(gpu), Some(total)) if total > 0 && gpu >= total => model.size_bytes,
                _ => model.vram_bytes,
            },
            context_length: Some(model.context_length),
            expires_at: model.expires_at,
            kept: model.kept,
            parameter_size: model.params_label.clone().unwrap_or_default(),
            quantization_level: model
                .id
                .rsplit_once(':')
                .map(|(_, quant)| quant.to_owned())
                .unwrap_or_default(),
        })
        .collect()
}

/// Modèle prêt à servir une proposition.
struct Ready {
    /// Identifiant canonique (base locale).
    id: String,
    /// Options avec lesquelles il est prêt (tenues telles quelles).
    options: LoadOptions,
    family: Option<fim::Family>,
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
