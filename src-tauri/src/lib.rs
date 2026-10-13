//! ChatonLM : client desktop pour Ollama.

pub mod attachments;
mod chat;
mod commands;
mod context_menu;
mod desktop;
mod dictation;
mod easy_link;
mod error;
mod graphics;
mod hub;
mod i18n;
mod ollama;
mod os_util;
mod paths;
mod proofread;
mod startup;
mod store;
mod study;
mod tts;
mod updater;
mod voice_mode;
mod window;

use std::sync::Arc;
use std::time::Duration;

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, RunEvent, WindowEvent};

use attachments::{Staging, protocol};
use chat::ChatEngine;
use dictation::Dictation;
use dictation::downloads::SpeechDownloads;
use hub::downloads::HubDownloads;
use startup::Startup;
use store::Store;

/// Temps laissé aux réponses en cours pour s'enregistrer, et aux
/// téléchargements du ModelHub pour fermer leur requête, à la fermeture.
const EXIT_GRACE: Duration = Duration::from_secs(1);

/// Processus enfant (extraction d'un PDF) : si l'app a été lancée pour cela,
/// fait le travail et renvoie le code de sortie, sans rien initialiser
/// d'autre ; sinon `None`. Appelé au tout début de `main`.
pub fn child_process() -> Option<i32> {
    attachments::pdf_child::run_if_requested()
}

/// Ollama va s'arrêter (fermeture, intégré qui ne sert plus, changement de
/// son dossier des modèles) : réponses du chat, cours (rien noté en échec)
/// et corrections (page et raccourci global) s'arrêtent.
fn stop_ollama_work(app: &tauri::AppHandle) {
    app.state::<ChatEngine>().cancel_all();
    app.state::<study::StudyEngine>().cancel_all();
    app.state::<proofread::Proofreader>().cancel_all();
    app.state::<proofread::selection::Selection>().cancel();
}

