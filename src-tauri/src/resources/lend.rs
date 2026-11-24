//! Prêt de la carte graphique (propre à ChatonLM, Atelier ; conception
//! `109-moteur-taches-conception.md` § 6) : un travail de fond qui tient un
//! modèle de texte le décharge le temps d'un travail qui veut la carte
//! entière (lot d'images, transcription), puis le rouvre.
//!
//! Le prêt ne rouvre pas le modèle : l'appelant a déjà rendu sa session
//! (un modèle tenu ne serait jamais évincé), et la rouvre ensuite par le
//! chemin ordinaire (lot 0 : environ 15 s à chaud, 38 s à froid). Le
//! déchargement est immédiat ([`crate::engine::Engine::unload`], même si une
//! autre session le tenait encore) ; la VRAM rendue (seulement si une
//! instance a vraiment été arrêtée) est signalée au moteur
//! ([`crate::engine::Engine::gpu_freed`]) pour que les modèles repliés sur le
//! processeur réessaient la carte à leur prochain chargement. Code commun à
//! Linux et Windows.

#![cfg_attr(not(test), expect(dead_code, reason = "appelant : l'Atelier (lot 2)"))]

use std::future::Future;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::time::Instant;

use super::Resources;

/// Durées d'un prêt (vue d'avancement, journal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LendReport {
    /// Déchargement du modèle de texte (0 sans modèle à décharger).
    pub unload_ms: u64,
    /// Travail fait avec la carte.
    pub job_ms: u64,
}

/// Ce que le prêt demande au moteur de texte (le vrai moteur, ou un faux
/// dans les essais).
pub(crate) trait TextEngine: Send + Sync {
    /// Arrête le modèle `id` et attend la fin de son processus ; `false` :
    /// il n'était pas chargé (rien d'arrêté).
    fn unload<'a>(&'a self, id: &'a str) -> BoxFuture<'a, bool>;
    /// VRAM rendue (quantité inconnue).
    fn gpu_freed(&self);
}

impl TextEngine for crate::engine::Engine {
    fn unload<'a>(&'a self, id: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // Instances (prêtes ou en chargement) que `Engine::unload` prend.
            let loaded = self
                .running()
                .iter()
                .any(|model| crate::local_models::id::same_model(&model.id, id));
            crate::engine::Engine::unload(self, id).await;
            loaded
        })
    }

    fn gpu_freed(&self) {
        crate::engine::Engine::gpu_freed(self, None);
    }
}

impl Resources {
    /// Prête la carte à `job` : décharge `text_model` (s'il est donné ; sans
    /// effet s'il n'est pas chargé), signale la VRAM rendue (s'il l'était),
    /// attend `job`, rend son résultat et les durées. L'appelant a rendu sa
    /// session avant, et rouvre le modèle de texte après.
    ///
    /// L'appelant (l'Atelier, lot 2) attend d'abord que l'utilisateur ne se
    /// serve plus de l'app ([`super::interactive::Interactive::idle`]) : le
    /// prêt décharge le modèle même si une conversation s'en servait.
    pub async fn lend_gpu<T>(
        &self,
        engine: &crate::engine::Engine,
        text_model: Option<&str>,
        job: impl Future<Output = T>,
    ) -> (T, LendReport) {
        lend(engine, text_model, job).await
    }
}

/// Travail de [`Resources::lend_gpu`], sur tout moteur de texte.
pub(crate) async fn lend<T>(
    engine: &(impl TextEngine + ?Sized),
    text_model: Option<&str>,
    job: impl Future<Output = T>,
) -> (T, LendReport) {
    let started = Instant::now();
    if let Some(model) = text_model
        && engine.unload(model).await
    {
        engine.gpu_freed();
        // i18n-ignore : journal.
        crate::app_log!("chatonlm: text model {model} unloaded to lend the graphics card");
    }
    let unloaded = Instant::now();
    let value = job.await;
    let report = LendReport {
        unload_ms: millis(unloaded - started),
        job_ms: millis(unloaded.elapsed()),
    };
    (value, report)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    /// Faux moteur : journal des appels, déchargement de 2 s (sauf
    /// `absent` : modèle pas chargé, rien d'arrêté).
    #[derive(Default)]
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        absent: bool,
    }

    impl TextEngine for Fake {
        fn unload<'a>(&'a self, id: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async move {
                self.log.lock().push(format!("unload {id} start"));
                if self.absent {
                    return false;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                self.log.lock().push(format!("unload {id} end"));
                true
            })
        }

        fn gpu_freed(&self) {
            self.log.lock().push("gpu freed".into());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_text_model_is_unloaded_before_the_job_and_times_are_reported() {
        let engine = Fake::default();
        let log = engine.log.clone();
        let job = async move {
            log.lock().push("job start".into());
            tokio::time::sleep(Duration::from_secs(3)).await;
            log.lock().push("job end".into());
            42
        };
        let (value, report) = lend(&engine, Some("qwen3-8b"), job).await;
        assert_eq!(value, 42);
        assert_eq!(
            *engine.log.lock(),
            [
                "unload qwen3-8b start",
                "unload qwen3-8b end",
                "gpu freed",
                "job start",
                "job end",
            ]
        );
        assert_eq!(
            report,
            LendReport {
                unload_ms: 2_000,
                job_ms: 3_000,
            }
        );
    }

    /// Avec le vrai moteur (faux llama-server) : le modèle de texte est
    /// arrêté, processus fini, avant que le travail ne commence.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_engine_lets_go_of_the_model_for_the_job() {
        let bench = crate::engine::tests::bench();
        let id = bench.install("simple", 1 << 30);
        bench
            .engine
            .preload(&id, crate::engine::LoadOptions::default())
            .await
            .unwrap();
        let pid = bench.engine.pid_of(&id).expect("model running");
        let engine = bench.engine.clone();
        let (seen, _) = Resources::new()
            .lend_gpu(&bench.engine, Some(&id), async move {
                (engine.running().len(), crate::engine::tests::alive(pid))
            })
            .await;
        assert_eq!(seen, (0, false), "the model still runs during the job");
        bench.engine.shutdown().await;
    }

    /// Modèle nommé mais pas chargé : rien d'arrêté, aucune VRAM signalée
    /// (les modèles repliés sur le processeur ne réessaient pas la carte
    /// pour rien).
    #[tokio::test(start_paused = true)]
    async fn an_unloaded_text_model_frees_nothing() {
        let engine = Fake {
            absent: true,
            ..Fake::default()
        };
        let (value, _) = lend(&engine, Some("qwen3-8b"), async { 7 }).await;
        assert_eq!(value, 7);
        assert_eq!(*engine.log.lock(), ["unload qwen3-8b start"]);
    }

    /// Avec le vrai moteur : un modèle qui n'est pas chargé n'est pas
    /// déchargé ; un modèle chargé l'est.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_engine_tells_whether_a_model_was_unloaded() {
        let bench = crate::engine::tests::bench();
        let id = bench.install("simple", 1 << 30);
        assert!(!TextEngine::unload(&bench.engine, &id).await);
        bench
            .engine
            .preload(&id, crate::engine::LoadOptions::default())
            .await
            .unwrap();
        assert!(TextEngine::unload(&bench.engine, &id).await);
        assert!(bench.engine.running().is_empty());
        bench.engine.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_text_model_only_the_job_runs() {
        let engine = Fake::default();
        let (value, report) = lend(&engine, None, async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            "done"
        })
        .await;
        assert_eq!(value, "done");
        assert!(engine.log.lock().is_empty());
        assert_eq!(
            report,
            LendReport {
                unload_ms: 0,
                job_ms: 500,
            }
        );
    }
}
