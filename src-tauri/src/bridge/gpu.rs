//! Méthodes du chef de la carte graphique et de l'autocomplétion (lot 3,
//! contrat `.tools/research/91-lot3-gpu-completion-contract.md` § E) :
//! `gpu.status` (puis notifications `gpu.changed`), `completion.prepare`,
//! `complete.fim`, `completion.measure`, `completion.suggest` et
//! `models.offerPull`. La logique est dans [`crate::gpu`] ; ici, ce que le
//! pont en fait : lecture des réglages, refus en codes JSON-RPC, écoute du
//! plan par connexion et sondage du moteur tant qu'un éditeur écoute.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::connection::{Outbox, app_settings, code, editor_model};
use super::{Host, Shared};
use crate::chat::Server;
use crate::error::AppResult;
use crate::gpu::plan::Reason;
use crate::gpu::{Inputs, Refusal, fim, suggest};

/// Sondage du moteur tant qu'un éditeur écoute.
const POLL: Duration = Duration::from_secs(1);
/// Écart minimal entre deux `gpu.changed` d'une connexion (2 par seconde).
const NOTIFY_GAP: Duration = Duration::from_millis(500);
/// Machine (VRAM libre) gardée par le sondage.
const MACHINE_KEEP: Duration = Duration::from_secs(5);
/// Deux propositions de téléchargement montrées à au moins cet écart (un
/// éditeur qui insiste ne vole pas le premier plan à chaque appel).
const OFFER_GAP: Duration = Duration::from_secs(10);

/// Ce que le chef lit de l'app ; `fresh` : mémoire de la machine relue.
pub(super) async fn inputs<H: Host>(shared: &Arc<Shared<H>>, fresh: bool) -> AppResult<Inputs> {
    let settings = app_settings(&shared.store).await?;
    let keep = if fresh { Duration::ZERO } else { MACHINE_KEEP };
    let machine = shared
        .host
        .gpu()
        .machine_within(keep, shared.host.machine())
        .await;
    let busy = Arc::clone(shared);
    Ok(Inputs {
        server: Server::current(&shared.access).await?,
        chat_model: settings.selected_model.clone(),
        // Le moteur des modèles est celui de l'app, sur cette machine.
        local: true,
        // Réponse de conversation, ou autre travail de l'app sur un modèle.
        busy: Box::new(move || busy.host.engine().is_working()),
        machine,
    })
}

/// -32007 : l'autocomplétion attend, avec sa raison.
pub(super) fn paused(out: &Outbox, id: &Value, reason: Reason) {
    out.error(
        id,
        code::PAUSED,
        "paused",
        Some(json!({ "reason": reason.code() })),
    );
}

fn refusal(out: &Outbox, id: &Value, refusal: Refusal) {
    match refusal {
        Refusal::Paused(reason) => paused(out, id, reason),
        Refusal::Refused(message) => out.error(
            id,
            code::REFUSED,
            "refused",
            Some(json!({ "message": message })),
        ),
    }
}

fn refused(out: &Outbox, id: &Value, error: &crate::error::AppError) {
    refusal(out, id, Refusal::Refused(error.to_string()));
}

fn invalid(out: &Outbox, id: &Value) {
    out.error(id, code::INVALID_PARAMS, "invalid params", None);
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ModelParams {
    model: Option<String>,
}

/// Modèle servi pour le nom `model` de l'éditeur : un ancien nom d'Ollama
/// d'un modèle proposé (réglage d'une extension 0.1.x) devient le modèle
/// repris d'Ollama s'il est installé, sinon son identifiant
/// ([`suggest::current`]) ; la base n'est lue que pour un tel nom.
async fn served<H: Host>(shared: &Arc<Shared<H>>, model: &str) -> String {
    if !suggest::is_old_name(model) {
        return model.trim().to_owned();
    }
    suggest::current(model, &local_models(shared).await)
}

/// `model` non vide (`(nom demandé, modèle servi)`, voir [`served`] ; un
/// `@ollama/<nom>` sans étiquette reçoit `:latest`), ou paramètres
/// invalides.
async fn model_of<H: Host>(
    shared: &Arc<Shared<H>>,
    params: Value,
    required: bool,
) -> Result<Option<(String, String)>, ()> {
    let params = if params.is_null() { json!({}) } else { params };
    let params = serde_json::from_value::<ModelParams>(params).map_err(|_| ())?;
    let Some(asked) = params
        .model
        .map(|model| editor_model(&model))
        .filter(|model| !model.is_empty())
    else {
        return if required { Err(()) } else { Ok(None) };
    };
    let current = served(shared, &asked).await;
    Ok(Some((asked, current)))
}

/// Retient le modèle d'autocomplétion demandé par la connexion : sous un
/// ancien nom renommé, ce nom lui est rendu dans le plan ([`for_connection`]).
fn remember<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    (asked, current): &(String, String),
) {
    let alias = (asked != current).then(|| (asked.clone(), current.clone()));
    shared.set_completion_alias(connection, alias);
}

