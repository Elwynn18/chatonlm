//! Essais d'Easy Link de bout en bout, sans fenêtre : faux Ollama, faux
//! Telegram et faux Discord, base en mémoire.

use std::sync::Arc;
use std::time::Duration;

use easy_link::fake::discord::{self as fake_discord, FakeDiscord};
use easy_link::fake::telegram::{self as fake_telegram, FakeTelegram};
use fake_ollama::{FakeOllama, Mode};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::*;
use crate::chat::{self, ChatEvent, Origin, SendRequest, Server};
use crate::store::chats;
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
    assert_eq!(listed[0].source, Some(Platform::Telegram));
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
    assert_eq!(listed[0].source, Some(Platform::Discord));
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