/// Point d'entrée de l'application.
pub fn run() {
    graphics::apply_nvidia_workaround();
    // `APPIMAGE` et `APPDIR` hérités d'une autre AppImage par un paquet qui
    // n'en est pas une (.deb lancé depuis le terminal d'un éditeur livré en
    // AppImage) : retirés avant que Tauri ne les lise (relance, dossier des
    // ressources).
    updater::forget_foreign_appimage();

    let mut context = tauri::generate_context!();
    paths::apply_data_dir_override(&mut context);

    // Instance unique : relancer ChatonLM ramène la fenêtre existante au premier
    // plan (nom D-Bus dérivé de l'identifiant, distinct en développement) ;
    // `chatonlm --dictate` bascule la dictée, `chatonlm --proofread` corrige le
    // texte sélectionné, sans la montrer.
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            desktop::on_second_instance(app, &args);
        }))
        // Sélecteurs natifs, ouverts seulement par les commandes pick_files et
        // pick_models_folder : la page n'a aucune permission `dialog:*`.
        .plugin(tauri_plugin_dialog::init())
        // Mises à jour (module updater) : seulement depuis le moteur, la
        // page n'a aucune permission `updater:*`.
        .plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_webdriver::init());
    // Raccourcis globaux de Windows (`RegisterHotKey`) : appelés du moteur
    // seulement (`desktop::shortcut`), la page n'a aucune permission
    // `global-shortcut:*`. Sous Linux, c'est le portail du bureau.
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());

    builder
        .setup(|app| {
            // Téléchargements du ModelHub : la liste complète part vers la
            // page à chaque changement (événement `hub-downloads`). Ils
            // survivent à un rechargement de la page.
            let handle = app.handle().clone();
            app.manage(HubDownloads::with_options(
                move |downloads| {
                    let _ = handle.emit(hub::DOWNLOADS_EVENT, downloads);
                },
                hub::downloads::Options {
                    location_changing: commands::models_dir::location_changing,
                    ..hub::downloads::Options::default()
                },
            ));
            #[cfg(debug_assertions)]
            // Permet aux essais de vérifier l'identifiant et le dossier de données.
            eprintln!(
                "chatonlm: identifier={} data={:?}",
                app.config().identifier,
                app.path().app_local_data_dir()
            );
            let data_dir = paths::data_dir(app.handle())?;
            let database = data_dir.join("db").join("chatonlm.sqlite");
            // Une base illisible n'empêche pas la fenêtre de s'ouvrir : le
            // frontend montre le problème (voir startup.rs).
            let startup = Startup::new(database);
            let store = startup.open();
            // Langue des textes du moteur (zone de notification, notifications,
            // erreurs) avant tout le reste.
            i18n::set_current(i18n::saved(store.as_ref()));
            #[cfg(any(debug_assertions, feature = "e2e"))]
            if let Some(store) = &store
                && std::env::var_os("CHATONLM_SEED_DEMO").is_some()
            {
                let now = store::now_ms();
                store.blocking(move |connection| {
                    store::chats::seed_demo_if_empty(connection, now)
                })?;
            }
            // Ollama intégré et cible de toutes les requêtes à Ollama
            // (`ollama::target`) : un changement de cible ferme les
            // téléchargements vers l'ancienne et prévient la page ; avant
            // tout arrêt de l'intégré par la cible, chat, cours et
            // corrections s'arrêtent. Démarré au plus tôt quand il sert
            // (source `builtin`, ou rien d'installé).
            let bin = ollama::builtin::locate(app.path().resource_dir().ok().as_deref());
            let builtin =
                ollama::builtin::Builtin::new(ollama::builtin::Config::new(bin, &data_dir));
            let handle = app.handle().clone();
            let stopping = app.handle().clone();
            let targeting = Arc::new(
                ollama::target::Targeting::new(builtin, move |target| {
                    ollama::target::announce(
                        target,
                        handle.try_state::<HubDownloads>().as_deref(),
                        |target| {
                            let _ = handle.emit(ollama::target::TARGET_EVENT, target);
                        },
                    );
                })
                .before_builtin_stop(move || stop_ollama_work(&stopping)),
            );
            ollama::target::install(targeting.clone());
            // Ménage d'un intégré laissé par une session précédente, puis
            // premier choix de la cible (une requête précoce l'attend).
            let store_for_target = store.clone();
            tauri::async_runtime::spawn(async move {
                let cleaning = targeting.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    cleaning.builtin().clean_leftovers();
                })
                .await;
                if let Some(store) = store_for_target {
                    targeting.first_decision(&store).await;
                }
            });
            // Catalogue du ModelHub : cache disque sous le dossier des données.
            app.manage(hub::state::Catalog::new(data_dir.join("modelhub")));
            // Dictée : téléchargements des modèles de transcription (liste
            // complète vers la page à chaque changement, événement
            // `speech-downloads` ; statut de la dictée renvoyé quand un modèle
            // est installé) ; identité et icône des notifications.
            let handle = app.handle().clone();
            let speech_downloads = SpeechDownloads::new(
                move |downloads| {
                    let _ = handle.emit(dictation::downloads::DOWNLOADS_EVENT, downloads);
                },
                paths::speech_models_dir(app.handle())?,
            );
            let handle = app.handle().clone();
            speech_downloads.on_installed(move |_| {
                handle.state::<Dictation>().emit_status(&handle);
            });
            app.manage(speech_downloads);
            // Lecture à voix haute : file des voix (dossier `voice-models`,
            // événement `voice-downloads`, suivie aussi par la lecture qui
            // attend sa voix) ; état de la lecture vers la page (événement
            // `read-aloud`) ; lecture automatique branchée sur le moteur de
            // chat, selon le réglage enregistré.
            let voice_models = paths::voice_models_dir(app.handle())?;
            let handle = app.handle().clone();
            app.manage(tts::VoiceDownloads(SpeechDownloads::with_options(
                move |downloads| {
                    handle.state::<tts::ReadAloud>().on_downloads(downloads);
                    let _ = handle.emit(tts::VOICE_DOWNLOADS_EVENT, downloads);
                },
                voice_models.clone(),
                dictation::downloads::Options {
                    test_env: tts::VOICE_MODELS_URL_ENV,
                    find: tts::models::find_item,
                    ..dictation::downloads::Options::default()
                },
            )));
            let handle = app.handle().clone();
            let read_aloud = app.state::<tts::ReadAloud>();
            read_aloud.configure(tts::service::Backend::real(voice_models), move |state| {
                let _ = handle.emit(tts::READ_ALOUD_EVENT, state);
            });
            if let Some(store) = &store
                && let Ok(settings) =
                    store.blocking(|connection| store::settings::get(connection, None))
            {
                read_aloud.set_auto_read(settings.auto_read);
            }
            // Mode conversation vocale : écoute du système, moteur de
            // transcription de la dictée ; la séance suit ses envois vocaux
            // (observateur du chat) et leur lecture (écouteur de la lecture
            // à voix haute).
            app.manage(voice_mode::VoiceMode::for_app(
                app.handle(),
                voice_mode::audio::system_ear(),
                app.state::<Dictation>().engine(),
            ));
            let handle = app.handle().clone();
            read_aloud.set_voice_listener(move |chat, reading| {
                if let Some(voice_mode) = handle.try_state::<voice_mode::VoiceMode>() {
                    voice_mode.on_reading(chat, reading);
                }
            });
            let handle = app.handle().clone();
            app.state::<ChatEngine>()
                .set_observer(move |chat, event, voice| {
                    handle.state::<tts::ReadAloud>().observe(chat, event, voice);
                    if let Some(voice_mode) = handle.try_state::<voice_mode::VoiceMode>() {
                        voice_mode.observe(chat, event, voice);
                    }
                });
            app.state::<Dictation>().setup(app.handle());
            window::create_main_window(app, store.as_ref())?;
            if let Some(store) = store {
                app.manage(store);
            }
            app.manage(startup);
            // Mises à jour : moteur prêt avant la zone de notification (qui
            // lit son état), vérification 3 s plus tard.
            updater::setup(app.handle());
            // Easy Link : les bots reliés écoutent dès le lancement.
            easy_link::setup(app.handle());
            // Zone de notification, raccourcis globaux, `--dictate` et
            // `--proofread`.
            desktop::setup(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Avec une zone de notification, la fenêtre n'est que masquée.
            WindowEvent::CloseRequested { api, .. } => {
                window::remember_size(window);
                desktop::on_close_requested(window, api);
            }
            // Dépôt natif : ses chemins (absolus ; wry y met aussi les
            // adresses d'un lien glissé) deviennent préparables, une fois
            // chacun. Tauri a déjà émis `tauri://drag-drop` vers la page, mais
            // la page ne peut appeler `stage_paths` qu'une fois ce rappel fini
            // (même fil principal). Un `tauri://drag-drop` émis par la page
            // elle-même ne passe pas ici : il n'autorise rien.
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                window.state::<Staging>().grant(
                    paths
                        .iter()
                        .filter(|path| path.is_absolute())
                        .map(|path| path.to_string_lossy().into_owned()),
                );
            }
            _ => {}
        })
        // Page rechargée : ses flux n'ont plus de destinataire (chat, cours),
        // ses fichiers préparés plus de composer, ses chemins remis plus de
        // destinataire, la séance du mode vocal plus de page qui envoie.
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                if let Some(voice_mode) = webview.app_handle().try_state::<voice_mode::VoiceMode>()
                {
                    voice_mode.stop();
                }
                webview.app_handle().state::<ChatEngine>().cancel_page();
                webview
                    .app_handle()
                    .state::<study::StudyEngine>()
                    .cancel_streams();
                webview.app_handle().state::<Staging>().clear();
            }
        })
        // Images des pièces jointes (chatonlm://localhost/...).
        .register_asynchronous_uri_scheme_protocol(
            protocol::SCHEME,
            |context, request, responder| {
                let app = context.app_handle().clone();
                let uri = request.uri().to_string();
                tauri::async_runtime::spawn(async move {
                    let store = app.try_state::<Store>().map(|store| store.inner().clone());
                    let staging = app.state::<Staging>();
                    responder.respond(protocol::handle(store.as_ref(), &staging, &uri).await);
                });
            },
        )
        .manage(ChatEngine::default())
        .manage(Staging::default())
        .manage(desktop::Desktop::default())
        .manage(Dictation::new())
        .manage(proofread::Proofreader::default())
        .manage(proofread::selection::Selection::new())
        .manage(study::StudyEngine::default())
        .manage(tts::ReadAloud::new())
        .invoke_handler(tauri::generate_handler![
            commands::account::account_sign_in,
            commands::account::account_sign_out,
            commands::account::account_status,
            commands::account::cloud_status,
            commands::attachments::pick_files,
            commands::attachments::stage_bytes,
            commands::attachments::stage_clipboard_image,
            commands::attachments::stage_paths,
            commands::attachments::unstage,
            #[cfg(feature = "e2e")]
            commands::attachments::e2e_allow_paths,
            commands::chats::cancel_message,
            commands::chats::delete_chat,
            commands::chats::get_chat,
            commands::chats::list_chats,
            commands::chats::rename_chat,
            commands::chats::send_message,
            commands::chats::set_chat_favorite,
            commands::desktop::menu_entry_status,
            commands::desktop::set_menu_entry,
            commands::dictation::dictation_bind_shortcut,
            commands::dictation::dictation_cancel,
            commands::dictation::dictation_configure_shortcut,
            commands::dictation::dictation_copy_last,
            commands::dictation::dictation_info,
            commands::dictation::dictation_status,
            commands::dictation::dictation_toggle,
            commands::dictation::get_dictation_settings,
            commands::dictation::speech_cancel_download,
            commands::dictation::speech_delete_model,
            commands::dictation::speech_dismiss_download,
            commands::dictation::speech_download,
            commands::dictation::speech_downloads,
            commands::dictation::speech_models,
            commands::dictation::update_dictation_settings,
            commands::tts::read_aloud,
            commands::tts::read_aloud_state,
            commands::tts::read_aloud_stop,
            commands::tts::read_aloud_study,
            commands::tts::voice_cancel_download,
            commands::tts::voice_delete_model,
            commands::tts::voice_dismiss_download,
            commands::tts::voice_download,
            commands::tts::voice_downloads,
            commands::tts::voice_models,
            commands::easy_link::easy_link_cancel_pairing,
            commands::easy_link::easy_link_connect,
            commands::easy_link::easy_link_disconnect,
            commands::easy_link::easy_link_pair,
            commands::easy_link::easy_link_set_enabled,
            commands::easy_link::easy_link_state,
            commands::easy_link::easy_link_unlink,
            commands::hub::hub_browse,
            commands::hub::hub_cancel_download,
            commands::hub::hub_pause_download,
            commands::hub::hub_resume_download,
            commands::hub::hub_prioritize_download,
            commands::hub::hub_delete_model,
            commands::hub::hub_dismiss_download,
            commands::hub::hub_download,
            commands::hub::hub_downloads,
            commands::hub::hub_hardware,
            commands::hub::hub_home,
            commands::hub::hub_installed,
            commands::hub::hub_readme,
            commands::hub::hub_repo,
            commands::hub::hub_search,
            commands::models::get_think_pref,
            commands::models::list_models,
            commands::models::load_model,
            commands::models::model_details,
            commands::models::model_exists_upstream,
            commands::models::outdated_models,
            commands::models::recommended_models,
            commands::models::running_models,
            commands::models::set_think_pref,
            commands::models::unload_all_models,
            commands::models::unload_model,
            commands::models_dir::change_builtin_models_dir,
            commands::models_dir::change_models_location,
            commands::models_dir::check_models_folder,
            commands::models_dir::fix_ollama_home,
            commands::models_dir::get_models_location,
            commands::models_dir::get_ollama_home,
            commands::models_dir::pick_models_folder,
            commands::platform::app_platform,
            commands::proofread::proofread_cancel,
            commands::proofread::proofread_cancel_all,
            commands::proofread::proofread_last_selection,
            commands::proofread::proofread_run,
            commands::proofread::shortcuts_bind,
            commands::proofread::shortcuts_configure,
            commands::proofread::shortcuts_recording,
            commands::proofread::shortcuts_reset_trigger,
            commands::proofread::shortcuts_set_trigger,
            commands::proofread::shortcuts_status,
            commands::server::ollama_runtime,
            commands::server::open_builtin_folder,
            commands::server::open_builtin_log,
            commands::server::restart_builtin_ollama,
            commands::server::server_status,
            commands::server::set_ollama_source,
            commands::server::start_ollama_service,
            commands::settings::get_settings,
            commands::settings::ui_language,
            commands::settings::update_settings,
            commands::study::study_answer,
            commands::study::study_ask,
            commands::study::study_clarify,
            commands::study::study_create,
            commands::study::study_delete,
            commands::study::study_explain,
            commands::study::study_get,
            commands::study::study_list,
            commands::study::study_notes,
            commands::study::study_pause,
            commands::study::study_progress,
            commands::study::study_regenerate,
            commands::study::study_remedy,
            commands::study::study_replan,
            commands::study::study_retry,
            commands::study::study_save_notes,
            commands::study::study_skip,
            commands::study::study_start,
            commands::study::study_stream_cancel,
            commands::system::open_external,
            commands::system::quit_app,
            commands::updates::app_versions,
            commands::updates::check_for_updates,
            commands::updates::download_update,
            commands::updates::restart_to_update,
            commands::updates::set_auto_download,
            commands::updates::update_status,
            commands::voice_mode::get_voice_mode_settings,
            commands::voice_mode::update_voice_mode_settings,
            commands::voice_mode::voice_mode_end_turn,
            commands::voice_mode::voice_mode_hold,
            commands::voice_mode::voice_mode_info,
            commands::voice_mode::voice_mode_interrupt,
            commands::voice_mode::voice_mode_mute,
            commands::voice_mode::voice_mode_reply_failed,
            commands::voice_mode::voice_mode_requirements,
            commands::voice_mode::voice_mode_resume,
            commands::voice_mode::voice_mode_start,
            commands::voice_mode::voice_mode_state,
            commands::voice_mode::voice_mode_stop,
            #[cfg(feature = "e2e")]
            context_menu::e2e::e2e_last_context_menu,
            startup::database_problem,
            startup::recover_database,
        ])
        .build(context)
        .expect("error while building ChatonLM")
        .run(|app, event| {
            // Fermeture : services arrêtés proprement (voir `stop_services`),
            // puis le processus web de WebKit (sinon il survit parfois
            // plusieurs secondes à l'app).
            if let RunEvent::ExitRequested { .. } = event {
                tauri::async_runtime::block_on(stop_services(app));
                desktop::terminate_web_processes(app);
            }
        });
}

