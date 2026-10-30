//! Essais d'Easy Link de bout en bout, sans fenêtre : faux Ollama, faux
//! Telegram et faux Discord, base en mémoire.

use std::sync::Arc;
use std::time::Duration;

use easy_link::fake::discord::{self as fake_discord, FakeDiscord};
use easy_link::fake::telegram::{self as fake_telegram, FakeTelegram};
use fake_ollama::{FakeOllama, Mode};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::relay::Relay;
use super::*;
use crate::chat::{self, ChatEvent, Origin, SendRequest, Server};
use crate::store::chats::{self, AttachmentInfo};
use crate::store::settings::{self, SettingsPatch};

/// Action d'un essai au prochain accès au moteur de chat.
type Hook = Box<dyn FnOnce() + Send>;

/// L'app, pour les essais : son moteur de chat, et ce qu'elle a annoncé.
struct TestHost {
    engine: ChatEngine,
    states: Arc<Mutex<Vec<EasyLinkState>>>,
    updated: Arc<Mutex<Vec<String>>>,
    /// Ce que rend la transcription d'un message vocal.
    heard: Arc<Mutex<Result<String, HearError>>>,
    /// Faux : la transcription attend (un essai agit pendant la préparation).
    gate: Arc<tokio::sync::watch::Sender<bool>>,
    /// Lancé une fois, au prochain accès au moteur (un essai agit juste avant
    /// le départ d'un envoi).
    on_engine: Arc<Mutex<Option<Hook>>>,
    /// Lancé une fois, à la prochaine annonce de l'état (un essai y retient
    /// la tâche qui l'annonce).
    on_changed: Arc<Mutex<Option<Hook>>>,
}

impl Host for TestHost {
    fn engine(&self) -> &ChatEngine {
        // Hors du verrou : l'action peut elle-même passer par le moteur.
        let hook = self.on_engine.lock().take();
        if let Some(hook) = hook {
            hook();
        }
        &self.engine
    }

    fn changed(&self, state: &EasyLinkState) {
        let hook = self.on_changed.lock().take();
        if let Some(hook) = hook {
            hook();
        }
        self.states.lock().push(state.clone());
    }

    fn chat_updated(&self, chat_id: &str) {
        self.updated.lock().push(chat_id.to_owned());
    }

    async fn transcribe(&self, _samples: Vec<f32>) -> Result<String, HearError> {
        let mut open = self.gate.subscribe();
        let _ = open.wait_for(|open| *open).await;
        self.heard.lock().clone()
    }
}

struct Bench {
    ollama: FakeOllama,
    telegram: FakeTelegram,
    discord: FakeDiscord,
    store: Store,
    service: EasyLink<TestHost>,
    updated: Arc<Mutex<Vec<String>>>,
    heard: Arc<Mutex<Result<String, HearError>>>,
    gate: Arc<tokio::sync::watch::Sender<bool>>,
    on_engine: Arc<Mutex<Option<Hook>>>,
    on_changed: Arc<Mutex<Option<Hook>>>,
    /// Événements vus par l'observateur du moteur de chat.
    observed: Arc<Mutex<usize>>,
}

const ANA: &str = "1001";

/// App dont le modèle choisi est `model`, sans bot relié.
async fn bench(model: &str) -> Bench {
    let ollama = FakeOllama::start(Mode::ollama()).await.unwrap();
    let telegram = FakeTelegram::start().await;
    let discord = FakeDiscord::start().await;
    let store = Store::open_in_memory().unwrap();
    let patch = SettingsPatch {
        server_url: Some(ollama.url()),
        selected_model: Some(model.to_owned()),
        ..Default::default()
    };
    store
        .blocking(|c| settings::update(c, patch, None).map(drop))
        .unwrap();
    let engine = ChatEngine::default();
    let observed = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&observed);
    engine.set_observer(move |_, _: &ChatEvent, _| *seen.lock() += 1);
    let host = TestHost {
        engine,
        states: Arc::default(),
        updated: Arc::default(),
        heard: Arc::new(Mutex::new(Err(HearError::NoModel))),
        gate: Arc::new(tokio::sync::watch::channel(true).0),
        on_engine: Arc::default(),
        on_changed: Arc::default(),
    };
    let (updated, heard) = (Arc::clone(&host.updated), Arc::clone(&host.heard));
    let (gate, on_engine) = (Arc::clone(&host.gate), Arc::clone(&host.on_engine));
    let on_changed = Arc::clone(&host.on_changed);
    let urls = Urls {
        telegram: telegram.url(),
        discord: discord.url(),
    };
    Bench {
        service: EasyLink::new(host, store.clone(), urls),
        ollama,
        telegram,
        discord,
        store,
        updated,
        heard,
        gate,
        on_engine,
        on_changed,
        observed,
    }
}

async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "jamais : {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Textes que la messagerie montre : dernier texte de chaque message du bot,
/// dans l'ordre d'envoi.
fn shown(sent: &[Value]) -> Vec<String> {
    let mut messages: Vec<(String, String)> = Vec::new();
    for action in sent {
        let (Some(id), Some(text)) = (action["id"].as_str(), action["text"].as_str()) else {
            continue;
        };
        match action["kind"].as_str() {
            Some("send") => messages.push((id.to_owned(), text.to_owned())),
            Some("edit") => {
                if let Some(message) = messages.iter_mut().find(|(known, _)| known == id) {
                    message.1 = text.to_owned();
                }
            }
            _ => {}
        }
    }
    messages.into_iter().map(|(_, text)| text).collect()
}

