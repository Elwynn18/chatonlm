//! Travaux de l'app demandés par l'éditeur (lot 4a du lien VS Code, contrat
//! `.tools/research/93-lot4a-contract.md`) :
//!
//! - `notify.send` / `notify.task` : Easy Link prévient les comptes liés à la
//!   fin d'une tâche, et `/status` montre les tâches (réglage de l'app coupé
//!   par défaut ; bornes dans [`crate::easy_link::notify`]) ;
//! - `study.create` / `study.list` : cours de l'Étude sur des textes envoyés
//!   par l'éditeur (jamais un chemin lu par l'app) ; ensuite, la connexion
//!   reçoit chaque changement d'un cours en `study.event` (diffusion du
//!   moteur, quel que soit l'appel qui l'a causé) ; le cours vit dans
//!   l'onglet Study ;
//! - `image.generate` : image du moteur d'images, sans conversation ; les
//!   octets du PNG reviennent à l'éditeur, l'app n'écrit aucun fichier ;
//!   progression en `image.event` ; annulable ;
//! - `proofread.run` : correction ou reformulation d'un texte (passages finis
//!   en `proofread.event`) ; annulable ;
//! - `app.show { route? }` : montre la fenêtre, sur une page d'une liste
//!   blanche seulement (cours ou conversation existants, Images du ModelHub,
//!   onglets Easy Link et VS Code des réglages).

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use super::connection::{Outbox, code};
use super::{Host, Shared};
use crate::chat::Server;
use crate::chat::images::{self, ImageProgress, PaintRefusal, PaintRequest};
use crate::easy_link::notify::{Note, NotifyError, Outcome, TaskState, TaskUpdate};
use crate::i18n::{Lang, t};
use crate::proofread::{self, Cancel, ProofreadEvent, ProofreadMode};
use crate::study::{CourseSummary, Ctx, StudyLevel, StudyUpdated};

/// Longueur permise d'un nom de tâche reçu (il est ensuite réduit à 80
/// caractères) et d'un chemin de dossier.
const MAX_RAW_TITLE: usize = 4_000;

/// Description d'une image, au plus (caractères).
pub const MAX_PROMPT: usize = 4_000;

fn invalid(out: &Outbox, id: &Value) {
    out.error(id, code::INVALID_PARAMS, "invalid params", None);
}

fn refused(out: &Outbox, id: &Value, message: String) {
    out.error(
        id,
        code::REFUSED,
        "refused",
        Some(json!({ "message": message })),
    );
}