/// Plan tel que la connexion le lit : le modèle d'autocomplétion qu'elle a
/// demandé sous un ancien nom d'Ollama porte ce nom (une extension 0.1.x
/// compare le plan à son réglage, qu'elle ne renomme pas : sans cela, elle
/// attendrait sans fin un modèle « pas chargé »).
fn for_connection<H: Host>(shared: &Shared<H>, connection: u64, plan: Value) -> Value {
    let mut plan = plan;
    if let Some((asked, current)) = shared.completion_alias(connection)
        && plan["completion"]["model"]
            .as_str()
            .is_some_and(|model| suggest::same_id(model, &current))
    {
        plan["completion"]["model"] = json!(asked);
    }
    plan
}

/// `gpu.status { model? }` → plan ; la connexion reçoit ensuite
/// `gpu.changed { plan }` à chaque changement (au plus deux par seconde).
pub(super) async fn status<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(model) = model_of(shared, params, false).await else {
        return invalid(out, id);
    };
    if let Some(model) = &model {
        remember(shared, connection, model);
        shared.host.gpu().set_completion(&model.1);
    }
    let inputs = match inputs(shared, false).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    let plan = shared.host.gpu().plan(&inputs).await;
    out.result(id, for_connection(shared, connection, json!(plan)));
    listen(shared, connection, out);
}

/// `completion.prepare { model }` → `{ plan }`, ou en pause.
pub(super) async fn prepare<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(Some(model)) = model_of(shared, params, true).await else {
        return invalid(out, id);
    };
    remember(shared, connection, &model);
    let inputs = match inputs(shared, true).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    match shared.host.gpu().prepare(&inputs, &model.1).await {
        Ok(plan) => out.result(
            id,
            json!({ "plan": for_connection(shared, connection, json!(plan)) }),
        ),
        Err(error) => refusal(out, id, error),
    }
}

/// `complete.fim` → `{ text, firstTokenMs, totalMs, cachedTokens, timedOut }`.
pub(super) async fn complete<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(mut params) = serde_json::from_value::<fim::FimParams>(params) else {
        return invalid(out, id);
    };
    params.model = served(shared, &editor_model(&params.model)).await;
    if !params.valid() {
        return invalid(out, id);
    }
    let inputs = match inputs(shared, false).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    match shared.host.gpu().complete(&inputs, &params).await {
        Ok(done) => out.result(
            id,
            json!({
                "text": done.text,
                "firstTokenMs": done.first_token_ms,
                "totalMs": done.total_ms,
                "cachedTokens": done.cached_tokens,
                "timedOut": done.timed_out,
            }),
        ),
        Err(error) => refusal(out, id, error),
    }
}

/// `completion.measure { model }` → `{ rating, firstTokenMs, tokensPerSecond,
/// partialGpu }`.
pub(super) async fn measure<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(Some((_, model))) = model_of(shared, params, true).await else {
        return invalid(out, id);
    };
    let inputs = match inputs(shared, false).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    match shared.host.gpu().measure(&inputs, &model).await {
        Ok(measure) => out.result(id, json!(measure)),
        Err(error) => refusal(out, id, error),
    }
}

/// Identifiants de la base locale des modèles (téléchargés par le ModelHub ;
/// liste vide si la base ne se lit pas).
async fn local_models<H: Host>(shared: &Arc<Shared<H>>) -> Vec<String> {
    shared
        .store
        .call(|connection| {
            Ok(crate::store::local_models::list(connection)?
                .into_iter()
                .map(|model| model.id)
                .collect())
        })
        .await
        .unwrap_or_default()
}

