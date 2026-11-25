//! Modèle d'un projet de l'Atelier, choisi au lancement (conception § 5 et
//! § 4.5) : le modèle demandé ; sinon le plus gros modèle de texte installé
//! qui tient sur la carte à [`PROJECT_CONTEXT`] jetons
//! (`engine::auto::largest_fitting`) ; sinon celui du chat s'il peut servir ;
//! sinon le plus gros modèle de texte installé. Les
//! avertissements (`engine::auto::size_warning`) suivent le modèle retenu,
//! sans jamais bloquer.

use crate::engine::auto::{ModelWarning, Pick, size_warning, total_parameters};
use crate::local_models::LocalModel;
use crate::local_models::id::same_model;

/// Contexte visé pour un projet (fiches de 8 000 à 16 000 jetons, plus la
/// réponse).
pub(crate) const PROJECT_CONTEXT: u32 = 32_768;

/// Contexte possible le plus petit accepté : en deçà, les consignes d'une
/// partie (jusqu'à 16 000 jetons), la réflexion et la réponse ne tiennent
/// pas (code `contextTooSmall`).
pub(crate) const MIN_CONTEXT: u32 = 16_384;

/// Modèle retenu pour un lancement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelChoice {
    pub id: String,
    /// Contexte demandé à l'ouverture.
    pub ctx: u32,
    pub warnings: Vec<ModelWarning>,
    /// Choisi par l'Atelier (aucun modèle demandé).
    pub auto: bool,
}

/// Pourquoi aucun modèle (codes : `noModel`, `modelNotFound`, `remoteOff`,
/// `contextTooSmall`).
pub(crate) type ChoiceError = &'static str;

/// Avertissements d'un modèle installé.
fn local_warnings(model: &LocalModel) -> Vec<ModelWarning> {
    size_warning(
        model.params_label.as_deref().and_then(total_parameters),
        model.context_max.unwrap_or(0),
        model.capabilities.thinking,
    )
}

/// Modèle installé (ou distant) `id` tel que l'Atelier le prendrait.
fn chosen(
    id: &str,
    installed: &[LocalModel],
    remote_on: bool,
    auto: bool,
) -> Result<ModelChoice, ChoiceError> {
    if crate::llm::ids::is_remote(id) {
        if !remote_on {
            return Err("remoteOff");
        }
        // Taille et contexte inconnus ; la réflexion se découvre à
        // l'ouverture (rien n'est dit ici).
        return Ok(ModelChoice {
            id: id.to_owned(),
            ctx: PROJECT_CONTEXT,
            warnings: size_warning(None, 0, true),
            auto,
        });
    }
    let model = installed
        .iter()
        .find(|model| same_model(&model.id, id))
        .ok_or("modelNotFound")?;
    let ctx = match model.context_max.filter(|max| *max > 0) {
        Some(max) if max < MIN_CONTEXT => return Err("contextTooSmall"),
        Some(max) => PROJECT_CONTEXT.min(max),
        None => PROJECT_CONTEXT,
    };
    Ok(ModelChoice {
        id: model.id.clone(),
        ctx,
        warnings: local_warnings(model),
        auto,
    })
}

/// Choix pur : `requested` (non vide) ; sinon `fitting` (le plus gros qui
/// tient, déjà calculé) ; sinon `chat` (modèle du chat) ; sinon `noModel`.
pub(crate) fn decide(
    requested: &str,
    installed: &[LocalModel],
    fitting: Option<&Pick>,
    chat: &str,
    remote_on: bool,
) -> Result<ModelChoice, ChoiceError> {
    let requested = requested.trim();
    if !requested.is_empty() {
        return chosen(requested, installed, remote_on, false);
    }
    if let Some(pick) = fitting {
        let mut choice = chosen(&pick.id, installed, remote_on, true)?;
        if pick.ctx < MIN_CONTEXT {
            return Err("contextTooSmall");
        }
        choice.ctx = pick.ctx;
        return Ok(choice);
    }
    // Modèle du chat seulement s'il peut servir : un nom resté d'une version
    // précédente, désinstallé, ou un modèle cloud désactivé ne doit pas
    // bloquer Auto quand un autre modèle est installé.
    let chat = chat.trim();
    let chat_usable = !chat.is_empty()
        && if crate::llm::ids::is_remote(chat) {
            remote_on
        } else {
            installed.iter().any(|model| same_model(&model.id, chat))
        };
    let mut refusal = None;
    if chat_usable {
        match chosen(chat, installed, remote_on, true) {
            Ok(choice) => return Ok(choice),
            Err(code) => refusal = Some(code),
        }
    }
    match largest_installed(installed) {
        Some(model) => chosen(&model.id, installed, remote_on, true),
        // Le modèle du chat dit pourquoi il ne sert pas (contexte trop petit).
        None => Err(refusal.unwrap_or("noModel")),
    }
}

