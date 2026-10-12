//! Une seule requête en vol par clé : un appel qui trouve la même ressource en
//! cours de chargement attend son résultat au lieu d'en lancer une autre
//! (deux `hub_home` simultanés ne doublent pas les requêtes à Hugging Face).

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;

use parking_lot::Mutex;
use tokio::sync::watch;

#[derive(Default)]
pub struct Flights {
    /// Clé -> récepteur du résultat (`watch::Receiver<Option<T>>`).
    running: Mutex<HashMap<String, Box<dyn Any + Send + Sync>>>,
}

enum Role<T> {
    Lead(watch::Sender<Option<T>>),
    Follow(watch::Receiver<Option<T>>),
    /// Clé occupée par un autre type de résultat : sans partage.
    Alone,
}

/// Retire la clé à la fin du vol, même s'il est abandonné en route.
struct Landing<'a> {
    flights: &'a Flights,
    key: &'a str,
}

impl Drop for Landing<'_> {
    fn drop(&mut self) {
        self.flights.running.lock().remove(self.key);
    }
}

impl Flights {
    /// Résultat de `load` pour `key` : lancé ici si aucun vol de cette clé
    /// n'est en cours, sinon celui du vol en cours. Si le vol suivi est
    /// abandonné (appel annulé), un suiveur prend le relais.
    pub async fn run<T, F, Fut>(&self, key: &str, load: F) -> T
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let mut load = Some(load);
        loop {
            let role = {
                let mut running = self.running.lock();
                match running.get(key) {
                    Some(any) => match any.downcast_ref::<watch::Receiver<Option<T>>>() {
                        Some(receiver) => Role::Follow(receiver.clone()),
                        None => Role::Alone,
                    },
                    None => {
                        let (sender, receiver) = watch::channel(None);
                        running.insert(key.to_owned(), Box::new(receiver));
                        Role::Lead(sender)
                    }
                }
            };
            match role {
                Role::Lead(sender) => {
                    let _landing = Landing { flights: self, key };
                    let value = (load.take().expect("a single lead"))().await;
                    sender.send_replace(Some(value.clone()));
                    return value;
                }
                Role::Follow(mut receiver) => {
                    if let Ok(value) = receiver.wait_for(Option::is_some).await {
                        return value.clone().expect("a value");
                    }
                    // Vol abandonné : on recommence (et on mène, sans doute).
                }
                Role::Alone => return (load.take().expect("a single run"))().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn concurrent_calls_share_one_load() {
        let flights = Flights::default();
        let loads = AtomicUsize::new(0);
        let load = || async {
            loads.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            7u32
        };
        let (a, b, c) = tokio::join!(
            flights.run("k", load),
            flights.run("k", load),
            flights.run("k", load)
        );
        assert_eq!((a, b, c), (7, 7, 7));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        // Clé libérée : un nouvel appel recharge.
        assert_eq!(flights.run("k", load).await, 7);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        // Autre clé : vol séparé.
        let (x, y) = tokio::join!(flights.run("k", load), flights.run("other", load));
        assert_eq!((x, y), (7, 7));
        assert_eq!(loads.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn an_abandoned_flight_is_taken_over() {
        let flights = Arc::new(Flights::default());
        let leader = {
            let flights = flights.clone();
            tokio::spawn(async move {
                flights
                    .run("k", || async {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        1u32
                    })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let follower = {
            let flights = flights.clone();
            tokio::spawn(async move { flights.run("k", || async { 2u32 }).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        leader.abort();
        let value = tokio::time::timeout(Duration::from_secs(5), follower)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(value, 2);
        assert!(flights.running.lock().is_empty());
    }
}
