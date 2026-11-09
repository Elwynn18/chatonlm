-- Reprise des modèles des Ollama de ChatonLM 0.9.x (passage à llama-server,
-- propre à ChatonLM) : ce qui a été repris, de l'Ollama intégré ou de celui de
-- la machine, et sous quel identifiant de la base locale (`local_models`).
-- Sert de table de correspondance ancien nom → identifiant : les réglages qui
-- nomment un modèle (`settings.selected_model`, `proofread_model`,
-- `study_model`, `dictation_settings.polish_model`, `model_prefs`,
-- `model_traits`, `easy_link_accounts.model`) sont réécrits d'après elle ;
-- l'historique (`messages.model`, `courses.model`) garde l'ancien nom.
CREATE TABLE ollama_imports (
    -- 'builtin' : Ollama intégré de ChatonLM ; 'machine' : Ollama de la machine.
    origin     TEXT    NOT NULL CHECK (origin IN ('builtin', 'machine')),
    -- Nom tel qu'Ollama l'écrit (« qwen3:0.6b », « hf.co/u/R-GGUF:Q4_K_M »).
    name       TEXT    NOT NULL,
    -- Clé de comparaison du nom : minuscules, sans hôte ni « library/ » du
    -- registre d'Ollama, avec « :latest » s'il n'a pas d'étiquette.
    name_key   TEXT    NOT NULL,
    -- Identifiant dans `local_models` ; NULL : pas repris.
    model_id   TEXT,
    -- 'imported' : repris ; 'redownload' : format propre à Ollama que
    -- llama.cpp ne charge pas (à retélécharger, équivalent proposé dans
    -- `suggestion`) ; 'failed' : échec de la reprise (`detail`).
    status     TEXT    NOT NULL CHECK (status IN ('imported', 'redownload', 'failed')),
    suggestion TEXT,
    detail     TEXT,
    -- ms depuis 1970.
    at         INTEGER NOT NULL,
    -- Échecs de suite (remis à zéro par une réussite) : au deuxième, le modèle
    -- n'est plus repris automatiquement au lancement (seulement à la demande).
    attempts   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (origin, name_key)
);

-- Réglages par modèle repris d'Ollama : couche `params` (JSON : température,
-- top_k, top_p, min_p, pénalités, arrêts, contexte…) et consigne `system` du
-- modèle. Clé : identifiant de `local_models`.
CREATE TABLE model_defaults (
    model  TEXT PRIMARY KEY NOT NULL,
    params TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(params)),
    system TEXT NOT NULL DEFAULT ''
);

-- Reprise automatique des modèles de l'Ollama intégré, au premier lancement :
-- 'pending' tant qu'elle n'a pas abouti.
ALTER TABLE engine_settings ADD COLUMN builtin_import TEXT NOT NULL DEFAULT 'pending'
    CHECK (builtin_import IN ('pending', 'done'));

-- L'Ollama intégré range ses modèles dans un dossier de l'Ollama de la
-- machine (0.9.x acceptait de le régler sur `~/.ollama/models`…) : sa reprise
-- n'y fait que des copies, et rien n'y est jamais effacé. Reste vrai une fois
-- vu (lot L6, relecture B1).
ALTER TABLE engine_settings ADD COLUMN builtin_shared INTEGER NOT NULL DEFAULT 0
    CHECK (builtin_shared IN (0, 1));

-- Changement du dossier des modèles en cours (dossier visé), écrit avant le
-- premier fichier déplacé et effacé avec le nouveau réglage : après une
-- coupure, les fichiers déjà passés dans le dossier visé sont remis dans
-- l'ancien (`local_models::import::folder::recover`).
ALTER TABLE engine_settings ADD COLUMN models_dir_moving TEXT;

-- Le dossier déplacé de l'intégré (0.9.x, Settings › Model location) n'est
-- pas repris ici : seul le code sait s'il est propre à l'intégré ou partagé
-- avec l'Ollama de la machine (`store::model_import::adopt_builtin_folder`, à
-- la première reprise).
