//! Crochet [`RemoteModels`] de l'app : listes des fournisseurs actifs
//! (`/v1/models`), sessions par `Endpoint::remote`.
//!
//! Listes : une liste reçue est gardée [`TTL`] ; un échec est retenu
//! [`FAILURE_TTL`] (rien n'est redemandé avant : un fournisseur éteint ne
//! ralentit pas chaque liste) ; une requête part en tâche de fond et n'est
//! attendue que [`LIST_BUDGET`] depuis son départ, par tous ceux qui la
//! demandent : au-delà, la liste gardée, sinon rien (la requête continue et
//! remplit la liste pour la fois suivante). Fournisseurs demandés en
//! parallèle : la liste des modèles locaux n'attend jamais plus d'un délai.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared, join_all};
use parking_lot::Mutex;

use super::catalog::{self, RemoteModel};
use super::keys::Keys;
use super::providers::{base_of, dialect_of, needs_key};
use super::{RemoteModels, assumed_capabilities, note};
use crate::inference::Endpoint;
use crate::llm::models::{ChatModel, ModelOrigin, capability_words};
use crate::llm::{LlmError, Session, ids};
use crate::local_models::ModelCapabilities;
use crate::store::Store;
use crate::store::remote::{self as rows, ProviderRow};

/// Durée de vie d'une liste gardée.
const TTL: Duration = Duration::from_secs(10 * 60);
/// Durée pendant laquelle un échec de la liste est retenu.
const FAILURE_TTL: Duration = Duration::from_secs(60);
/// Attente d'une liste, au plus, depuis le départ de sa requête.
const LIST_BUDGET: Duration = Duration::from_secs(3);
/// Attente des en-têtes d'une réponse de conversation (un fournisseur qui
/// accepte la connexion puis se tait ne tient pas l'envoi sans fin).
const HEADER_TIMEOUT: Duration = Duration::from_secs(120);

/// Requête de liste en cours, partagée par ceux qui l'attendent.
type Pending = Shared<BoxFuture<'static, ()>>;

/// Ce qui est su de la liste d'un fournisseur.
#[derive(Default)]
struct Slot {
    /// Dernière liste reçue et sa date.
    models: Option<(Instant, Vec<RemoteModel>)>,
    /// Dernier échec (plus récent que la liste).
    failed: Option<Instant>,
    /// Requête en cours et son départ.
    pending: Option<(Instant, Pending)>,
}

#[derive(Default)]
struct Cache {
    /// Change à chaque [`RemoteModels::invalidate`] : une requête partie
    /// avant n'écrit rien.
    generation: u64,
    slots: HashMap<String, Slot>,
}

impl Slot {
    fn kept(&self) -> Vec<RemoteModel> {
        self.models
            .as_ref()
            .map(|(_, models)| models.clone())
            .unwrap_or_default()
    }
}

/// Modèles distants d'une installation.
pub struct Remote {
    store: Store,
    data_dir: PathBuf,
    cache: Arc<Mutex<Cache>>,
    budget: Duration,
    header_timeout: Duration,
}

impl Remote {
    pub fn new(store: Store, data_dir: PathBuf) -> Remote {
        Remote {
            store,
            data_dir,
            cache: Arc::default(),
            budget: LIST_BUDGET,
            header_timeout: HEADER_TIMEOUT,
        }
    }

    /// Essais : attente d'une liste et des en-têtes raccourcies.
    #[cfg(test)]
    pub(crate) fn with_timeouts(mut self, budget: Duration, header_timeout: Duration) -> Remote {
        self.budget = budget;
        self.header_timeout = header_timeout;
        self
    }

    fn keys(&self) -> Keys {
        Keys::new(&self.data_dir)
    }

    /// Clé à envoyer : `None` si elle manque alors que la sorte l'exige.
    fn key_of(&self, row: &ProviderRow) -> Option<String> {
        let key = self.keys().get(&row.id).unwrap_or_default();
        (!key.is_empty() || !needs_key(row.kind)).then_some(key)
    }

    /// Modèles d'un fournisseur : la liste gardée si elle est récente ou si
    /// un échec récent est retenu ; sinon la requête (lancée au besoin) est
    /// attendue jusqu'à [`LIST_BUDGET`] APRÈS SON DÉPART (pas après chaque
    /// demande) : l'envoi qui l'a attendue pour ses détails ne l'attend pas
    /// une seconde fois à l'ouverture.
    async fn models(&self, row: &ProviderRow) -> Vec<RemoteModel> {
        let waiting = {
            let mut cache = self.cache.lock();
            let generation = cache.generation;
            let slot = cache.slots.entry(row.id.clone()).or_default();
            let failed = slot.failed.is_some_and(|at| at.elapsed() < FAILURE_TTL);
            let fresh = slot
                .models
                .as_ref()
                .is_some_and(|(at, _)| at.elapsed() < TTL);
            if failed || fresh {
                return slot.kept();
            }
            match &slot.pending {
                Some((started, pending)) => (*started, pending.clone()),
                None => {
                    let (Some(base), Some(key)) = (base_of(row), self.key_of(row)) else {
                        return Vec::new();
                    };
                    let pending = self.spawn_fetch(row, base, key, generation);
                    let started = Instant::now();
                    slot.pending = Some((started, pending.clone()));
                    (started, pending)
                }
            }
        };
        let (started, pending) = waiting;
        let left = self.budget.saturating_sub(started.elapsed());
        let _ = tokio::time::timeout(left, pending).await;
        self.cache
            .lock()
            .slots
            .get(&row.id)
            .map(Slot::kept)
            .unwrap_or_default()
    }

