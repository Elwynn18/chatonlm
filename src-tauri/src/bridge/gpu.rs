//! Méthodes du chef de la carte graphique et de l'autocomplétion (lot 3,
//! contrat `.tools/research/91-lot3-gpu-completion-contract.md` § E) :
//! `gpu.status` (puis notifications `gpu.changed`), `completion.prepare`,
//! `complete.fim`, `completion.measure`, `completion.suggest` et
//! `models.offerPull`. La logique est dans [`crate::gpu`] ; ici, ce que le
//! pont en fait : lecture des réglages, refus en codes JSON-RPC, écoute du
//! plan par connexion et sondage de `/api/ps` tant qu'un éditeur écoute.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::connection::{Outbox, code, is_local};
use super::{Host, Shared};
use crate::chat::Server;
use crate::error::AppResult;
use crate::gpu::plan::Reason;
use crate::gpu::{Inputs, Refusal, fim, suggest};

/// Sondage de `/api/ps` tant qu'un éditeur écoute.
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
    let settings = crate::ollama::target::request_settings(&shared.store).await?;
    let url = settings.effective_server_url.clone();
    let keep = if fresh { Duration::ZERO } else { MACHINE_KEEP };
    let machine = shared
        .host
        .gpu()
        .machine_within(keep, shared.host.machine(url.clone()))
        .await;
    let busy = Arc::clone(shared);
    Ok(Inputs {
        server: Server::from_settings(&settings),
        chat_model: settings.selected_model.clone(),
        local: on_this_computer(&url),
        // Réponse de conversation, ou autre travail de l'app sur Ollama.
        busy: Box::new(move || busy.host.engine().is_working()),
        machine,
    })
}

/// Serveur sur cette machine pour l'autocomplétion : `localhost`, boucle
/// locale, adresse d'écoute ; pas un nom `*.localhost` (résolu en boucle
/// locale par systemd-resolved, mais pas sous Windows ni par tout DNS).
/// Un tunnel vers `localhost` passe pour local : limite connue.
fn on_this_computer(url: &str) -> bool {
    is_local(url)
        && url::Url::parse(url).is_ok_and(|url| match url.host() {
            Some(url::Host::Domain(domain)) => domain
                .trim_end_matches('.')
                .eq_ignore_ascii_case("localhost"),
            _ => true,
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

/// `model` non vide, ou paramètres invalides.
fn model_of(params: Value, required: bool) -> Result<Option<String>, ()> {
    let params = if params.is_null() { json!({}) } else { params };
    let params = serde_json::from_value::<ModelParams>(params).map_err(|_| ())?;
    let model = params
        .model
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty());
    if required && model.is_none() {
        return Err(());
    }
    Ok(model)
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
    let Ok(model) = model_of(params, false) else {
        return invalid(out, id);
    };
    if let Some(model) = &model {
        shared.host.gpu().set_completion(model);
    }
    let inputs = match inputs(shared, false).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    let plan = shared.host.gpu().plan(&inputs).await;
    out.result(id, json!(plan));
    listen(shared, connection, out);
}

/// `completion.prepare { model }` → `{ plan }`, ou en pause.
pub(super) async fn prepare<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(Some(model)) = model_of(params, true) else {
        return invalid(out, id);
    };
    let inputs = match inputs(shared, true).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    match shared.host.gpu().prepare(&inputs, &model).await {
        Ok(plan) => out.result(id, json!({ "plan": plan })),
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
    let Ok(params) = serde_json::from_value::<fim::FimParams>(params) else {
        return invalid(out, id);
    };
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
    let Ok(Some(model)) = model_of(params, true) else {
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

/// `completion.suggest {}` → `{ recommended, options }`.
pub(super) async fn suggest<H: Host>(shared: &Arc<Shared<H>>, out: &Outbox, id: &Value) {
    let inputs = match inputs(shared, true).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    out.result(id, json!(shared.host.gpu().suggest(&inputs).await));
}

/// `models.offerPull { model }` → `{ installed }` : un modèle proposé par
/// `completion.suggest` et absent est montré dans l'app, qui demande à
/// l'utilisateur de le télécharger (rien n'est téléchargé par le pont).
pub(super) async fn offer_pull<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(Some(model)) = model_of(params, true) else {
        return invalid(out, id);
    };
    let Some(known) = suggest::known(&model) else {
        return invalid(out, id);
    };
    let inputs = match inputs(shared, false).await {
        Ok(inputs) => inputs,
        Err(error) => return refused(out, id, &error),
    };
    if !inputs.local {
        return paused(out, id, Reason::NotLocal);
    }
    let installed = shared
        .host
        .gpu()
        .suggest(&inputs)
        .await
        .options
        .iter()
        .any(|option| option.model == known.model && option.installed);
    if !installed && shared.host.gpu().may_offer(OFFER_GAP) {
        shared.host.offer_pull(known.model, known.commercial_use);
    }
    out.result(id, json!({ "installed": installed }));
}

/// Se termine quand une réponse de conversation s'écrit, ou qu'un autre
/// travail de l'app tient Ollama (tout de suite si c'est déjà le cas).
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
/// `/api/ps` est sondé tant qu'un éditeur écoute.
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
/// de réponse ou d'un autre travail de l'app sur Ollama.
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

#[cfg(test)]
mod tests {
    use super::on_this_computer;

    /// Rapport 92, m7 : seulement ce qui désigne sûrement cette machine.
    #[test]
    fn completion_needs_a_server_on_this_computer() {
        for url in [
            "http://localhost:11434",
            "http://LOCALHOST.:11434",
            "http://127.0.0.1:11434",
            "http://127.0.0.2:11434",
            "http://[::1]:11434",
            "http://0.0.0.0:11434",
        ] {
            assert!(on_this_computer(url), "{url}");
        }
        for url in [
            "http://ollama.localhost:11434",
            "http://192.168.1.20:11434",
            "http://desktop.lan:11434",
            "not a url",
        ] {
            assert!(!on_this_computer(url), "{url}");
        }
    }
}