fn notify_refused(out: &Outbox, id: &Value, error: NotifyError) {
    match error {
        NotifyError::Disabled => out.error(id, code::NOTIFY_OFF, "notifications off", None),
        NotifyError::NotLinked => out.error(id, code::NOT_LINKED, "easy link not linked", None),
        NotifyError::Failed(message) => refused(out, id, message),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendParams {
    title: String,
    outcome: Outcome,
    duration_ms: u64,
    #[serde(default)]
    workspace: Option<String>,
}

/// `notify.send { title, outcome, durationMs, workspace? }` → `{ sent }`.
pub(super) async fn notify_send<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<SendParams>(params) else {
        return invalid(out, id);
    };
    if params.title.chars().count() > MAX_RAW_TITLE
        || params
            .workspace
            .as_ref()
            .is_some_and(|folder| folder.chars().count() > MAX_RAW_TITLE)
    {
        return invalid(out, id);
    }
    let Some(notifier) = shared.host.notifier() else {
        return notify_refused(out, id, NotifyError::NotLinked);
    };
    let note = Note {
        title: params.title,
        outcome: params.outcome,
        duration_ms: params.duration_ms,
        workspace: params.workspace,
    };
    match notifier.notify(note).await {
        Ok(sent) => out.result(id, json!({ "sent": sent })),
        Err(error) => notify_refused(out, id, error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskParams {
    id: String,
    title: String,
    state: TaskState,
    started_at: i64,
}

/// `notify.task { id, title, state, startedAt }` → `{}` : tâche montrée par
/// `/status` d'Easy Link.
pub(super) async fn notify_task<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<TaskParams>(params) else {
        return invalid(out, id);
    };
    let length = params.id.chars().count();
    if !(1..=64).contains(&length) || params.title.chars().count() > MAX_RAW_TITLE {
        return invalid(out, id);
    }
    let Some(notifier) = shared.host.notifier() else {
        return notify_refused(out, id, NotifyError::NotLinked);
    };
    let update = TaskUpdate {
        id: params.id,
        title: params.title,
        state: params.state,
        started_at: params.started_at,
    };
    match notifier.task(connection, update).await {
        Ok(()) => out.result(id, json!({})),
        Err(error) => notify_refused(out, id, error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    name: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CourseParams {
    #[serde(default)]
    topic: String,
    level: StudyLevel,
    /// Minutes : 10, 20 ou 30.
    duration: u32,
    #[serde(default)]
    documents: Vec<Document>,
    #[serde(default)]
    language: Option<Lang>,
}

/// Nom d'un document montré dans le cours : une ligne, dernier élément d'un
/// chemin, 100 caractères au plus.
fn document_name(name: &str) -> String {
    let line: String = name.chars().filter(|c| !c.is_control()).collect();
    let last = line
        .rsplit(['/', '\\'])
        .find(|part| !part.trim().is_empty())
        .unwrap_or_default()
        .trim();
    let name: String = last.chars().take(100).collect();
    if name.is_empty() {
        "document.txt".to_owned()
    } else {
        name
    }
}

/// La connexion suit les cours : abonnée avant la création ou la lecture
/// (rien de manqué entre-temps), suivi lancé après la réponse.
struct Following {
    closed: watch::Receiver<bool>,
    updates: broadcast::Receiver<StudyUpdated>,
}

/// Abonne la connexion aux changements des cours, une seule fois (`None`
/// si elle l'est déjà, ou sans moteur des cours).
fn follow<H: Host>(shared: &Arc<Shared<H>>, connection: u64) -> Option<Following> {
    let engine = shared.host.study()?;
    // Abonné avant de marquer la connexion : aucun changement perdu.
    let updates = engine.subscribe();
    let closed = shared.listen_study(connection)?;
    Some(Following { closed, updates })
}

impl Following {
    /// Envoie `study.event { courseId, course }` (résumé relu, sans doublon)
    /// à chaque changement d'un cours, quel que soit celui qui l'a causé
    /// (page, cette connexion ou une autre, file de l'Étude lancée par l'une
    /// ou l'autre), jusqu'à la fin de la connexion. Un cours supprimé
    /// n'envoie rien ; en retard, tous les cours sont relus.
    fn start<H: Host>(self, shared: &Arc<Shared<H>>, connection: u64, out: &Outbox) {
        let Following {
            mut closed,
            mut updates,
        } = self;
        let (shared, out) = (Arc::clone(shared), out.clone());
        tauri::async_runtime::spawn(async move {
            let mut last: HashMap<String, CourseSummary> = HashMap::new();
            loop {
                let changed = tokio::select! {
                    // Fermée, ou retirée (émetteur lâché).
                    _ = closed.wait_for(|closed| *closed) => break,
                    changed = updates.recv() => changed,
                };
                let only = match changed {
                    Ok(updated) if updated.deleted => {
                        last.remove(&updated.course_id);
                        continue;
                    }
                    Ok(updated) => Some(updated.course_id),
                    Err(broadcast::error::RecvError::Lagged(_)) => None,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if !shared.is_open(connection) {
                    break;
                }
                let Some(engine) = shared.host.study() else {
                    break;
                };
                let Ok(courses) = engine.list(&shared.store).await else {
                    continue;
                };
                for summary in courses {
                    if only.as_ref().is_some_and(|only| *only != summary.id)
                        || last.get(&summary.id) == Some(&summary)
                    {
                        continue;
                    }
                    out.notify(
                        "study.event",
                        json!({ "courseId": summary.id, "course": summary }),
                    );
                    last.insert(summary.id.clone(), summary);
                }
            }
        });
    }
}

/// `study.create { topic, level, duration, documents: [{ name, text }],
/// language? }` → `{ courseId }` ; la connexion suit ensuite les cours
/// (`study.event`, voir [`Following::start`]) : la préparation du plan et des
/// paliers de ce cours, même si la file de l'Étude sert d'abord un autre
/// cours ou un autre appel.
pub(super) async fn study_create<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<CourseParams>(params) else {
        return invalid(out, id);
    };
    let Some(engine) = shared.host.study() else {
        return refused(out, id, t!("bridge.unavailable"));
    };
    // Au-delà, les documents seraient coupés sans le dire : refus clair.
    let chars: usize = params
        .documents
        .iter()
        .map(|document| document.text.chars().count())
        .sum();
    if chars > crate::study::DOCUMENTS_CHARS {
        return refused(out, id, t!("bridge.study.tooLong"));
    }
    let files: Vec<(String, String)> = params
        .documents
        .into_iter()
        .map(|document| (document_name(&document.name), document.text))
        .collect();
    let host = Arc::clone(shared);
    // La page l'apprend (`study-updated`) ; les connexions qui suivent les
    // cours, par la diffusion du moteur.
    let ctx = Ctx::new(shared.store.clone(), move |updated| {
        host.host.study_updated(&updated);
    });
    let language = params.language.unwrap_or_else(crate::i18n::current);
    let following = follow(shared, connection);
    let created = engine
        .create_from_texts(
            &ctx,
            &params.topic,
            params.level,
            params.duration,
            files,
            language,
        )
        .await;
    match created {
        Ok(course) => out.result(id, json!({ "courseId": course })),
        Err(error) => refused(out, id, error.to_string()),
    }
    if let Some(following) = following {
        following.start(shared, connection, out);
    }
}

/// `study.list {}` → résumés des cours (comme l'onglet Study) ; la connexion
/// suit ensuite les cours (`study.event`), par exemple après une reconnexion.
pub(super) async fn study_list<H: Host>(
    shared: &Arc<Shared<H>>,
    connection: u64,
    out: &Outbox,
    id: &Value,
    params: &Value,
) {
    if !params.is_null() && params.as_object().is_none_or(|fields| !fields.is_empty()) {
        return invalid(out, id);
    }
    let Some(engine) = shared.host.study() else {
        return refused(out, id, t!("bridge.unavailable"));
    };
    let following = follow(shared, connection);
    match engine.list(&shared.store).await {
        Ok(courses) => out.result(id, json!(courses)),
        Err(error) => refused(out, id, error.to_string()),
    }
    if let Some(following) = following {
        following.start(shared, connection, out);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageParams {
    prompt: String,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

/// Raison d'un refus d'image, pour l'extension.
fn paint_reason(refusal: &PaintRefusal) -> &'static str {
    match refusal {
        PaintRefusal::Unavailable => "unavailable",
        PaintRefusal::CpuUnsupported(_) => "cpuUnsupported",
        PaintRefusal::NoModel => "noModel",
    }
}

fn no_image(out: &Outbox, id: &Value, refusal: &PaintRefusal) {
    out.error(
        id,
        code::NO_IMAGE_MODEL,
        "no image model",
        Some(json!({ "reason": paint_reason(refusal), "message": refusal.message() })),
    );
}

/// `image.generate { prompt, width?, height? }` → `{ pngBase64, seed, model,
/// width, height }` ; avancement en `image.event { requestId, progress }`.
/// L'image n'est écrite nulle part : l'éditeur écrit le fichier lui-même.
/// Lâcher le futur (`$/cancelRequest`) arrête l'image.
pub(super) async fn image_generate<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<ImageParams>(params) else {
        return invalid(out, id);
    };
    let prompt = params.prompt.trim().to_owned();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT {
        return invalid(out, id);
    }
    let Some(painter) = shared.host.engine().painter() else {
        return no_image(out, id, &PaintRefusal::Unavailable);
    };
    let ready = match painter.ready().await {
        Ok(ready) => ready,
        Err(refusal) => return no_image(out, id, &refusal),
    };
    let cpu = painter.cpu().await;
    let (width, height) = images::image_size(
        &json!({ "width": params.width, "height": params.height }),
        ready.side(cpu),
    );
    // L'autocomplétion de l'éditeur attend pendant ce temps.
    let _working = shared.host.engine().hold();
    let mut progress = |progress: ImageProgress| {
        out.notify(
            "image.event",
            json!({ "requestId": id, "progress": progress }),
        );
    };
    let request = PaintRequest {
        prompt,
        width,
        height,
    };
    match painter.paint(request, &mut progress).await {
        Ok(painted) => out.result(
            id,
            json!({
                "pngBase64": BASE64.encode(&painted.png),
                "seed": painted.seed,
                "model": painted.model_name,
                "width": painted.width,
                "height": painted.height,
            }),
        ),
        Err(message) => refused(out, id, message),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProofreadParams {
    text: String,
    mode: ProofreadMode,
    /// Accepté, sans effet : le modèle garde la langue du texte, et les
    /// messages de l'app sont dans sa langue.
    #[serde(default)]
    #[allow(dead_code)]
    language: Option<Lang>,
}

/// `proofread.run { text, mode, language? }` → `{ text, model }` ; chaque
/// passage fini en `proofread.event { requestId, event }` (`started`, puis
/// `partDone`). Refus du moteur : -32020 `{ message, code }`.
pub(super) async fn proofread_run<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let Ok(params) = serde_json::from_value::<ProofreadParams>(params) else {
        return invalid(out, id);
    };
    let settings = match crate::commands::models::app_settings(&shared.store).await {
        Ok(settings) => settings,
        Err(error) => return refused(out, id, error.to_string()),
    };
    let model = proofread::model_of(&settings).await.unwrap_or_default();
    // « Auto » : un modèle cloud ou distant n'est utilisé que s'il est
    // choisi sur la page Proofread, comme pour le raccourci de correction
    // (`proofread::selection`) : celui du chat n'y retombe jamais. Refus
    // `noModel` : il faut choisir un modèle.
    if settings.proofread_model.is_empty()
        && !model.is_empty()
        && proofread::selection::runs_elsewhere(&settings.effective_server_url, &model).await
    {
        let message = t!("proofread.selection.cloudHint", model = model);
        return out.error(
            id,
            code::REFUSED,
            "refused",
            Some(json!({ "message": message, "code": proofread::ProofreadErrorCode::NoModel })),
        );
    }
    // L'autocomplétion de l'éditeur attend pendant ce temps (aussi tenu par
    // `proofread::run` sur le compteur de l'app : c'est le même).
    let _working = shared.host.engine().hold();
    let emit = |event: ProofreadEvent| {
        if matches!(
            event,
            ProofreadEvent::Started { .. } | ProofreadEvent::PartDone { .. }
        ) {
            out.notify(
                "proofread.event",
                json!({ "requestId": id, "event": event }),
            );
        }
    };
    let outcome = proofread::run(
        &Server::from_settings(&settings),
        &model,
        params.mode,
        &params.text,
        &Cancel::default(),
        &emit,
    )
    .await;
    match &outcome.error {
        Some((error, message)) => out.error(
            id,
            code::REFUSED,
            "refused",
            Some(json!({ "message": message, "code": error })),
        ),
        None => out.result(
            id,
            json!({ "text": outcome.text(), "model": outcome.model }),
        ),
    }
}

/// Pages fixes que l'éditeur peut faire montrer.
const SHOWN_PAGES: [&str; 3] = [
    "/hub/images",
    "/settings?section=easyLink",
    "/settings?section=vscode",
];

/// Page demandée par `app.show`, d'après sa forme seulement.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    Page,
    Study(String),
    Chat(String),
}

/// Forme d'une page permise (liste blanche) ; `None` sinon. Les identifiants
/// sont des UUID (cours et conversations), en minuscules.
pub(crate) fn shown_route(route: &str) -> Option<Shown> {
    if SHOWN_PAGES.contains(&route) {
        return Some(Shown::Page);
    }
    let uuid = |id: &str| {
        uuid::Uuid::try_parse(id)
            .is_ok_and(|parsed| parsed.hyphenated().to_string() == id)
            .then(|| id.to_owned())
    };
    if let Some(id) = route.strip_prefix("/study/") {
        return uuid(id).map(Shown::Study);
    }
    route.strip_prefix("/c/").and_then(uuid).map(Shown::Chat)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowParams {
    #[serde(default)]
    route: Option<String>,
}

/// `app.show { route? }` → `{}` : la fenêtre de l'app montrée, et menée à
/// `route` si elle est dans la liste blanche (cours ou conversation qui
/// existent) ; sinon -32602 et rien ne s'ouvre.
pub(super) async fn app_show<H: Host>(
    shared: &Arc<Shared<H>>,
    out: &Outbox,
    id: &Value,
    params: Value,
) {
    let params = if params.is_null() {
        ShowParams { route: None }
    } else {
        let Ok(params) = serde_json::from_value::<ShowParams>(params) else {
            return invalid(out, id);
        };
        params
    };
    let Some(route) = params.route else {
        shared.host.show(None);
        return out.result(id, json!({}));
    };
    let exists = match shown_route(&route) {
        None => false,
        Some(Shown::Page) => true,
        Some(Shown::Study(course)) => shared
            .store
            .call(move |c| crate::store::study::row(c, &course))
            .await
            .is_ok_and(|row| row.is_some()),
        Some(Shown::Chat(chat)) => shared
            .store
            .call(move |c| crate::store::chats::get(c, &chat))
            .await
            .is_ok(),
    };
    if !exists {
        return invalid(out, id);
    }
    shared.host.show(Some(&route));
    out.result(id, json!({}));
}