/// Plus gros modèle de texte installé qui tient assez de contexte (rang :
/// paramètres annoncés, sinon estimés d'après le fichier), quand aucun ne
/// tient sur la carte et que le modèle du chat ne peut pas servir.
fn largest_installed(installed: &[LocalModel]) -> Option<&LocalModel> {
    use crate::engine::auto::{estimated_parameters, writes_text};
    installed
        .iter()
        .filter(|model| writes_text(model))
        .filter(|model| {
            model
                .context_max
                .is_none_or(|max| max == 0 || max >= MIN_CONTEXT)
        })
        .map(|model| {
            let rank = model
                .params_label
                .as_deref()
                .and_then(total_parameters)
                .or_else(|| estimated_parameters(model))
                .unwrap_or(0.0);
            (model, rank)
        })
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(model, _)| model)
}

/// [`decide`] sur la base et le moteur de l'app.
pub(crate) async fn choose(
    access: &crate::llm::Access,
    requested: &str,
) -> Result<ModelChoice, ChoiceError> {
    let installed = crate::local_models::list(access.store(), access.data_dir())
        .await
        .unwrap_or_default();
    let settings = access
        .store()
        .call(|c| crate::store::settings::get(c))
        .await
        .map_err(|_| "noModel")?;
    let fitting = if requested.trim().is_empty() {
        match access.engine().machine().await {
            Some(machine) => {
                crate::engine::auto::largest_fitting(&installed, &machine, PROJECT_CONTEXT).await
            }
            None => None,
        }
    } else {
        None
    };
    decide(
        requested,
        &installed,
        fitting.as_ref(),
        &settings.selected_model,
        settings.remote_models,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::{ModelCapabilities, ModelSource};

    fn local(id: &str, params: &str, context: u32, thinking: bool) -> LocalModel {
        let (repo, quant) = id.rsplit_once(':').unwrap();
        LocalModel {
            id: id.into(),
            display_name: repo.into(),
            repo: repo.into(),
            quant: quant.into(),
            path: format!("/m/{repo}/{quant}.gguf"),
            mmproj: None,
            size_bytes: 1,
            context_max: Some(context),
            capabilities: ModelCapabilities {
                thinking,
                ..ModelCapabilities::default()
            },
            arch: Some("qwen3".into()),
            params_label: Some(params.into()),
            installed_at: 0,
            source: ModelSource::Hf,
        }
    }

    fn installed() -> Vec<LocalModel> {
        vec![
            local("big/m:Q4_K_M", "32B", 131_072, true),
            local("tiny/m:Q8_0", "0.6B", 8_192, false),
            local("mid/m:Q8_0", "0.6B", 20_000, false),
        ]
    }

    fn pick(id: &str, ctx: u32) -> Pick {
        Pick {
            id: id.into(),
            params: None,
            ctx,
            estimated_bytes: 0,
        }
    }

    #[test]
    fn the_requested_model_wins_with_its_warnings() {
        let choice = decide(
            " mid/m:Q8_0 ",
            &installed(),
            Some(&pick("big/m:Q4_K_M", 32_768)),
            "big/m:Q4_K_M",
            false,
        )
        .unwrap();
        assert_eq!(choice.id, "mid/m:Q8_0");
        assert!(!choice.auto);
        // Contexte borné à celui du modèle.
        assert_eq!(choice.ctx, 20_000);
        assert_eq!(
            choice.warnings,
            [ModelWarning::TooSmall, ModelWarning::NoThinking]
        );
    }

    /// Relecture du lot 2 : un modèle dont le contexte possible est sous
    /// 16 384 jetons est refusé au lancement, avec son code (dit par
    /// `model_check`), qu'il soit demandé, choisi par l'Atelier ou celui du
    /// chat.
    #[test]
    fn a_model_with_too_small_a_context_is_refused() {
        let installed = installed();
        assert_eq!(
            decide("tiny/m:Q8_0", &installed, None, "", false),
            Err("contextTooSmall")
        );
        // Celui du chat ne sert pas : un autre installé le remplace ; aucun
        // autre : son refus.
        assert_eq!(
            decide("", &installed, None, "tiny/m:Q8_0", false).map(|c| c.id),
            Ok("big/m:Q4_K_M".to_owned())
        );
        assert_eq!(
            decide("", &installed[1..2], None, "tiny/m:Q8_0", false),
            Err("contextTooSmall")
        );
        assert_eq!(
            decide(
                "",
                &installed,
                Some(&pick("big/m:Q4_K_M", 8_192)),
                "",
                false
            ),
            Err("contextTooSmall")
        );
        assert!(
            decide(
                "",
                &installed,
                Some(&pick("big/m:Q4_K_M", 16_384)),
                "",
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn without_a_request_the_largest_fitting_model_is_taken() {
        let choice = decide(
            "",
            &installed(),
            Some(&pick("big/m:Q4_K_M", 32_768)),
            "tiny/m:Q8_0",
            false,
        )
        .unwrap();
        assert_eq!(choice.id, "big/m:Q4_K_M");
        assert!(choice.auto);
        assert_eq!(choice.ctx, 32_768);
        assert!(choice.warnings.is_empty());
    }

    #[test]
    fn without_a_fitting_model_the_chat_model_is_taken() {
        let choice = decide("", &installed(), None, "mid/m:Q8_0", false).unwrap();
        assert_eq!(choice.id, "mid/m:Q8_0");
        assert!(choice.auto);
        assert_eq!(decide("", &[], None, " ", false), Err("noModel"));
    }

    #[test]
    fn a_chat_model_that_cannot_serve_gives_way_to_the_largest_installed() {
        // Ancien nom (d'avant la 0.10), cloud désactivé, ou aucun : le plus
        // gros modèle installé qui tient assez de contexte, jamais un refus.
        for chat in [
            "hf.co/unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_XL",
            "@remote/mistral/mistral-large",
            "",
        ] {
            let choice = decide("", &installed(), None, chat, false).unwrap();
            assert_eq!(choice.id, "big/m:Q4_K_M", "{chat}");
            assert!(choice.auto);
        }
        // Le plus gros a trop peu de contexte : le suivant qui en a assez.
        let small_context = vec![
            local("tiny/m:Q8_0", "0.6B", 8_192, false),
            local("mid/m:Q8_0", "0.6B", 20_000, false),
        ];
        let choice = decide("", &small_context, None, "gone/m:Q4_0", false).unwrap();
        assert_eq!(choice.id, "mid/m:Q8_0");
        // Rien d'installé : aucun modèle, pas « plus installé ».
        assert_eq!(decide("", &[], None, "gone/m:Q4_0", false), Err("noModel"));
    }

    #[test]
    fn a_missing_or_remote_model_is_refused_with_its_code() {
        assert_eq!(
            decide("gone/m:Q4_0", &installed(), None, "", false),
            Err("modelNotFound")
        );
        let remote = "@remote/mistral/mistral-large";
        assert_eq!(
            decide(remote, &installed(), None, "", false),
            Err("remoteOff")
        );
        let choice = decide(remote, &installed(), None, "", true).unwrap();
        assert_eq!(choice.warnings, [ModelWarning::UnknownSize]);
    }
}
