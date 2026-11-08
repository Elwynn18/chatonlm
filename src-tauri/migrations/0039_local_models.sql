-- Base locale des modèles de langage (passage d'Ollama à llama-server, propre à
-- ChatonLM) : une ligne par modèle installé dans le dossier des modèles de
-- l'app, écrite à la fin d'un téléchargement (empreintes vérifiées, en-tête GGUF
-- lu). Identifiant : « <auteur>/<dépôt>:<QUANT> » pour Hugging Face,
-- « @ollama/<nom>:<étiquette> » pour un modèle importé d'un Ollama.
CREATE TABLE local_models (
    id           TEXT    PRIMARY KEY NOT NULL,
    source       TEXT    NOT NULL CHECK (source IN ('hf', 'ollama')),
    -- « auteur/dépôt » (Hugging Face) ; nom du modèle importé.
    repo         TEXT    NOT NULL,
    -- Commit du dépôt au début du téléchargement ; NULL pour un importé.
    revision     TEXT,
    quant        TEXT    NOT NULL,
    -- Tableau JSON des chemins, relatifs au dossier des modèles, morceaux
    -- (« -0000N-of-0000M ») dans l'ordre.
    files        TEXT    NOT NULL,
    -- Projecteur de vision, relatif au dossier des modèles ; NULL sans vision.
    mmproj       TEXT,
    size_bytes   INTEGER NOT NULL DEFAULT 0,
    -- Objet JSON chemin relatif → SHA-256 (hexadécimal), projecteur compris.
    sha256       TEXT    NOT NULL DEFAULT '{}',
    -- Métadonnées lues dans l'en-tête GGUF à l'installation.
    arch         TEXT,
    context_max  INTEGER,
    params_label TEXT,
    file_type    INTEGER,
    -- Objet JSON {vision, tools, thinking, fim}.
    capabilities TEXT    NOT NULL DEFAULT '{}',
    -- ms depuis 1970 ; ordre des listes (plus récent d'abord).
    installed_at INTEGER NOT NULL
);

-- Réglages du moteur (une seule ligne, à part de `settings` : « Reset to
-- defaults » n'y touche pas). models_dir NULL : <données de l'app>/models.
CREATE TABLE engine_settings (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    models_dir TEXT
);

INSERT INTO engine_settings (id) VALUES (1);