/// `completion.suggest {}` → `{ recommended, options }`.
pub(super) async fn suggest<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value) {
    let inputs = match inputs(shared, true).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    let local = local_models(shared).await;
    out.result(id, json!(shared.host.gpu().suggest(&inputs, &local).await));
}

/// `models.offerPull { model }` → `{ installed }` : un modèle proposé par
/// `completion.suggest` et absent est montré dans l'app, qui demande à
/// l'utilisateur de le télécharger (rien n'est téléchargé par le pont). Un
/// modèle proposé déjà là (dépôt, ou modèle repris d'Ollama qui en tient
/// lieu) : `installed`, rien n'est montré.
pub(super) async fn offer_pull<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(Some((_, model))) = model_of(shared, params, true).await else {
        return invalid(out, id);
    };
    let inputs = match inputs(shared, false).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    let local = local_models(shared).await;
    let suggested = shared.host.gpu().suggest(&inputs, &local).await;
    let proposed = suggested
        .options
        .iter()
        .find(|option| suggest::same_id(&option.model, &model));
    let Some(known) = suggest::known(&model) else {
        // Modèle repris d'Ollama proposé à la place du dépôt : déjà là.
        return match proposed {
            Some(option) if option.installed && inputs.local => {
                out.result(id, json!({ "installed": true }));
            }
            _ => invalid(out, id),
        };
    };
    if !inputs.local {
        return paused(out, id, Reason::NotLocal);
    }
    let installed = proposed.is_some_and(|option| option.installed);
    if !installed && shared.host.gpu().may_offer(OFFER_GAP) {
        shared.host.offer_pull(known.model, known.commercial_use);
    }
    out.result(id, json!({ "installed": installed }));
}

/// Se termine quand une réponse de conversation s'écrit, ou qu'un autre
/// travail de l'app tient un modèle (tout de suite si c'est déjà le cas).
pub(super) async fn chat_begins<H: Host>(shared: &Arc<Shared<H>>) {
    let engine = shared.host.engine();
    loop {
        // Créé avant la lecture : un début entre les deux le réveille.
        let notified = engine.activity().notified();
        if engine.is_working() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_millis(250), notified).await;
    }
}

/// La connexion écoute le plan : ses changements lui sont envoyés, et
/// le moteur est sondé tant qu'un éditeur écoute.
fn listen<H: Host>(shared: &Arc<Shared<H>>, connection: u64, out: &Outbox) {
    if !shared.listen_gpu(connection) {
        return;
    }
    let mut plans = shared.host.gpu().subscribe();
    plans.mark_unchanged();
    let (forwarding, out) = (Arc::clone(shared), out.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            let changed = tokio::select! {
                changed = plans.changed() => match changed {
                    Ok(()) => true,
                    Err(_) => break,
                },
                () = tokio::time::sleep(POLL) => false,
            };
            if !forwarding.is_open(connection) {
                break;
            }
            if changed {
                let plan = plans.borrow_and_update().clone();
                if let Some(plan) = plan {
                    let plan = for_connection(&forwarding, connection, json!(plan));
                    out.notify("gpu.changed", json!({ "plan": plan }));
                }
                tokio::time::sleep(NOTIFY_GAP).await;
            }
        }
    });
    poll(shared);
}

/// Sondage du plan (un seul pour le pont), arrêté quand plus personne
/// n'écoute ; réveillé aussi par un chargement et par un début ou une fin
/// de réponse ou d'un autre travail de l'app sur un modèle.
fn poll<H: Host>(shared: &Arc<Shared<H>>) {
    if shared
        .host
        .gpu()
        .polling
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let shared = Arc::clone(shared);
    tauri::async_runtime::spawn(async move {
        loop {
            let gpu = shared.host.gpu();
            if gpu.listeners() == 0 && !gpu.stop_polling() {
                break;
            }
            if let Ok(inputs) = inputs(&shared, false).await {
                gpu.plan(&inputs).await;
            }
            tokio::select! {
                () = tokio::time::sleep(POLL) => {}
                () = gpu.poked() => {}
                () = shared.host.engine().activity().notified() => {}
            }
        }
    });
}
