//! Trait [`Host`] : ce que le pipeline demande au socle (base, disque,
//! questions, priorité de l'utilisateur). Contrat :
//! `.tools/research/111-lot2-contrat.md`. Implémenté par le socle
//! (`workshop/service.rs`), et par un faux en mémoire dans les essais du
//! pipeline.
//!
//! Futurs boxés (`HostFuture`) plutôt que `async fn` : le trait reste
//! utilisable en `&dyn Host` et ses futurs sont `Send` (tâche tokio), sans
//! dépendre d'`async-trait`.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::types::{Answer, EventKind, ProjectInfo, Question, StateKey};

/// Futur rendu par un [`Host`].
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Le socle d'un projet, vu du pipeline.
pub trait Host: Send + Sync {
    /// Le projet : tâche, pièces jointes (textes extraits), stratégie
    /// imposée, autonomie, langue de l'app, dossier.
    fn project(&self) -> &ProjectInfo;

    /// Enregistre l'événement, le diffuse ; rend son numéro.
    fn emit(&self, kind: EventKind) -> HostFuture<'_, u64>;

    /// Sauve un état reprenable (remplace la valeur de `key`).
    fn save(&self, key: StateKey, value: Value) -> HostFuture<'_, ()>;

    /// Lit un état sauvé.
    fn load(&self, key: StateKey) -> HostFuture<'_, Option<Value>>;

    /// Pose une question : projet en attente (`Waiting`), notification,
    /// attente de la réponse ; [`Answer::Cancelled`] si le projet est annulé
    /// pendant l'attente.
    fn ask(&self, question: Question) -> HostFuture<'_, Answer>;

    /// Cède la place à l'utilisateur : attend que l'app soit libre
    /// (`Interactive::idle`), avec les événements `Yielding` / `Resumed` s'il
    /// a fallu attendre. Appelé avant CHAQUE requête au modèle.
    fn yield_to_user(&self) -> HostFuture<'_, ()>;

    /// Écrit `bytes` sous le dossier du projet, au chemin relatif `rel`
    /// (garde-fou du socle : relatif, sans « .. », sans lien suivi). Erreur :
    /// code.
    fn write_file<'a>(
        &'a self,
        rel: &'a str,
        bytes: &'a [u8],
    ) -> HostFuture<'a, Result<(), String>>;

    /// Arrêt du projet (pause ou annulation : le socle sait lequel).
    fn cancelled(&self) -> CancellationToken;

    /// Nom du projet trouvé au cadrage (`title`, vide : aucun) : le socle en
    /// fait le titre du projet et, si l'utilisateur n'a pas nommé le dossier,
    /// renomme le dossier provisoire. Sans effet en cas d'échec (dit au
    /// journal de l'app) ; appelé de nouveau à une reprise.
    fn name_project<'a>(&'a self, title: &'a str) -> HostFuture<'a, ()> {
        let _ = title;
        Box::pin(async {})
    }
}