impl Bench {
    /// État montré, lu sans `await` (pour les conditions de `wait_until`).
    fn state(&self) -> EasyLinkState {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.service.state())
        })
        .unwrap()
    }

    fn link_of(&self, platform: Platform) -> EasyLinkLink {
        let state = self.state();
        state
            .bots
            .iter()
            .find(|bot| bot.platform == platform)
            .unwrap()
            .link
    }

    /// Relie le bot Telegram et lie le compte `ANA` par le code.
    async fn telegram_linked(&self) {
        self.service
            .connect(Platform::Telegram, fake_telegram::TOKEN)
            .await
            .unwrap();
        wait_until("Telegram connecté", || {
            self.link_of(Platform::Telegram) == EasyLinkLink::Connected
        })
        .await;
        let state = self.service.pair(Platform::Telegram).await.unwrap();
        let code = state.pairing.unwrap().code;
        self.say(&format!("/start {code}"));
        wait_until("compte lié", || self.state().accounts.len() == 1).await;
        wait_until("accueil envoyé", || !self.telegram.sent().is_empty()).await;
    }

    /// Message privé d'`ANA` au bot Telegram.
    fn say(&self, text: &str) {
        self.telegram
            .message(json!({ "user": ANA, "name": "Ana", "text": text }));
    }

    fn told(&self) -> Vec<String> {
        shown(&self.telegram.sent())
    }

    /// Attend qu'un message du bot contienne `part`.
    async fn wait_told(&self, part: &str) {
        wait_until(&format!("le bot dit « {part} »"), || {
            self.told().iter().any(|text| text.contains(part))
        })
        .await;
    }

    /// Photo sans légende d'`ANA` au bot Telegram.
    fn photo(&self) {
        self.telegram
            .message(json!({ "user": ANA, "name": "Ana", "text": "", "image": true }));
    }

    /// Messages du bot qui contiennent `part`.
    fn told_count(&self, part: &str) -> usize {
        self.told()
            .iter()
            .filter(|text| text.contains(part))
            .count()
    }

    /// Requêtes reçues par le faux Telegram pour ces méthodes.
    fn telegram_calls(&self, methods: &[&str]) -> usize {
        self.telegram
            .requests()
            .iter()
            .filter(|request| {
                request["method"]
                    .as_str()
                    .is_some_and(|method| methods.contains(&method))
            })
            .count()
    }

    /// Plus grand `offset` envoyé par `getUpdates` : ce que Telegram tient
    /// pour acquitté.
    fn acknowledged(&self) -> i64 {
        self.telegram
            .requests()
            .iter()
            .filter(|request| request["method"] == "getUpdates")
            .filter_map(|request| request["body"]["offset"].as_i64())
            .max()
            .unwrap_or(0)
    }

    /// Retient la tâche qui relaie les événements de l'écoute en cours (à sa
    /// prochaine annonce, une coupure provoquée ici), puis fait arriver
    /// `message` : l'écoute le reçoit et l'acquitte, l'app ne l'a pas encore
    /// vu. Rend de quoi relâcher la tâche.
    async fn message_in_flight(&self, message: Value) -> std::sync::mpsc::Sender<()> {
        let (release, held) = std::sync::mpsc::channel::<()>();
        *self.on_changed.lock() = Some(Box::new(move || {
            tokio::task::block_in_place(|| {
                let _ = held.recv_timeout(Duration::from_secs(20));
            });
        }));
        // L'acquittement de `/help` échoue (502) : l'écoute annonce la
        // coupure, puis reprend.
        self.telegram.fail_next(
            "getUpdates",
            502,
            json!({ "ok": false, "error_code": 502, "description": "Bad Gateway" }),
        );
        self.say("/help");
        wait_until("relais retenu", || self.on_changed.lock().is_none()).await;
        let before = self.acknowledged();
        self.telegram.message(message);
        wait_until("message acquitté", || self.acknowledged() > before).await;
        release
    }

    /// Relie de nouveau le bot Telegram, du même jeton (jeton recollé).
    async fn relink(&self) {
        self.service
            .connect(Platform::Telegram, fake_telegram::TOKEN)
            .await
            .unwrap();
        wait_until("nouvelle écoute connectée", || {
            self.link_of(Platform::Telegram) == EasyLinkLink::Connected
        })
        .await;
    }

    fn account(&self) -> crate::store::easy_link::Account {
        self.store
            .blocking(|c| saved::account(c, Platform::Telegram, ANA))
            .unwrap()
            .expect("compte lié")
    }

    fn busy(&self) -> bool {
        !self.service.shared().busy.lock().is_empty()
    }

    /// Attend la fin de la réponse en cours.
    async fn wait_answered(&self, requests: usize) {
        wait_until("requête au modèle", || {
            self.ollama.chat_requests().len() >= requests
        })
        .await;
        wait_until("réponse finie", || !self.busy()).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_is_checked_saved_and_never_returned() {
    let bench = bench("qwen3:0.6b").await;
    let refused = bench
        .service
        .connect(Platform::Telegram, "1:wrong-token-wrong-token")
        .await;
    assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
    assert!(!bench.state().bots[0].configured);
    assert!(matches!(
        bench.service.connect(Platform::Telegram, "  ").await,
        Err(AppError::Invalid(_))
    ));

    let state = bench
        .service
        .connect(Platform::Telegram, &format!("  {}\n", fake_telegram::TOKEN))
        .await
        .unwrap();
    let bot = &state.bots[0];
    assert!(bot.configured && bot.enabled);
    assert_eq!(bot.name, "chatonlm_test_bot");
    assert_eq!(bot.url.as_deref(), Some("https://t.me/chatonlm_test_bot"));
    let token = fake_telegram::TOKEN;
    assert_eq!(bot.token_hint, &token[token.len() - 4..]);
    // Rien de ce qui part vers la page ne porte le jeton.
    let page = serde_json::to_string(&state).unwrap();
    assert!(!page.contains(token), "{page}");
    wait_until("connecté", || {
        bench.link_of(Platform::Telegram) == EasyLinkLink::Connected
    })
    .await;
    // Discord n'a pas bougé.
    assert_eq!(bench.link_of(Platform::Discord), EasyLinkLink::Off);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_gets_one_notice_and_never_reaches_the_model() {
    let bench = bench("qwen3:0.6b").await;
    bench
        .service
        .connect(Platform::Telegram, fake_telegram::TOKEN)
        .await
        .unwrap();
    bench.say("hello there");
    bench.say("/models");
    bench.say("answer me");
    bench.wait_told("This bot is private").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Un seul avis, aucune commande servie, aucune conversation, rien envoyé
    // à Ollama.
    assert_eq!(bench.told().len(), 1, "{:?}", bench.told());
    assert!(bench.ollama.chat_requests().is_empty());
    assert!(bench.store.blocking(|c| chats::list(c)).unwrap().is_empty());
    assert!(bench.state().accounts.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pairing_code_links_an_account_once() {
    let bench = bench("qwen3:0.6b").await;
    // Pas de code sans bot.
    assert!(bench.service.pair(Platform::Telegram).await.is_err());
    bench
        .service
        .connect(Platform::Telegram, fake_telegram::TOKEN)
        .await
        .unwrap();
    let state = bench.service.pair(Platform::Telegram).await.unwrap();
    let pairing = state.pairing.unwrap();
    assert_eq!(pairing.platform, Platform::Telegram);
    assert_eq!(
        pairing.url.as_deref(),
        Some(format!("https://t.me/chatonlm_test_bot?start={}", pairing.code).as_str())
    );
    // Un mauvais code ne lie pas.
    let wrong = if pairing.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    bench.say(wrong);
    bench.wait_told("This bot is private").await;
    assert!(bench.state().accounts.is_empty());

    bench.say(&format!("/start {}", pairing.code));
    wait_until("compte lié", || bench.state().accounts.len() == 1).await;
    let state = bench.state();
    assert_eq!(state.accounts[0].name, "Ana");
    assert_eq!(state.accounts[0].platform, Platform::Telegram);
    assert_eq!(state.pairing, None, "le code est consommé");
    bench.wait_told("Your account is linked").await;
    // L'accueil donne les commandes, avec le préfixe de Telegram.
    assert!(bench.told().iter().any(|text| text.contains("/models")));
    // La page a été prévenue.
    wait_until("page prévenue", || {
        bench
            .service
            .shared()
            .host
            .states
            .lock()
            .iter()
            .any(|state| state.accounts.len() == 1 && state.pairing.is_none())
    })
    .await;

    // Le même code ne lie pas un second compte.
    bench.telegram.message(json!({
        "user": "2002", "name": "Eve", "text": pairing.code
    }));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(bench.state().accounts.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linked_account_talks_to_the_model_in_a_marked_conversation() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    bench.say("hello");
    bench.wait_answered(1).await;

    let listed = bench.store.blocking(|c| chats::list(c)).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source, Some(chats::ChatSource::Telegram));
    assert_eq!(listed[0].user_excerpt, "hello");
    let chat_id = listed[0].id.clone();
    assert_eq!(bench.account().chat_id.as_deref(), Some(chat_id.as_str()));
    let chat = bench.store.blocking(|c| chats::get(c, &chat_id)).unwrap();
    assert_eq!(chat.messages.len(), 2);
    let answer = chat.messages[1].content.clone();
    assert!(!answer.trim().is_empty());
    // La messagerie montre la réponse enregistrée (un seul message).
    let told = bench.told();
    assert_eq!(told.len(), before + 1, "{told:?}");
    let first_word = answer.split_whitespace().next().unwrap();
    assert!(told[before].contains(first_word), "{told:?}");
    // La page est prévenue de la conversation.
    assert!(bench.updated.lock().contains(&chat_id));
    // « écrit… » a été montré avant la réponse.
    assert!(
        bench
            .telegram
            .sent()
            .iter()
            .any(|action| action["kind"] == "typing")
    );
    // Réponse destinée à la messagerie : l'observateur (lecture à voix haute)
    // ne la voit pas.
    assert_eq!(*bench.observed.lock(), 0);

    // Le message suivant continue la même conversation.
    bench.say("and then?");
    bench.wait_answered(2).await;
    let requests = bench.ollama.chat_requests();
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 3);
    assert_eq!(bench.store.blocking(|c| chats::list(c)).unwrap().len(), 1);

    // `/new` : la suivante en ouvre une autre.
    bench.say("/new");
    bench.wait_told("New conversation").await;
    assert_eq!(bench.account().chat_id, None);
    bench.say("fresh start");
    bench.wait_answered(3).await;
    let requests = bench.ollama.chat_requests();
    assert_eq!(requests[2]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(bench.store.blocking(|c| chats::list(c)).unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_interrupts_the_answer_and_a_second_message_waits() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    bench.say("write slowly");
    wait_until("réponse commencée", || bench.told().len() > before).await;
    assert!(bench.busy());

    // Un second message pendant la réponse n'est pas envoyé au modèle.
    bench.say("are you there?");
    bench.wait_told("still answering").await;
    assert_eq!(bench.ollama.chat_requests().len(), 1);

    // Un rechargement de la page n'arrête pas la réponse d'un bot.
    bench.service.shared().host.engine.cancel_page();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(bench.busy(), "arrêtée par le rechargement de la page");

    bench.say("/stop");
    bench.wait_told("Stopped.").await;
    wait_until("réponse arrêtée", || !bench.busy()).await;
    // La partie reçue reste dans l'historique.
    let listed = bench.store.blocking(|c| chats::list(c)).unwrap();
    let chat = bench
        .store
        .blocking(|c| chats::get(c, &listed[0].id))
        .unwrap();
    assert_eq!(chat.messages.len(), 2);
    assert!(!chat.messages[1].content.is_empty());

    bench.say("/stop");
    bench.wait_told("No answer is being written").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn models_are_listed_chosen_loaded_and_unloaded_from_the_bot() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;

    bench.say("/models");
    bench.wait_told("Installed models").await;
    let list = bench
        .told()
        .into_iter()
        .find(|text| text.contains("Installed models"))
        .unwrap();
    // Triés par nom : les numéros ne changent pas d'un appel à l'autre.
    let (first, second) = (
        list.find("qwen3.5:0.8b").unwrap(),
        list.find("qwen3:0.6b").unwrap(),
    );
    assert!(list.find("1. ").unwrap() < first && first < list.find("2. ").unwrap());
    assert!(list.find("2. ").unwrap() < second, "{list}");
    assert!(list.contains("chosen"), "{list}");

    // Un modèle qui n'est pas installé ne se choisit pas (rien n'est
    // téléchargé depuis un bot).
    bench.say("/model llama-not-there");
    bench.wait_told("No installed model matches").await;
    assert_eq!(bench.account().model, "");

    bench.say("/model 1");
    wait_until("modèle choisi", || bench.account().model == "qwen3.5:0.8b").await;

    bench.say("/load");
    bench.wait_told("is in memory").await;
    let loads = bench.ollama.load_requests();
    assert!(
        loads.iter().any(|load| load["model"] == "qwen3.5:0.8b"),
        "{loads:?}"
    );

    bench.say("/status");
    bench.wait_told("In memory:").await;
    bench.wait_told("Web search: off").await;

    bench.say("/unload");
    bench.wait_told("is unloaded").await;

    // Le message suivant part au modèle choisi par le compte, pas à celui de
    // l'app.
    bench.say("hello");
    bench.wait_answered(1).await;
    assert_eq!(bench.ollama.chat_requests()[0]["model"], "qwen3.5:0.8b");

    bench.say("/frobnicate");
    bench.wait_told("Unknown command").await;
    assert!(bench.ollama.pull_requests().is_empty());
    assert!(bench.ollama.delete_requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_photo_goes_to_a_model_that_sees() {
    let bench = bench("chatonlm-vision").await;
    bench.telegram_linked().await;

    // Photo sans légende : gardée pour le message suivant.
    bench
        .telegram
        .message(json!({ "user": ANA, "name": "Ana", "text": "", "image": true }));
    bench.wait_told("Photo received").await;
    assert!(bench.ollama.chat_requests().is_empty());
    bench.say("what is this?");
    bench.wait_answered(1).await;
    let request = &bench.ollama.chat_requests()[0];
    let message = &request["messages"][0];
    assert_eq!(message["content"], "what is this?");
    assert_eq!(message["images"].as_array().map(Vec::len), Some(1));

    // Photo avec légende : envoyée tout de suite.
    bench.telegram.message(json!({
        "user": ANA, "name": "Ana", "text": "and this one?", "image": true
    }));
    bench.wait_answered(2).await;
    let request = &bench.ollama.chat_requests()[1];
    let last = request["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["content"], "and this one?");
    assert_eq!(last["images"].as_array().map(Vec::len), Some(1));
    // Rien ne reste dans la zone de préparation des bots.
    assert!(bench.service.shared().pending.lock().is_empty());
}

/// Modèle cloud choisi dans l'app, réglage coupé : le bot le dit, et rien
/// ne part (ni détails, ni message), photo comprise.
#[tokio::test(flavor = "multi_thread")]
async fn a_cloud_model_is_refused_while_turned_off() {
    let bench = bench("kimi-k2.6:cloud").await;
    bench.telegram_linked().await;
    bench.say("hello");
    bench.wait_told("Cloud models are turned off").await;
    wait_until("libre", || !bench.busy()).await;
    bench.telegram.message(json!({
        "user": ANA, "name": "Ana", "text": "what is this?", "image": true
    }));
    wait_until("photo refusée", || {
        bench.told_count("Cloud models are turned off") == 2
    })
    .await;
    assert!(bench.ollama.chat_requests().is_empty());
    assert!(bench.ollama.show_requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_photo_is_refused_by_a_model_that_cannot_see() {
    let bench = bench("chatonlm-search").await;
    bench.telegram_linked().await;
    bench.telegram.message(json!({
        "user": ANA, "name": "Ana", "text": "what is this?", "image": true
    }));
    bench.wait_told("cannot read images").await;
    wait_until("libre", || !bench.busy()).await;
    assert!(bench.ollama.chat_requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_message_is_transcribed_then_sent() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;

    // Aucun modèle de transcription : le bot dit quoi installer.
    bench
        .telegram
        .message(json!({ "user": ANA, "name": "Ana", "text": "", "voice": true }));
    bench.wait_told("install a speech model").await;
    wait_until("libre", || !bench.busy()).await;
    assert!(bench.ollama.chat_requests().is_empty());

    *bench.heard.lock() = Ok("what time is it".into());
    bench
        .telegram
        .message(json!({ "user": ANA, "name": "Ana", "text": "", "voice": true }));
    // Ce qui a été compris est redit, puis envoyé au modèle.
    bench.wait_told("🎤 what time is it").await;
    bench.wait_answered(1).await;
    assert_eq!(
        bench.ollama.chat_requests()[0]["messages"][0]["content"],
        "what time is it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn web_search_follows_the_account() {
    let bench = bench("chatonlm-search").await;
    bench.telegram_linked().await;
    bench.say("no search please");
    bench.wait_answered(1).await;
    assert!(bench.ollama.chat_requests()[0].get("tools").is_none());

    bench.ollama.set_signed_in(true);
    bench.say("/web on");
    bench.wait_told("Web search is on").await;
    assert!(bench.account().web_search);
    bench.say("/new");
    bench.wait_told("New conversation").await;
    let before = bench.ollama.chat_requests().len();
    bench.say("search the web");
    wait_until("requête avec outils", || {
        bench.ollama.chat_requests().len() > before
    })
    .await;
    wait_until("réponse finie", || !bench.busy()).await;
    assert!(bench.ollama.chat_requests()[before].get("tools").is_some());

    bench.say("/web maybe");
    bench.wait_told("/web on or /web off").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_delivered_long_after_is_not_served() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    // Remis dix minutes après leur envoi (l'app était fermée).
    for text in ["old question", "are you there?", "/models"] {
        bench.telegram.message(json!({
            "user": ANA, "name": "Ana", "text": text, "age": 600
        }));
    }
    // Le compte lié en est prévenu, une seule fois ; rien ne part au modèle
    // et aucune commande n'est servie.
    bench.wait_told("was not running when you sent").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(bench.told().len(), before + 1, "{:?}", bench.told());
    assert!(bench.ollama.chat_requests().is_empty());
}

/// L'horloge du PC avance : le code d'appairage paraît envoyé il y a dix
/// minutes, il lie quand même le compte (l'ancienneté ne vaut que pour un
/// compte lié).
#[tokio::test(flavor = "multi_thread")]
async fn a_pairing_code_that_looks_old_still_links_the_account() {
    let bench = bench("qwen3:0.6b").await;
    bench
        .service
        .connect(Platform::Telegram, fake_telegram::TOKEN)
        .await
        .unwrap();
    let state = bench.service.pair(Platform::Telegram).await.unwrap();
    let code = state.pairing.unwrap().code;
    bench.telegram.message(json!({
        "user": ANA, "name": "Ana", "text": format!("/start {code}"), "age": 600
    }));
    wait_until("compte lié", || bench.state().accounts.len() == 1).await;
    bench.wait_told("Your account is linked").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unlinking_and_removing_the_bot_end_the_service() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    let account = bench.state().accounts[0].id;
    let state = bench.service.unlink(account).await.unwrap();
    assert!(state.accounts.is_empty());
    // Délié : de nouveau un inconnu.
    bench.say("hello");
    bench.wait_told("This bot is private").await;
    assert!(bench.ollama.chat_requests().is_empty());

    // Bot désactivé : plus d'écoute, plus de code.
    let state = bench
        .service
        .set_enabled(Platform::Telegram, false)
        .await
        .unwrap();
    assert_eq!(state.bots[0].link, EasyLinkLink::Off);
    assert!(state.bots[0].configured && !state.bots[0].enabled);
    assert!(bench.service.pair(Platform::Telegram).await.is_err());
    let state = bench
        .service
        .set_enabled(Platform::Telegram, true)
        .await
        .unwrap();
    assert_ne!(state.bots[0].link, EasyLinkLink::Off);

    // Bot retiré : jeton oublié, comptes déliés.
    bench.service.pair(Platform::Telegram).await.unwrap();
    let state = bench.service.disconnect(Platform::Telegram).await.unwrap();
    assert!(!state.bots[0].configured && !state.bots[0].enabled);
    assert_eq!(state.bots[0].name, "");
    assert_eq!(state.bots[0].link, EasyLinkLink::Off);
    assert_eq!(state.pairing, None);
    let bot = bench
        .store
        .blocking(|c| saved::bot(c, Platform::Telegram))
        .unwrap();
    assert!(bot.token.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_token_shows_why_the_bot_is_off() {
    let bench = bench("qwen3:0.6b").await;
    // Jeton enregistré puis révoqué chez la messagerie (base d'un lancement
    // précédent).
    bench
        .store
        .blocking(|c| saved::save_bot(c, Platform::Telegram, "1:revoked-token-revoked", "1", "old"))
        .unwrap();
    bench.service.start_saved().await;
    wait_until("jeton refusé montré", || {
        bench.link_of(Platform::Telegram)
            == EasyLinkLink::Failed {
                problem: EasyLinkProblem::Unauthorized,
            }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn discord_links_by_private_message_and_splits_a_long_answer() {
    let bench = bench("chatonlm-markdown-long").await;
    let state = bench
        .service
        .connect(Platform::Discord, fake_discord::TOKEN)
        .await
        .unwrap();
    let bot = &state.bots[1];
    assert_eq!(bot.platform, Platform::Discord);
    assert!(bot.configured);
    assert!(
        bot.url
            .as_deref()
            .is_some_and(|url| url.contains("client_id=") && url.contains("scope=bot")),
        "{:?}",
        bot.url
    );
    wait_until("Discord connecté", || {
        bench.link_of(Platform::Discord) == EasyLinkLink::Connected
    })
    .await;
    let state = bench.service.pair(Platform::Discord).await.unwrap();
    let pairing = state.pairing.unwrap();
    assert_eq!(pairing.url, None);
    let dm = |text: &str| {
        bench
            .discord
            .message(json!({ "user": "3003", "name": "Luis", "text": text }));
    };
    let told = || shown(&bench.discord.sent());

    // Un message d'un serveur ne lie personne, même avec le bon code.
    bench.discord.message(json!({
        "user": "3003", "name": "Luis", "text": pairing.code, "private": false
    }));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(bench.state().accounts.is_empty());

    dm(&pairing.code);
    wait_until("compte Discord lié", || bench.state().accounts.len() == 1).await;
    wait_until("accueil", || !told().is_empty()).await;
    // Les commandes sont données avec le préfixe de Discord.
    assert!(told()[0].contains("!models"), "{:?}", told());

    let before = told().len();
    dm("write a long answer");
    wait_until("requête au modèle", || {
        !bench.ollama.chat_requests().is_empty()
    })
    .await;
    wait_until("réponse finie", || !bench.busy()).await;
    let listed = bench.store.blocking(|c| chats::list(c)).unwrap();
    assert_eq!(listed[0].source, Some(chats::ChatSource::Discord));
    let chat = bench
        .store
        .blocking(|c| chats::get(c, &listed[0].id))
        .unwrap();
    let answer = chat.messages[1].content.clone();
    let parts = told()[before..].to_vec();
    assert!(parts.len() > 1, "réponse en un seul message");
    for part in &parts {
        assert!(part.encode_utf16().count() <= 2000, "message trop long");
    }
    // Tout le texte de la réponse est dans les messages, dans l'ordre.
    let words = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .filter(|word| !word.starts_with("```"))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(words(&parts.join("\n")), words(&answer));
}

/// Un refus « trop de requêtes » pendant l'écriture ne perd rien : les
/// modifications suivantes attendent, et la dernière écriture insiste.
#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limit_while_writing_loses_nothing() {
    let bench = bench("chatonlm-markdown-long").await;
    bench
        .service
        .connect(Platform::Discord, fake_discord::TOKEN)
        .await
        .unwrap();
    let state = bench.service.pair(Platform::Discord).await.unwrap();
    let code = state.pairing.unwrap().code;
    let dm = |text: &str| {
        bench
            .discord
            .message(json!({ "user": "3003", "name": "Luis", "text": text }));
    };
    dm(&code);
    wait_until("compte Discord lié", || bench.state().accounts.len() == 1).await;
    wait_until("accueil", || !bench.discord.sent().is_empty()).await;
    let before = shown(&bench.discord.sent()).len();
    for _ in 0..2 {
        bench.discord.fail_next(
            "edit",
            429,
            json!({ "message": "You are being rate limited.", "retry_after": 0.8, "global": false }),
        );
    }
    dm("write a long answer");
    wait_until("requête au modèle", || {
        !bench.ollama.chat_requests().is_empty()
    })
    .await;
    wait_until("réponse finie", || !bench.busy()).await;
    let listed = bench.store.blocking(|c| chats::list(c)).unwrap();
    let chat = bench
        .store
        .blocking(|c| chats::get(c, &listed[0].id))
        .unwrap();
    let words = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .filter(|word| !word.starts_with("```"))
            .map(str::to_owned)
            .collect()
    };
    let parts = shown(&bench.discord.sent())[before..].to_vec();
    assert_eq!(words(&parts.join("\n")), words(&chat.messages[1].content));
}

/// Un arrêt demandé pendant la préparation (ici la transcription d'un message
/// vocal, avant que le moteur de chat ne connaisse le flux) n'est pas perdu.
#[tokio::test(flavor = "multi_thread")]
async fn stop_or_unlink_during_the_transcription_sends_nothing_to_the_model() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    *bench.heard.lock() = Ok("sent by mistake".into());
    let voice = || {
        bench
            .telegram
            .message(json!({ "user": ANA, "name": "Ana", "text": "", "voice": true }));
    };

    bench.gate.send_replace(false);
    voice();
    wait_until("préparation en cours", || bench.busy()).await;
    bench.say("/stop");
    bench.wait_told("Stopped.").await;
    bench.gate.send_replace(true);
    wait_until("libre", || !bench.busy()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(bench.ollama.chat_requests().is_empty());
    assert!(bench.store.blocking(|c| chats::list(c)).unwrap().is_empty());

    // Même chose quand le compte est délié dans l'app pendant la préparation.
    bench.gate.send_replace(false);
    voice();
    wait_until("préparation en cours", || bench.busy()).await;
    let account = bench.state().accounts[0].id;
    bench.service.unlink(account).await.unwrap();
    bench.gate.send_replace(true);
    wait_until("libre", || !bench.busy()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(bench.ollama.chat_requests().is_empty());
    assert!(bench.store.blocking(|c| chats::list(c)).unwrap().is_empty());
}

/// La fermeture de l'app pendant la préparation (ici la transcription d'un
/// message vocal) n'envoie rien au modèle.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_app_during_the_transcription_sends_nothing_to_the_model() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    *bench.heard.lock() = Ok("sent while closing".into());
    bench.gate.send_replace(false);
    bench
        .telegram
        .message(json!({ "user": ANA, "name": "Ana", "text": "", "voice": true }));
    wait_until("préparation en cours", || bench.busy()).await;
    bench.service.shutdown();
    bench.gate.send_replace(true);
    wait_until("libre", || !bench.busy()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(bench.ollama.chat_requests().is_empty());
    assert!(bench.store.blocking(|c| chats::list(c)).unwrap().is_empty());
}

/// Un arrêt arrivé entre la relecture du compte et le départ de l'envoi (le
/// moteur de chat ne connaît pas encore le flux) n'est pas perdu : rien ne
/// part au modèle, rien n'est écrit dans la messagerie.
#[tokio::test(flavor = "multi_thread")]
async fn stop_just_before_the_sending_starts_sends_nothing_to_the_model() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    let account = bench.state().accounts[0].id;
    // Premier accès au moteur : celui de l'envoi, avant `begin`. L'arrêt y
    // est demandé, puis l'envoi retenu un instant : le premier tic de la
    // réponse passe avant que le moteur ne connaisse le flux.
    let service = bench.service.clone();
    *bench.on_engine.lock() = Some(Box::new(move || {
        service.shared().stop_answer(account);
        tokio::task::block_in_place(|| std::thread::sleep(Duration::from_millis(100)));
    }));
    bench.say("write slowly");
    wait_until("arrêt demandé", || bench.on_engine.lock().is_none()).await;
    wait_until("libre", || !bench.busy()).await;
    assert!(bench.ollama.chat_requests().is_empty());
    assert_eq!(bench.told().len(), before, "{:?}", bench.told());
}

/// Délier un compte arrête sa réponse en cours.
#[tokio::test(flavor = "multi_thread")]
async fn unlinking_stops_the_answer_being_written() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    bench.say("write slowly");
    wait_until("réponse commencée", || bench.told().len() > before).await;
    let account = bench.state().accounts[0].id;
    let started = std::time::Instant::now();
    bench.service.unlink(account).await.unwrap();
    wait_until("réponse arrêtée", || !bench.busy()).await;
    // Bien avant la fin naturelle de la réponse (8 s environ).
    assert!(started.elapsed() < Duration::from_secs(4));
}

/// Réponse coupée parce que la conversation remplit le contexte : le
/// correspondant, qui ne peut pas compacter, est invité à renvoyer son
/// message (l'envoi suivant compactera de lui-même).
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_cut_by_a_full_context_asks_to_send_again() {
    let bench = bench("chatonlm-context").await;
    bench.telegram_linked().await;
    // 15 000 caractères : 3 750 jetons sur 4 096, la réponse n'a plus la
    // place de ses 600 jetons.
    bench.say(&"Tell me everything. ".repeat(750));
    bench
        .wait_told("the conversation will be summarized first")
        .await;
    wait_until("libre", || !bench.busy()).await;
}

/// Un modèle supprimé depuis son choix n'est pas retéléchargé par un message
/// du bot (le moteur de chat, lui, télécharge un modèle absent).
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_model_is_not_downloaded_from_the_bot() {
    let bench = bench("chatonlm-pull").await;
    bench.telegram_linked().await;
    bench.say("hello");
    bench.wait_told("is not installed anymore").await;
    wait_until("libre", || !bench.busy()).await;
    assert!(bench.ollama.pull_requests().is_empty());
    assert!(bench.ollama.chat_requests().is_empty());
    assert!(bench.store.blocking(|c| chats::list(c)).unwrap().is_empty());
}

/// Un album de Telegram arrive en plusieurs messages, la légende sur le
/// premier : ses photos partent ensemble, sans invite « photo reçue ».
#[tokio::test(flavor = "multi_thread")]
async fn an_album_with_a_caption_goes_to_the_model_whole() {
    let bench = bench("chatonlm-vision").await;
    bench.telegram_linked().await;
    bench.telegram.message(json!({
        "user": ANA, "name": "Ana", "text": "what are these?", "image": true
    }));
    for _ in 0..2 {
        bench
            .telegram
            .message(json!({ "user": ANA, "name": "Ana", "text": "", "image": true }));
    }
    bench.wait_answered(1).await;
    let request = &bench.ollama.chat_requests()[0];
    assert_eq!(
        request["messages"][0]["images"].as_array().map(Vec::len),
        Some(3)
    );
    assert!(
        !bench
            .told()
            .iter()
            .any(|text| text.contains("Photo received")),
        "{:?}",
        bench.told()
    );
    assert!(bench.service.shared().pending.lock().is_empty());
}

/// Une photo sans légende arrivée pendant une réponse, après la jointure des
/// photos à son message, est annoncée à la fin de la réponse, puis jointe au
/// message suivant.
#[tokio::test(flavor = "multi_thread")]
async fn a_photo_arriving_during_an_answer_is_announced_when_it_ends() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    bench.say("write slowly");
    wait_until("réponse commencée", || bench.told().len() > before).await;
    // Le faux Ollama n'a pas de modèle lent qui voie : le compte passe à un
    // modèle qui voit pendant la réponse, pour que la photo soit gardée.
    let account = bench.account().id;
    bench
        .store
        .blocking(move |c| saved::set_model(c, account, "chatonlm-vision"))
        .unwrap();
    bench
        .telegram
        .message(json!({ "user": ANA, "name": "Ana", "text": "", "image": true }));
    wait_until("photo gardée", || {
        !bench.service.shared().pending.lock().is_empty()
    })
    .await;
    assert!(bench.busy(), "réponse finie avant l'arrivée de la photo");
    bench.wait_told("Photo received").await;
    assert!(!bench.busy());

    bench.say("what is this?");
    bench.wait_answered(2).await;
    let request = &bench.ollama.chat_requests()[1];
    let last = request["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["content"], "what is this?");
    assert_eq!(last["images"].as_array().map(Vec::len), Some(1));
}

/// La dernière écriture échoue (ici, chaque modification est refusée) : le
/// bot dit que la réponse est incomplète dans la messagerie.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_last_write_is_told() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench
        .service
        .connect(Platform::Discord, fake_discord::TOKEN)
        .await
        .unwrap();
    let state = bench.service.pair(Platform::Discord).await.unwrap();
    let code = state.pairing.unwrap().code;
    let dm = |text: &str| {
        bench
            .discord
            .message(json!({ "user": "3003", "name": "Luis", "text": text }));
    };
    dm(&code);
    wait_until("compte Discord lié", || bench.state().accounts.len() == 1).await;
    for _ in 0..40 {
        bench.discord.fail_next(
            "edit",
            400,
            json!({ "message": "Unknown Message", "code": 10008 }),
        );
    }
    dm("write slowly");
    wait_until("requête au modèle", || {
        !bench.ollama.chat_requests().is_empty()
    })
    .await;
    wait_until("réponse finie", || !bench.busy()).await;
    let told = shown(&bench.discord.sent());
    assert!(
        told.iter()
            .any(|text| text.contains("could not be written here in full")),
        "{told:?}"
    );
}

/// Une écoute remplacée (le même bot relié de nouveau) peut encore avoir des
/// annonces en route : elles n'écrivent plus rien, ni la liaison montrée, ni
/// le nom du bot.
#[tokio::test(flavor = "multi_thread")]
async fn the_announcements_of_a_replaced_listener_change_nothing() {
    let bench = bench("qwen3:0.6b").await;
    // Première écoute : le bot porte encore son ancien nom ; sa réception
    // marche, tombe, puis revient.
    let old_me = json!({
        "ok": true,
        "result": {
            "id": 424_242, "is_bot": true, "first_name": "ChatonLM test",
            "username": "chatonlm_old_bot"
        }
    });
    for _ in 0..2 {
        bench.telegram.fail_next("getMe", 200, old_me.clone());
    }
    bench
        .telegram
        .fail_next("getUpdates", 200, json!({ "ok": true, "result": [] }));
    bench.telegram.fail_next(
        "getUpdates",
        502,
        json!({ "ok": false, "error_code": 502, "description": "Bad Gateway" }),
    );
    // Sa première annonce (connectée) reste en route : les deux suivantes
    // (coupée, connectée) attendent derrière elle, dans l'app.
    let (release, held) = std::sync::mpsc::channel::<()>();
    *bench.on_changed.lock() = Some(Box::new(move || {
        tokio::task::block_in_place(|| {
            let _ = held.recv_timeout(Duration::from_secs(20));
        });
    }));
    bench
        .service
        .connect(Platform::Telegram, fake_telegram::TOKEN)
        .await
        .unwrap();
    // Réception, coupure, reprise, puis attente longue : trois annonces.
    let polls = || bench.telegram_calls(&["getUpdates"]);
    wait_until("trois annonces", || polls() >= 4).await;
    assert!(bench.on_changed.lock().is_none(), "annonce non retenue");
    assert_eq!(bench.link_of(Platform::Telegram), EasyLinkLink::Connected);

    // Le bot, renommé entre-temps, est relié de nouveau.
    bench
        .service
        .connect(Platform::Telegram, fake_telegram::TOKEN)
        .await
        .unwrap();
    wait_until("nouvelle écoute annoncée", || {
        bench
            .service
            .shared()
            .host
            .states
            .lock()
            .last()
            .is_some_and(|state| state.bots[0].link == EasyLinkLink::Connected)
    })
    .await;
    // Les annonces de l'ancienne écoute passent enfin.
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let bot = &bench.state().bots[0];
    assert_eq!(bot.link, EasyLinkLink::Connected);
    assert_eq!(bot.name, "chatonlm_test_bot");
    let saved = bench
        .store
        .blocking(|c| saved::bot(c, Platform::Telegram))
        .unwrap();
    assert_eq!(saved.bot_name, "chatonlm_test_bot");
}

/// La conversation du compte a déjà un envoi en cours, lancé depuis la page :
/// le moteur de chat refuse le message du bot, qui le dit. Rien ne part au
/// modèle, et la photo jointe attend le message suivant.
#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_busy_in_the_app_refuses_the_message_and_keeps_its_photo() {
    let bench = bench("chatonlm-vision").await;
    bench.telegram_linked().await;
    bench.say("hello");
    bench.wait_answered(1).await;
    let chat_id = bench.account().chat_id.expect("conversation du compte");

    // Depuis la page, une réponse lente dans la même conversation.
    let page = {
        let (service, store, chat_id) = (bench.service.clone(), bench.store.clone(), chat_id);
        tokio::spawn(async move {
            let settings = crate::ollama::target::request_settings(&store)
                .await
                .unwrap();
            let request = SendRequest {
                chat_id: Some(chat_id),
                stream_id: "page".into(),
                content: "from the page".into(),
                model: "chatonlm-markdown-slow".into(),
                think: None,
                attachment_ids: None,
                edit_message_id: None,
                kept_attachment_ids: None,
                web_search: None,
                voice: None,
                image: None,
                ephemeral: None,
            };
            chat::send_to(
                &store,
                &service.shared().host.engine,
                &Staging::default(),
                Server::from_settings(&settings),
                request,
                Origin::Page,
                &|_| {},
            )
            .await
        })
    };
    wait_until("réponse de la page en cours", || {
        bench.ollama.chat_requests().len() == 2
    })
    .await;

    bench.telegram.message(json!({
        "user": ANA, "name": "Ana", "text": "what is this?", "image": true
    }));
    bench
        .wait_told("this chat is already waiting for an answer")
        .await;
    bench.wait_told("Photo received").await;
    wait_until("libre", || !bench.busy()).await;
    assert_eq!(bench.ollama.chat_requests().len(), 2);
    let account = bench.account().id;
    assert_eq!(
        bench
            .service
            .shared()
            .pending
            .lock()
            .get(&account)
            .map(Vec::len),
        Some(1)
    );

    // La réponse de la page finit (ici, arrêtée) : le message suivant emporte
    // la photo.
    bench.service.shared().host.engine.cancel("page");
    page.await.unwrap().unwrap();
    bench.say("and now?");
    bench.wait_answered(3).await;
    let request = &bench.ollama.chat_requests()[2];
    let last = request["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["content"], "and now?");
    assert_eq!(last["images"].as_array().map(Vec::len), Some(1));
    assert!(bench.service.shared().pending.lock().is_empty());
}

/// Le destinataire bloque le bot pendant une réponse : la messagerie refuse
/// toute écriture. La réponse s'arrête dans le moteur de chat, et le bot
/// n'essaie plus d'écrire.
#[tokio::test(flavor = "multi_thread")]
async fn a_recipient_who_blocks_the_bot_stops_the_answer_being_written() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    bench.say("write slowly");
    wait_until("réponse commencée", || bench.told().len() > before).await;
    let writes = || bench.telegram_calls(&["sendMessage", "editMessageText"]);
    bench.telegram.block(ANA);
    let (blocked_at, started) = (writes(), std::time::Instant::now());
    wait_until("réponse arrêtée", || !bench.busy()).await;
    // Bien avant la fin naturelle de la réponse (8 s environ).
    assert!(started.elapsed() < Duration::from_secs(4));
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Une seule écriture refusée, qui a révélé le blocage, puis plus rien.
    assert_eq!(writes(), blocked_at + 1);
    // La partie reçue reste dans l'historique de l'app.
    let listed = bench.store.blocking(|c| chats::list(c)).unwrap();
    let chat = bench
        .store
        .blocking(|c| chats::get(c, &listed[0].id))
        .unwrap();
    assert_eq!(chat.messages.len(), 2);
    assert!(!chat.messages[1].content.is_empty());
}

/// Le destinataire bloque le bot juste après son message : la réponse (ici
/// coupée par une erreur du modèle) ne peut pas s'écrire, et le bot n'essaie
/// pas d'écrire la note qui dirait l'erreur.
#[tokio::test(flavor = "multi_thread")]
async fn a_bot_blocked_before_its_answer_writes_no_final_note() {
    let bench = bench("chatonlm-error").await;
    bench.telegram_linked().await;
    let writes = || bench.telegram_calls(&["sendMessage", "editMessageText"]);
    let before = writes();
    bench.say("hello");
    bench.telegram.block(ANA);
    bench.wait_answered(1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Une seule écriture tentée : la réponse, refusée.
    assert_eq!(writes(), before + 1);
}

/// Plus de photos sans légende que le plafond d'un message (4, `MAX_IMAGES`)
/// : celles en trop sont refusées, y compris dans un album dont les photos
/// sont téléchargées en même temps ; les quatre gardées partent avec le
/// message suivant.
#[tokio::test(flavor = "multi_thread")]
async fn photos_beyond_the_limit_are_refused_and_four_go_with_the_next_message() {
    let bench = bench("chatonlm-vision").await;
    bench.telegram_linked().await;
    let downloads = || bench.telegram_calls(&["getFile"]);
    let kept = || {
        bench
            .service
            .shared()
            .pending
            .lock()
            .values()
            .map(Vec::len)
            .sum::<usize>()
    };

    // Album de cinq photos : toutes passent le premier contrôle avant
    // qu'aucune ne soit gardée (téléchargements retenus).
    bench.telegram.hold("getFile", true);
    for _ in 0..5 {
        bench.photo();
    }
    wait_until("cinq téléchargements en cours", || downloads() == 5).await;
    bench.telegram.hold("getFile", false);
    bench.wait_told("4 photos at most per message").await;
    bench.wait_told("Photo received").await;
    wait_until("quatre photos gardées", || kept() == 4).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(bench.told_count("at most"), 1, "{:?}", bench.told());
    assert_eq!(bench.told_count("Photo received"), 1, "{:?}", bench.told());

    // Une sixième, seule : refusée sans être téléchargée.
    bench.photo();
    wait_until("second refus", || bench.told_count("at most") == 2).await;
    assert_eq!(downloads(), 5);
    assert_eq!(kept(), 4);

    bench.say("what are these?");
    bench.wait_answered(1).await;
    let message = &bench.ollama.chat_requests()[0]["messages"][0];
    assert_eq!(message["content"], "what are these?");
    assert_eq!(message["images"].as_array().map(Vec::len), Some(4));
    assert_eq!(kept(), 0);
}

/// Fermeture de l'app pendant qu'une réponse s'écrit, dans l'ordre de
/// `stop_services` : Easy Link s'arrête, puis le moteur de chat arrête les
/// réponses en cours, celle du bot comprise. Une photo arrivée pendant la
/// réponse n'est pas annoncée au moment de fermer.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_app_while_an_answer_is_written_stops_it() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    let before = bench.told().len();
    bench.say("write slowly");
    wait_until("réponse commencée", || bench.told().len() > before).await;
    // Le faux Ollama n'a pas de modèle lent qui voie : le compte passe à un
    // modèle qui voit pendant la réponse, pour que la photo soit gardée.
    let account = bench.account().id;
    bench
        .store
        .blocking(move |c| saved::set_model(c, account, "chatonlm-vision"))
        .unwrap();
    bench.photo();
    wait_until("photo gardée", || {
        !bench.service.shared().pending.lock().is_empty()
    })
    .await;
    assert!(bench.busy(), "réponse finie avant l'arrivée de la photo");

    let started = std::time::Instant::now();
    bench.service.shutdown();
    bench
        .service
        .shared()
        .host
        .engine
        .stop_all(Duration::from_secs(1))
        .await;
    wait_until("réponse arrêtée", || !bench.busy()).await;
    // Bien avant la fin naturelle de la réponse (8 s environ).
    assert!(started.elapsed() < Duration::from_secs(4));
    // La partie reçue est enregistrée.
    let listed = bench.store.blocking(|c| chats::list(c)).unwrap();
    let chat = bench
        .store
        .blocking(|c| chats::get(c, &listed[0].id))
        .unwrap();
    assert_eq!(chat.messages.len(), 2);
    assert!(!chat.messages[1].content.is_empty());
    // Plus rien n'est écouté ni servi.
    assert_eq!(bench.link_of(Platform::Telegram), EasyLinkLink::Off);
    bench.say("are you there?");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(bench.ollama.chat_requests().len(), 1);
    assert_eq!(bench.told_count("Photo received"), 0, "{:?}", bench.told());
}

/// Le même bot est relié de nouveau (jeton recollé) pendant qu'un message est
/// en route dans l'app : l'ancienne écoute l'a acquitté, la nouvelle ne le
/// recevra pas. Il est servi quand même.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_in_flight_when_the_same_bot_is_relinked_is_served() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    let release = bench
        .message_in_flight(json!({ "user": ANA, "name": "Ana", "text": "in flight" }))
        .await;
    bench.relink().await;
    release.send(()).unwrap();
    bench.wait_answered(1).await;
    let request = &bench.ollama.chat_requests()[0];
    let last = request["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["content"], "in flight");
}

/// Un message remis à l'écoute remplacée, puis de nouveau à la suivante
/// (Telegram ne l'avait pas encore tenu pour acquitté), n'est servi qu'une
/// fois.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_delivered_again_after_relinking_is_served_once() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    let message = json!({ "user": ANA, "name": "Ana", "text": "only once", "messageId": 9001 });
    let release = bench.message_in_flight(message.clone()).await;
    bench.relink().await;
    bench.telegram.message(message);
    bench.wait_answered(1).await;
    // L'ancienne écoute remet enfin le même message.
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!bench.busy());
    assert_eq!(bench.ollama.chat_requests().len(), 1);
}

// --- Images produites (outil `generate_image`) ---

/// PNG valide de 2 × 2 points.
const PNG: [u8; 73] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd, 0xd4, 0x9a,
    0x73, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xb8, 0x99, 0xa3, 0x0d,
    0x44, 0x0c, 0x10, 0x0a, 0x00, 0x2b, 0x06, 0x05, 0xc1, 0x90, 0xd0, 0x9b, 0x7a, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Salon privé d'`ANA` chez le faux Discord.
const ANA_DISCORD: &str = "91001";

/// Ce que la messagerie montre, photos comprises : texte de chaque message
/// (dernière version), ou `[photo <nom> <taille>]`, dans l'ordre d'envoi.
fn shown_with_photos(sent: &[Value]) -> Vec<String> {
    let mut messages: Vec<(String, String)> = Vec::new();
    for action in sent {
        let Some(id) = action["id"].as_str() else {
            continue;
        };
        match action["kind"].as_str() {
            Some("send") => {
                messages.push((id.to_owned(), action["text"].as_str().unwrap().to_owned()))
            }
            Some("edit") => {
                if let Some(message) = messages.iter_mut().find(|(known, _)| known == id) {
                    message.1 = action["text"].as_str().unwrap().to_owned();
                }
            }
            Some("photo") => messages.push((
                id.to_owned(),
                format!(
                    "[photo {} {}]",
                    action["filename"].as_str().unwrap(),
                    action["size"]
                ),
            )),
            _ => {}
        }
    }
    messages.into_iter().map(|(_, text)| text).collect()
}

impl Bench {
    /// Messagerie de `platform` reliée aux faux serveurs, et salon d'`ANA`.
    fn messenger(&self, platform: Platform) -> (Messenger, &'static str) {
        let (token, channel) = match platform {
            Platform::Telegram => (fake_telegram::TOKEN, ANA),
            Platform::Discord => (fake_discord::TOKEN, ANA_DISCORD),
        };
        (
            self.service.shared().messenger(platform, token).unwrap(),
            channel,
        )
    }

    fn sent(&self, platform: Platform) -> Vec<Value> {
        match platform {
            Platform::Telegram => self.telegram.sent(),
            Platform::Discord => self.discord.sent(),
        }
    }

    /// Image produite enregistrée sur une réponse (graine 7).
    fn generated(&self) -> AttachmentInfo {
        self.store
            .blocking(|c| {
                let chat = chats::create(c, 1)?;
                let reply = chats::create_reply(c, &chat, "chatonlm-image", 1)?;
                chats::add_generated_image(c, reply, &PNG, 7, 2)
            })
            .unwrap()
    }
}

/// Le texte d'avant l'image est écrit, la photo part, puis la suite dans un
/// nouveau message.
#[tokio::test(flavor = "multi_thread")]
async fn an_image_goes_between_the_text_before_and_the_text_after() {
    for platform in [Platform::Telegram, Platform::Discord] {
        let bench = bench("qwen3:0.6b").await;
        let (messenger, channel) = bench.messenger(platform);
        let image = bench.generated();
        let mut relay = Relay::new(&messenger, channel);
        relay.push("Here is a cat:");
        relay.tick().await;
        relay.push(" a red one.");
        let png = bot::generated_png(&bench.store, image.id).await;
        assert_eq!(png.as_deref(), Some(&PNG[..]));
        relay.photo(png, &image.filename).await;
        relay.new_turn();
        relay.push("Do you like it?");
        relay.finish().await.unwrap();
        assert_eq!(
            shown_with_photos(&bench.sent(platform)),
            [
                "Here is a cat: a red one.".to_owned(),
                format!("[photo image-7.png {}]", PNG.len()),
                "Do you like it?".to_owned(),
            ],
            "{platform:?}"
        );
        let photo = bench
            .sent(platform)
            .into_iter()
            .find(|action| action["kind"] == "photo")
            .unwrap();
        assert_eq!(photo["contentType"], "image/png");
        assert_eq!(photo["channel"], channel);
        assert!(photo.get("caption").is_none());
    }
}

/// Une image seule, sans texte autour : rien qu'une photo, et la réponse
/// n'est pas « vide ».
#[tokio::test(flavor = "multi_thread")]
async fn an_image_alone_is_an_answer() {
    let bench = bench("qwen3:0.6b").await;
    let (messenger, channel) = bench.messenger(Platform::Telegram);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    relay.new_turn();
    assert!(relay.is_empty());
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    relay.new_turn();
    relay.finish().await.unwrap();
    assert!(!relay.is_empty());
    assert_eq!(
        shown_with_photos(&bench.telegram.sent()),
        [format!("[photo image-7.png {}]", PNG.len())]
    );
}

/// Photo refusée par la messagerie, ou introuvable en base : un court texte
/// à sa place, et la réponse continue.
#[tokio::test(flavor = "multi_thread")]
async fn a_photo_that_cannot_be_sent_is_replaced_by_a_short_text() {
    let failed = crate::i18n::t!("easyLink.photoFailed");
    for platform in [Platform::Telegram, Platform::Discord] {
        let bench = bench("qwen3:0.6b").await;
        match platform {
            Platform::Telegram => bench.telegram.fail_next(
                "sendPhoto",
                400,
                json!({ "ok": false, "error_code": 400, "description": "Bad Request: IMAGE_PROCESS_FAILED" }),
            ),
            Platform::Discord => bench.discord.fail_next("photo", 500, json!({"message": "boom"})),
        }
        let (messenger, channel) = bench.messenger(platform);
        let image = bench.generated();
        let mut relay = Relay::new(&messenger, channel);
        relay.push("Before.");
        relay.photo(Some(PNG.to_vec()), &image.filename).await;
        relay.push("Between.");
        // Image supprimée entre-temps : rien à lire en base.
        let gone = bot::generated_png(&bench.store, image.id + 1000).await;
        assert_eq!(gone, None);
        relay.photo(gone, &image.filename).await;
        relay.push("After.");
        relay.finish().await.unwrap();
        assert_eq!(
            shown_with_photos(&bench.sent(platform)),
            ["Before.", &failed, "Between.", &failed, "After."],
            "{platform:?}"
        );
    }
}

/// « Trop de requêtes » sur la photo : attendue, puis envoyée une fois.
#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limited_photo_is_sent_after_the_wait() {
    let bench = bench("qwen3:0.6b").await;
    let limited = json!({"message": "You are being rate limited.", "retry_after": 0.2});
    bench.discord.fail_next("photo", 429, limited);
    let (messenger, channel) = bench.messenger(Platform::Discord);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    let start = std::time::Instant::now();
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    assert!(start.elapsed() >= Duration::from_millis(200));
    assert_eq!(
        shown_with_photos(&bench.discord.sent()),
        [format!("[photo image-7.png {}]", PNG.len())]
    );
}

/// Destinataire qui a bloqué le bot : la photo refusée, ni texte de repli
/// ni suite.
#[tokio::test(flavor = "multi_thread")]
async fn nothing_more_is_sent_to_a_recipient_who_blocked_the_bot() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram.block(ANA);
    let (messenger, channel) = bench.messenger(Platform::Telegram);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    assert!(relay.gone());
    relay.push("After.");
    assert_eq!(relay.finish().await, Err(Problem::Forbidden));
    assert_eq!(bench.telegram_calls(&["sendPhoto"]), 1);
    assert_eq!(bench.telegram_calls(&["sendMessage"]), 0);
    assert!(bench.telegram.sent().is_empty());
}

/// Le texte d'avant l'image n'a pas pu être écrit : la photo et la suite
/// partent quand même, et la dernière écriture rend le problème (le bot dira
/// la réponse incomplète).
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_text_before_an_image_makes_the_answer_incomplete() {
    let bench = bench("qwen3:0.6b").await;
    bench
        .discord
        .fail_next("send", 400, json!({"message": "nope"}));
    let (messenger, channel) = bench.messenger(Platform::Discord);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    relay.push("Before.");
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    relay.push("After.");
    assert!(matches!(
        relay.finish().await,
        Err(Problem::Refused { status: 400, .. })
    ));
    assert_eq!(
        shown_with_photos(&bench.discord.sent()),
        [
            format!("[photo image-7.png {}]", PNG.len()),
            "After.".to_owned()
        ]
    );
}

/// Seule une image PNG part en photo : une autre pièce jointe (photo JPEG
/// d'un message reçu) n'est pas lue comme telle.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_png_is_read_as_a_generated_image() {
    let bench = bench("qwen3:0.6b").await;
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
    let id = bench
        .store
        .blocking(|c| {
            let chat = chats::create(c, 1)?;
            let attachment = chats::NewAttachment {
                filename: "photo.jpg",
                kind: chats::AttachmentKind::Image,
                mime: "image/jpeg",
                data: &jpeg,
                text: None,
            };
            let message = chats::add_user_message(c, &chat, "look", &[attachment], 2)?;
            Ok(message.attachments[0].id)
        })
        .unwrap();
    assert_eq!(bot::generated_png(&bench.store, id).await, None);
    let image = bench.generated();
    assert!(bot::generated_png(&bench.store, image.id).await.is_some());
}

/// Peintre d'essai : note les invites, annonce un avancement, puis attend
/// `gate` avant de rendre `PNG` (et lance `on_done` juste avant).
struct TestPainter {
    prompts: Arc<Mutex<Vec<String>>>,
    gate: tokio::sync::watch::Receiver<bool>,
    on_done: Mutex<Option<Hook>>,
}

impl chat::images::Painter for TestPainter {
    fn ready(
        &self,
    ) -> chat::images::PaintFuture<'_, Result<chat::images::Ready, chat::images::PaintRefusal>>
    {
        Box::pin(async {
            Ok(chat::images::Ready {
                model_name: "Test painter".into(),
                default_side: 512,
            })
        })
    }

    fn cpu(&self) -> chat::images::PaintFuture<'_, bool> {
        Box::pin(async { false })
    }

    fn paint<'a>(
        &'a self,
        request: chat::images::PaintRequest,
        progress: &'a mut (dyn FnMut(chat::ImageProgress) + Send),
    ) -> chat::images::PaintFuture<'a, Result<chat::images::Painted, String>> {
        Box::pin(async move {
            self.prompts.lock().push(request.prompt.clone());
            progress(chat::ImageProgress {
                stage: chat::ImageStage::Generating,
                step: Some(1),
                total: Some(2),
                preview: None,
                cpu: false,
                width: request.width,
                height: request.height,
            });
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|open| *open).await;
            let on_done = self.on_done.lock().take();
            if let Some(on_done) = on_done {
                on_done();
            }
            Ok(chat::images::Painted {
                png: PNG.to_vec(),
                width: request.width,
                height: request.height,
                seed: 7,
                model_name: "Test painter".into(),
            })
        })
    }
}

/// De bout en bout : le modèle demande une image, le bot montre « envoie une
/// photo… » pendant le calcul, envoie la photo, puis la suite de la réponse
/// dans un nouveau message.
#[tokio::test(flavor = "multi_thread")]
async fn an_image_asked_from_the_bot_arrives_as_a_photo() {
    let bench = bench("chatonlm-image").await;
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (open, gate) = tokio::sync::watch::channel(false);
    bench
        .service
        .shared()
        .host
        .engine()
        .set_painter(Arc::new(TestPainter {
            prompts: Arc::clone(&prompts),
            gate,
            on_done: Mutex::new(None),
        }));
    bench.telegram_linked().await;
    let before = bench.telegram.sent().len();
    bench.say("a red kitten");
    // Pendant le calcul : l'indicateur de Telegram pour une photo.
    wait_until("« envoie une photo… »", || {
        bench.telegram.sent()[before..]
            .iter()
            .any(|action| action["action"] == "upload_photo")
    })
    .await;
    assert_eq!(*prompts.lock(), ["a red kitten"]);
    open.send(true).unwrap();
    bench.wait_answered(2).await;
    let told = shown_with_photos(&bench.telegram.sent()[before..]);
    assert_eq!(
        told,
        [
            format!("[photo image-7.png {}]", PNG.len()),
            "Here is your image.".to_owned(),
        ]
    );
}

/// Arrêt demandé (`/stop`) pendant que l'image finissait, avant que le bot ne
/// la reçoive : la photo ne part pas.
#[tokio::test(flavor = "multi_thread")]
async fn no_photo_is_sent_once_the_answer_is_stopped() {
    let bench = bench("chatonlm-image").await;
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let (_open, gate) = tokio::sync::watch::channel(true);
    // Arrêt retenu par le bot seulement : le moteur, qui ne le sait pas
    // encore, enregistre et annonce l'image.
    let shared = Arc::clone(bench.service.shared());
    let stop: Hook = Box::new(move || {
        for claim in shared.busy.lock().values_mut() {
            claim.stopped = true;
        }
    });
    bench
        .service
        .shared()
        .host
        .engine()
        .set_painter(Arc::new(TestPainter {
            prompts: Arc::clone(&prompts),
            gate,
            on_done: Mutex::new(Some(stop)),
        }));
    bench.telegram_linked().await;
    bench.say("a red kitten");
    wait_until("image faite", || prompts.lock().len() == 1).await;
    wait_until("réponse finie", || !bench.busy()).await;
    let saved: i64 = bench
        .store
        .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM attachments", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(saved, 1, "l'image a été annoncée au bot");
    assert_eq!(bench.telegram_calls(&["sendPhoto", "sendDocument"]), 0);
}

/// « Trop de requêtes » avec une longue attente : la limite reste posée, et
/// rien n'est écrit avant sa fin (ni texte de repli, ni texte, ni photo).
#[tokio::test(flavor = "multi_thread")]
async fn a_long_rate_limit_holds_every_write() {
    let limited = json!({
        "ok": false,
        "error_code": 429,
        "description": "Too Many Requests: retry after 60",
        "parameters": { "retry_after": 60 },
    });
    // Sur la photo : pas de texte de repli, ni la suite.
    let bench = bench("qwen3:0.6b").await;
    bench.telegram.fail_next("sendPhoto", 429, limited.clone());
    let (messenger, channel) = bench.messenger(Platform::Telegram);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    relay.push("After.");
    relay.tick().await;
    assert!(matches!(
        relay.finish().await,
        Err(Problem::RateLimited(wait)) if wait > Duration::from_secs(50)
    ));
    assert_eq!(bench.telegram_calls(&["sendPhoto"]), 1);
    assert_eq!(bench.telegram_calls(&["sendMessage", "editMessageText"]), 0);

    // Sur le texte d'avant l'image : ni photo, ni texte de repli, ni la
    // suite.
    let bench = self::bench("qwen3:0.6b").await;
    bench.telegram.fail_next("sendMessage", 429, limited);
    let (messenger, channel) = bench.messenger(Platform::Telegram);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    relay.push("Before.");
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    relay.push("After.");
    relay.tick().await;
    assert!(matches!(relay.finish().await, Err(Problem::RateLimited(_))));
    assert_eq!(bench.telegram_calls(&["sendMessage"]), 1);
    assert_eq!(
        bench.telegram_calls(&["sendPhoto", "sendDocument", "editMessageText"]),
        0
    );
}

/// Échec du réseau à l'envoi de la photo (délai dépassé, réponse illisible) :
/// elle a pu arriver, le texte qui la remplace le dit avec prudence.
#[tokio::test(flavor = "multi_thread")]
async fn a_photo_lost_on_the_network_is_told_with_caution() {
    let bench = bench("qwen3:0.6b").await;
    // Réponse illisible : `Problem::Network`, comme un délai dépassé.
    bench
        .telegram
        .fail_next("sendPhoto", 200, json!({ "ok": false }));
    let (messenger, channel) = bench.messenger(Platform::Telegram);
    let image = bench.generated();
    let mut relay = Relay::new(&messenger, channel);
    relay.photo(Some(PNG.to_vec()), &image.filename).await;
    relay.finish().await.unwrap();
    let maybe = crate::i18n::t!("easyLink.photoMaybeFailed");
    assert_ne!(maybe, crate::i18n::t!("easyLink.photoFailed"));
    assert_eq!(shown_with_photos(&bench.telegram.sent()), [maybe]);
}

/// Dernière écriture refusée avec une longue attente : la réponse est
/// incomplète, mais le texte qui le dit n'est pas envoyé pendant la limite.
#[tokio::test(flavor = "multi_thread")]
async fn the_final_note_waits_for_a_long_rate_limit() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    let before = bench.telegram_calls(&["sendMessage"]);
    let limited = json!({
        "ok": false,
        "error_code": 429,
        "description": "Too Many Requests: retry after 60",
        "parameters": { "retry_after": 60 },
    });
    bench.telegram.fail_next("sendMessage", 429, limited);
    bench.say("hi");
    bench.wait_answered(1).await;
    // Le seul envoi, refusé ; ni nouvel essai, ni « réponse incomplète ».
    assert_eq!(bench.telegram_calls(&["sendMessage"]), before + 1);
    assert_eq!(bench.telegram_calls(&["editMessageText"]), 0);
}

// Mode éphémère d'un compte (`/ephemeral`, `/keep`).

impl Bench {
    /// Conversation en cours du compte d'`ANA`.
    fn current_chat(&self) -> Option<String> {
        self.account().chat_id
    }

    fn is_ephemeral(&self, id: &str) -> bool {
        let id = id.to_owned();
        self.store
            .blocking(move |c| chats::is_ephemeral(c, &id))
            .unwrap()
    }

    /// La conversation n'existe plus.
    fn erased(&self, id: &str) -> bool {
        let id = id.to_owned();
        matches!(
            self.store.blocking(move |c| chats::get(c, &id)),
            Err(AppError::NotFound(_))
        )
    }

    /// Identifiants de l'historique de l'app.
    fn history(&self) -> Vec<String> {
        self.store
            .blocking(|c| chats::list(c))
            .unwrap()
            .into_iter()
            .map(|chat| chat.id)
            .collect()
    }

    /// Conversations en base, éphémères comprises.
    fn chat_count(&self) -> i64 {
        self.store
            .blocking(|c| Ok(c.query_row("SELECT COUNT(*) FROM chats", [], |r| r.get(0))?))
            .unwrap()
    }
}

/// Mode actif : chaque nouvelle conversation est éphémère (hors de
/// l'historique) ; `/new` l'efface et la suivante l'est aussi ;
/// `/ephemeral off` efface la conversation en cours, et la suivante est
/// ordinaire.
#[tokio::test(flavor = "multi_thread")]
async fn the_ephemeral_mode_opens_conversations_that_are_erased_when_left() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    // L'accueil liste les deux commandes.
    assert!(
        bench
            .told()
            .iter()
            .any(|text| text.contains("/ephemeral on"))
    );
    assert!(bench.told().iter().any(|text| text.contains("/keep")));
    bench.say("/ephemeral");
    bench.wait_told("Ephemeral conversations: off").await;
    bench.say("/ephemeral maybe");
    bench.wait_told("/ephemeral on or /ephemeral off").await;
    assert!(!bench.account().ephemeral);

    bench.say("/ephemeral on");
    bench.wait_told("Ephemeral conversations are on").await;
    assert!(bench.account().ephemeral);
    bench.say("hello");
    bench.wait_answered(1).await;
    let first = bench.current_chat().expect("conversation ouverte");
    assert!(bench.is_ephemeral(&first));
    assert!(bench.history().is_empty());
    // Le message suivant la continue.
    bench.say("and then?");
    bench.wait_answered(2).await;
    assert_eq!(bench.current_chat().as_deref(), Some(first.as_str()));
    assert_eq!(
        bench.ollama.chat_requests()[1]["messages"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    // `/new` l'efface ; la suivante est éphémère aussi.
    bench.say("/new");
    bench.wait_told("New conversation").await;
    assert!(bench.erased(&first));
    assert_eq!(bench.current_chat(), None);
    assert!(
        bench
            .told()
            .iter()
            .any(|text| text.contains("The ephemeral conversation has been erased"))
    );
    bench.say("fresh start");
    bench.wait_answered(3).await;
    let second = bench.current_chat().expect("conversation ouverte");
    assert_ne!(second, first);
    assert!(bench.is_ephemeral(&second));
    assert!(bench.history().is_empty());
    bench.say("/ephemeral");
    bench.wait_told("Ephemeral conversations: on").await;

    // Coupé : la conversation en cours est effacée, la suivante est gardée.
    bench.say("/ephemeral off");
    bench.wait_told("Ephemeral conversations are off").await;
    assert!(!bench.account().ephemeral);
    assert!(bench.erased(&second));
    assert_eq!(bench.current_chat(), None);
    bench.say("keep this one");
    bench.wait_answered(4).await;
    let third = bench.current_chat().expect("conversation ouverte");
    assert!(!bench.is_ephemeral(&third));
    assert_eq!(bench.history(), std::slice::from_ref(&third));
    // Coupé de nouveau : une conversation ordinaire continue.
    bench.say("/ephemeral off");
    wait_until("second avis", || {
        bench.told_count("Ephemeral conversations are off") == 2
    })
    .await;
    assert_eq!(bench.current_chat(), Some(third));
}

/// `/ephemeral on` pendant une conversation ordinaire la laisse dans
/// l'historique ; `/keep` fait passer la conversation éphémère dans
/// l'historique, et le message suivant en ouvre une nouvelle, éphémère.
#[tokio::test(flavor = "multi_thread")]
async fn keep_puts_the_ephemeral_conversation_in_the_history() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.say("/keep");
    bench.wait_told("no ephemeral conversation to keep").await;

    bench.say("hello");
    bench.wait_answered(1).await;
    let ordinary = bench.current_chat().expect("conversation ouverte");
    bench.say("/ephemeral on");
    bench.wait_told("Ephemeral conversations are on").await;
    assert_eq!(bench.current_chat(), None);
    assert_eq!(bench.history(), std::slice::from_ref(&ordinary));
    assert!(!bench.is_ephemeral(&ordinary));

    bench.say("a secret");
    bench.wait_answered(2).await;
    let secret = bench.current_chat().expect("conversation ouverte");
    assert_ne!(secret, ordinary);
    assert!(bench.is_ephemeral(&secret));
    // Nouvelle conversation : seul le message envoyé part au modèle.
    assert_eq!(
        bench.ollama.chat_requests()[1]["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // `/ephemeral on` de nouveau : la conversation éphémère continue.
    bench.say("/ephemeral on");
    wait_until("second avis", || {
        bench.told_count("Ephemeral conversations are on") == 2
    })
    .await;
    assert_eq!(bench.current_chat().as_deref(), Some(secret.as_str()));

    bench.updated.lock().clear();
    bench.say("/keep");
    bench.wait_told("now in ChatonLM's history").await;
    assert!(!bench.is_ephemeral(&secret));
    assert!(bench.history().contains(&secret));
    assert_eq!(bench.current_chat(), None);
    // La page est prévenue.
    assert!(bench.updated.lock().contains(&secret));

    bench.say("next one");
    bench.wait_answered(3).await;
    let next = bench.current_chat().expect("conversation ouverte");
    assert!(next != secret && next != ordinary);
    assert!(bench.is_ephemeral(&next));
    assert_eq!(bench.history().len(), 2);
}

/// `/new` pendant une réponse dans une conversation éphémère : la réponse
/// s'arrête et rien n'en reste.
#[tokio::test(flavor = "multi_thread")]
async fn new_during_an_ephemeral_answer_stops_it_and_leaves_nothing() {
    let bench = bench("chatonlm-markdown-slow").await;
    bench.telegram_linked().await;
    bench.say("/ephemeral on");
    bench.wait_told("Ephemeral conversations are on").await;
    let before = bench.told().len();
    bench.say("write slowly");
    wait_until("réponse commencée", || bench.told().len() > before).await;
    let chat = bench.current_chat().expect("conversation ouverte");
    let started = std::time::Instant::now();
    bench.say("/new");
    bench.wait_told("New conversation").await;
    wait_until("réponse arrêtée", || !bench.busy()).await;
    // Bien avant la fin naturelle de la réponse (8 s environ).
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(bench.erased(&chat));
    assert_eq!(bench.chat_count(), 0);
    assert_eq!(bench.current_chat(), None);
}

/// Délier le compte, par le bot, par la page ou en retirant le bot, efface sa
/// conversation éphémère ; une conversation ordinaire reste.
#[tokio::test(flavor = "multi_thread")]
async fn unlinking_erases_the_ephemeral_conversation() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.say("/ephemeral on");
    bench.wait_told("Ephemeral conversations are on").await;
    bench.say("hello");
    bench.wait_answered(1).await;
    let chat = bench.current_chat().expect("conversation ouverte");
    bench.say("/unlink");
    bench.wait_told("This account is unlinked").await;
    assert!(bench.erased(&chat));
    assert!(bench.state().accounts.is_empty());

    // Lié de nouveau, délié par la page.
    let link_again = async || {
        let code = bench
            .service
            .pair(Platform::Telegram)
            .await
            .unwrap()
            .pairing
            .unwrap()
            .code;
        bench.say(&format!("/start {code}"));
        wait_until("compte lié", || bench.state().accounts.len() == 1).await;
        bench.say("/ephemeral on");
        wait_until("mode actif", || bench.account().ephemeral).await;
    };
    link_again().await;
    bench.say("hello again");
    bench.wait_answered(2).await;
    let chat = bench.current_chat().expect("conversation ouverte");
    let account = bench.state().accounts[0].id;
    bench.service.unlink(account).await.unwrap();
    assert!(bench.erased(&chat));

    // Lié de nouveau, bot retiré.
    link_again().await;
    bench.say("one more");
    bench.wait_answered(3).await;
    let chat = bench.current_chat().expect("conversation ouverte");
    bench.service.disconnect(Platform::Telegram).await.unwrap();
    assert!(bench.erased(&chat));
    assert_eq!(bench.chat_count(), 0);
}

/// `/ephemeral` passé entre la relecture du compte et l'ouverture de la
/// conversation (simulé au premier accès au moteur, celui de l'envoi) : la
/// conversation ouverte selon l'ancien mode n'est pas rattachée au compte ;
/// éphémère, elle est effacée à la fin de la réponse ; ordinaire, elle reste
/// dans l'historique. Le message suivant suit le nouveau mode.
#[tokio::test(flavor = "multi_thread")]
async fn a_mode_changed_while_a_conversation_opens_does_not_attach_it() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.say("/ephemeral on");
    bench.wait_told("Ephemeral conversations are on").await;
    let account = bench.account().id;
    let switch_to = |enabled: bool| {
        let store = bench.store.clone();
        *bench.on_engine.lock() = Some(Box::new(move || {
            tokio::task::block_in_place(|| {
                store.blocking(|c| saved::set_ephemeral(c, account, enabled))
            })
            .unwrap();
        }));
    };

    // Coupé pendant l'ouverture d'une conversation éphémère.
    switch_to(false);
    bench.say("hello");
    bench.wait_answered(1).await;
    assert!(bench.on_engine.lock().is_none(), "crochet jamais lancé");
    assert_eq!(bench.current_chat(), None);
    assert_eq!(bench.chat_count(), 0);
    bench.say("again");
    bench.wait_answered(2).await;
    let ordinary = bench.current_chat().expect("conversation ouverte");
    assert!(!bench.is_ephemeral(&ordinary));

    // Activé pendant l'ouverture d'une conversation ordinaire.
    bench.say("/new");
    bench.wait_told("New conversation").await;
    switch_to(true);
    bench.say("still ordinary");
    bench.wait_answered(3).await;
    assert_eq!(bench.current_chat(), None);
    let history = bench.history();
    assert_eq!(history.len(), 2);
    bench.say("secret");
    bench.wait_answered(4).await;
    let secret = bench.current_chat().expect("conversation ouverte");
    assert!(bench.is_ephemeral(&secret));
    assert_eq!(bench.history(), history);
}

/// `/new` et `/ephemeral off` détachent la conversation éphémère avant
/// d'arrêter sa réponse et de l'effacer : un message arrivé entre-temps en
/// ouvre une autre au lieu de partir dans celle-ci.
#[tokio::test(flavor = "multi_thread")]
async fn the_ephemeral_chat_is_detached_before_it_is_erased() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.say("/ephemeral on");
    bench.wait_told("Ephemeral conversations are on").await;
    let account = bench.account().id;
    let seen: Arc<Mutex<Option<Option<String>>>> = Arc::default();
    // Au premier accès au moteur (l'arrêt de la réponse) : conversation du
    // compte à cet instant.
    let watch = || {
        let (store, seen) = (bench.store.clone(), Arc::clone(&seen));
        *bench.on_engine.lock() = Some(Box::new(move || {
            let current = tokio::task::block_in_place(|| {
                store.blocking(|c| saved::account_by_id(c, account))
            })
            .unwrap()
            .unwrap()
            .chat_id;
            *seen.lock() = Some(current);
        }));
    };

    bench.say("hello");
    bench.wait_answered(1).await;
    let first = bench.current_chat().expect("conversation ouverte");
    watch();
    bench.say("/new");
    bench.wait_told("New conversation").await;
    assert_eq!(seen.lock().take(), Some(None));
    assert!(bench.erased(&first));

    bench.say("hello again");
    bench.wait_answered(2).await;
    let second = bench.current_chat().expect("conversation ouverte");
    watch();
    bench.say("/ephemeral off");
    bench.wait_told("Ephemeral conversations are off").await;
    assert_eq!(seen.lock().take(), Some(None));
    assert!(bench.erased(&second));
}

// Easy Link prévient à la fin d'une tâche de VS Code (lot 4a du lien VS
// Code) : envoi spontané borné, réglage coupé par défaut, tâches dans
// `/status`.

fn note(title: &str) -> notify::Note {
    notify::Note {
        title: title.to_owned(),
        outcome: notify::Outcome::Success,
        duration_ms: 185_000,
        workspace: Some("/home/ana/projets/chaton-web".to_owned()),
    }
}

/// Messages envoyés par le bot Telegram (`sendMessage`).
fn telegram_sends(bench: &Bench) -> Vec<Value> {
    bench
        .telegram
        .sent()
        .into_iter()
        .filter(|sent| sent["kind"] == "send")
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vscode_notice_needs_the_setting_and_a_linked_account() {
    let bench = bench("qwen3:0.6b").await;
    // Coupé par défaut.
    assert!(!bench.state().vscode_notify);
    let shared = bench.service.shared();
    assert_eq!(
        shared.notify(&note("build")).await,
        Err(notify::NotifyError::Disabled)
    );
    let update = notify::TaskUpdate {
        id: "1".to_owned(),
        title: "build".to_owned(),
        state: notify::TaskState::Running,
        started_at: now_ms(),
    };
    assert_eq!(
        shared.task(1, update.clone()).await,
        Err(notify::NotifyError::Disabled)
    );
    let state = bench.service.set_vscode_notify(true).await.unwrap();
    assert!(state.vscode_notify);
    // Aucun bot relié : personne à prévenir.
    assert_eq!(
        shared.notify(&note("build")).await,
        Err(notify::NotifyError::NotLinked)
    );
    shared.task(1, update).await.unwrap();

    bench.telegram_linked().await;
    // Coupé de nouveau : rien ne part, les tâches sont oubliées.
    bench.service.set_vscode_notify(false).await.unwrap();
    let before = telegram_sends(&bench).len();
    assert_eq!(
        shared.notify(&note("build")).await,
        Err(notify::NotifyError::Disabled)
    );
    assert_eq!(telegram_sends(&bench).len(), before);
    bench.service.set_vscode_notify(true).await.unwrap();
    assert!(shared.task_lines().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vscode_notice_is_bounded_in_rate_and_content() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.service.set_vscode_notify(true).await.unwrap();
    let shared = bench.service.shared();
    let before = telegram_sends(&bench).len();
    let title = format!(
        "cargo test --manifest-path /home/ana/projets/secret/Cargo.toml\n```\nerror: {}\n```",
        "x".repeat(3000)
    );
    assert_eq!(shared.notify(&note(&title)).await, Ok(1));
    let sends = telegram_sends(&bench);
    assert_eq!(sends.len(), before + 1);
    let sent = &sends[before];
    // Écrit en privé, sans message reçu juste avant : à l'identifiant du
    // compte.
    assert_eq!(sent["channel"], ANA);
    let text = sent["text"].as_str().unwrap();
    assert!(text.contains("Succeeded"), "{text}");
    assert!(text.contains("3 min 05 s"), "{text}");
    assert!(text.contains("Cargo.toml"), "{text}");
    assert!(text.contains("chaton-web"), "{text}");
    for leak in [
        "/home",
        "ana/projets",
        "secret",
        "```",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ] {
        assert!(!text.contains(leak), "{leak} dans {text}");
    }
    let shown = notify::message(&note(&title));
    assert!(shown.chars().count() <= notify::MAX_CHARS);
    assert!(
        !shown.contains('\n') || shown.lines().count() == 2,
        "{shown}"
    );

    // Moins de 30 s après : rien ne part pour ce compte.
    assert_eq!(shared.notify(&note("build")).await, Ok(0));
    assert_eq!(telegram_sends(&bench).len(), before + 1);
    // 30 s plus tard : de nouveau.
    let account = bench.account().id;
    shared.board.sent_at(
        account,
        std::time::Instant::now() - notify::MIN_INTERVAL - Duration::from_secs(1),
    );
    assert_eq!(shared.notify(&note("build")).await, Ok(1));
    assert_eq!(telegram_sends(&bench).len(), before + 2);
}

/// Envoi raté (le destinataire a bloqué le bot) : rien n'est parti, et le
/// compte n'est pas tenu 30 s pour autant.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_vscode_notice_does_not_hold_the_account_for_30_seconds() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.service.set_vscode_notify(true).await.unwrap();
    let shared = bench.service.shared();
    let account = bench.account().id;
    bench.telegram.block(ANA);
    assert_eq!(shared.notify(&note("build")).await, Ok(0));
    assert_eq!(shared.board.last_sent(account), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vscode_notice_opens_the_private_channel_on_discord() {
    let bench = bench("qwen3:0.6b").await;
    bench
        .service
        .connect(Platform::Discord, fake_discord::TOKEN)
        .await
        .unwrap();
    wait_until("Discord connecté", || {
        bench.link_of(Platform::Discord) == EasyLinkLink::Connected
    })
    .await;
    let pairing = bench.service.pair(Platform::Discord).await.unwrap();
    bench.discord.message(json!({
        "user": "3003", "name": "Luis", "text": pairing.pairing.unwrap().code
    }));
    wait_until("compte Discord lié", || bench.state().accounts.len() == 1).await;
    bench.service.set_vscode_notify(true).await.unwrap();
    let mut done = note("npm run build");
    done.outcome = notify::Outcome::Failure;
    done.workspace = None;
    assert_eq!(bench.service.shared().notify(&done).await, Ok(1));
    let sent = bench.discord.sent();
    let opened = sent
        .iter()
        .position(|action| action == &json!({"kind": "dm", "recipient": "3003"}))
        .expect("salon privé ouvert");
    let message = &sent[opened + 1];
    assert_eq!(message["kind"], "send");
    assert_eq!(message["channel"], "93003");
    let text = message["text"].as_str().unwrap();
    assert!(
        text.contains("Failed") && text.contains("`npm run build`"),
        "{text}"
    );
    assert!(!text.contains("Folder"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn status_shows_the_vscode_tasks_running_or_finished_within_an_hour() {
    let bench = bench("qwen3:0.6b").await;
    bench.telegram_linked().await;
    bench.service.set_vscode_notify(true).await.unwrap();
    let shared = bench.service.shared();
    let now = now_ms();
    let task = |id: &str, title: &str, state| notify::TaskUpdate {
        id: id.to_owned(),
        title: title.to_owned(),
        state,
        started_at: now - 120_000,
    };
    shared
        .task(7, task("a", "npm run build", notify::TaskState::Running))
        .await
        .unwrap();
    shared
        .task(
            7,
            task("b", "cargo test /home/ana/x", notify::TaskState::Failure),
        )
        .await
        .unwrap();
    shared
        .task(8, task("c", "lint", notify::TaskState::Running))
        .await
        .unwrap();
    // La connexion 8 finit : sa tâche en cours ne se finira plus.
    shared.board.connection_ended(8);
    // Une tâche finie il y a plus d'une heure n'est plus montrée.
    shared
        .task(7, task("d", "old", notify::TaskState::Success))
        .await
        .unwrap();
    shared.board.ended_at(7, "d", now - 2 * 3600 * 1000);

    bench.say("/status");
    bench.wait_told("VS Code tasks:").await;
    let status = bench
        .told()
        .into_iter()
        .find(|text| text.contains("VS Code tasks:"))
        .unwrap();
    assert!(
        status.contains("npm run build") && status.contains("running for 2 min"),
        "{status}"
    );
    assert!(
        status.contains("cargo test x") && status.contains("failed 0 s ago"),
        "{status}"
    );
    assert!(!status.contains("/home"), "{status}");
    assert!(!status.contains("lint"), "{status}");
    assert!(!status.contains("old"), "{status}");

    // Réglage coupé : `/status` ne les montre plus.
    bench.service.set_vscode_notify(false).await.unwrap();
    let before = bench.told_count("VS Code tasks:");
    bench.say("/status");
    bench.wait_told("Web search: off").await;
    wait_until("second statut", || bench.told_count("Web search: off") >= 2).await;
    assert_eq!(bench.told_count("VS Code tasks:"), before);
}