    /// Requête de la liste en tâche de fond : elle va à son terme (délais
    /// du client) et écrit son résultat, sauf si les listes ont été
    /// oubliées entre-temps.
    fn spawn_fetch(
        &self,
        row: &ProviderRow,
        base: String,
        key: String,
        generation: u64,
    ) -> Pending {
        let (cache, id, kind) = (self.cache.clone(), row.id.clone(), row.kind);
        let task = tokio::spawn(async move {
            let fetched = catalog::fetch(kind, &base, &key).await;
            if let Err(problem) = &fetched {
                note(&format!("models of {id}: {problem:?}"));
            }
            let mut cache = cache.lock();
            if cache.generation != generation {
                return;
            }
            let slot = cache.slots.entry(id).or_default();
            slot.pending = None;
            match fetched {
                Ok(models) => {
                    slot.models = Some((Instant::now(), models));
                    slot.failed = None;
                }
                Err(_) => slot.failed = Some(Instant::now()),
            }
        });
        task.map(drop).boxed().shared()
    }

    async fn enabled_rows(&self) -> Vec<ProviderRow> {
        match self.store.call(|c| rows::list(c)).await {
            Ok(rows) => rows.into_iter().filter(|row| row.enabled).collect(),
            Err(error) => {
                note(&format!("providers unreadable: {error}"));
                Vec::new()
            }
        }
    }

    /// Ligne active du fournisseur `provider`.
    async fn enabled_row(&self, provider: &str) -> Option<ProviderRow> {
        let id = provider.to_owned();
        self.store
            .call(move |c| rows::get(c, &id))
            .await
            .ok()
            .flatten()
            .filter(|row| row.enabled)
    }

    async fn known(&self, provider: &str, model: &str) -> Option<RemoteModel> {
        let row = self.enabled_row(provider).await?;
        self.models(&row)
            .await
            .into_iter()
            .find(|entry| entry.id == model)
    }

    async fn session(&self, provider: &str, model: &str) -> Result<Session, LlmError> {
        let row = self
            .enabled_row(provider)
            .await
            .ok_or(LlmError::RemoteUnavailable)?;
        let base = base_of(&row).ok_or(LlmError::RemoteUnavailable)?;
        let key = self.key_of(&row).ok_or_else(|| LlmError::RemoteKey {
            provider: row.name.clone(),
        })?;
        // Capacités et contexte de la liste ; inconnu (liste muette, en
        // échec ou partielle) : conversation et outils.
        let known = self
            .models(&row)
            .await
            .into_iter()
            .find(|entry| entry.id == model);
        let (context, capabilities) = match known {
            Some(entry) => (entry.context, entry.capabilities),
            None => (None, assumed_capabilities()),
        };
        let mut endpoint = Endpoint::remote(base, key)
            .map_err(|_| LlmError::RemoteUnreachable {
                provider: row.name.clone(),
            })?
            .with_header_timeout(self.header_timeout);
        endpoint.dialect = dialect_of(row.kind);
        Ok(Session::remote(
            ids::remote_id(provider, model),
            model.to_owned(),
            row.name,
            endpoint,
            context,
            capabilities,
        ))
    }
}

/// Entrée du sélecteur d'un modèle distant.
pub fn picker_entry(row: &ProviderRow, model: &RemoteModel) -> ChatModel {
    ChatModel {
        id: ids::remote_id(&row.id, &model.id),
        display_name: model.name.clone(),
        origin: ModelOrigin::Remote,
        provider: Some(row.name.clone()),
        size_bytes: None,
        params_label: None,
        quant: None,
        context_max: model.context,
        capabilities: capability_words(&model.capabilities),
        installed_at: None,
    }
}

impl RemoteModels for Remote {
    fn list(&self) -> BoxFuture<'_, Vec<ChatModel>> {
        Box::pin(async move {
            let rows = self.enabled_rows().await;
            let lists = join_all(rows.iter().map(|row| self.models(row))).await;
            rows.iter()
                .zip(lists)
                .flat_map(|(row, models)| {
                    models
                        .iter()
                        .map(|model| picker_entry(row, model))
                        .collect::<Vec<_>>()
                })
                .collect()
        })
    }

    fn open<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
    ) -> BoxFuture<'a, Result<Session, LlmError>> {
        Box::pin(self.session(provider, model))
    }

    fn capabilities<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
    ) -> BoxFuture<'a, ModelCapabilities> {
        Box::pin(async move {
            self.known(provider, model)
                .await
                .map_or_else(assumed_capabilities, |entry| entry.capabilities)
        })
    }

    fn invalidate(&self) {
        let mut cache = self.cache.lock();
        cache.generation += 1;
        cache.slots.clear();
    }
}