/// Arrête proprement les services, à la fermeture et avant d'installer une
/// mise à jour (« Restart to update ») : les cours (partie en cours gardée
/// « en cours », reprise au lancement suivant) et les corrections
/// s'arrêtent ; les réponses en cours s'arrêtent et s'enregistrent ; les
/// téléchargements du ModelHub ferment leur requête (Ollama garde les
/// morceaux : un nouveau clic reprend), comme ceux des modèles de
/// transcription (`.part` gardés) ; la séance du mode vocal finit (micro
/// fermé) ; la dictée en cours est abandonnée et son moteur libéré ; puis
/// l'Ollama intégré s'arrête pour de bon (SIGTERM à son
/// groupe, SIGKILL après 5 s, attente bornée ; plus aucune relance : un .deb
/// peut remplacer ses fichiers). Rien ne redémarre ensuite : l'app quitte ou
/// se relance. Appelée deux fois de suite (mise à jour puis fermeture) : la
/// seconde ne trouve plus rien à arrêter.
pub async fn stop_services(app: &AppHandle) {
    if let Some(voice_mode) = app.try_state::<voice_mode::VoiceMode>() {
        voice_mode.shutdown();
    }
    // Easy Link : plus d'écoute ni de nouvelle réponse ; celles en cours
    // s'arrêtent avec le moteur de chat, plus bas.
    if let Some(easy_link) = app.try_state::<easy_link::EasyLink>() {
        easy_link.shutdown();
    }
    stop_ollama_work(app);
    let engine = app.state::<ChatEngine>();
    let downloads = app.try_state::<HubDownloads>();
    let speech_downloads = app.try_state::<SpeechDownloads>();
    let voice_downloads = app.try_state::<tts::VoiceDownloads>();
    let dictation = app.state::<Dictation>();
    let downloads = async {
        if let Some(downloads) = downloads {
            downloads.stop_all(EXIT_GRACE).await;
        }
    };
    let speech_downloads = async {
        if let Some(downloads) = speech_downloads {
            downloads.stop_all(EXIT_GRACE).await;
        }
    };
    let voice_downloads = async {
        if let Some(downloads) = voice_downloads {
            downloads.stop_all(EXIT_GRACE).await;
        }
    };
    app.state::<tts::ReadAloud>().shutdown();
    tokio::join!(
        engine.stop_all(EXIT_GRACE),
        downloads,
        speech_downloads,
        voice_downloads,
        dictation.shutdown()
    );
    ollama::builtin::shutdown(app).await;
}
