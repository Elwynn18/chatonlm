-- Ollama intégré (étape 18b, propre à ChatonLM) : une seule ligne, à part de
-- la table settings (« Reset to defaults » n'y touche jamais).
CREATE TABLE builtin_ollama (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    -- Serveur utilisé : « auto » (celui de la machine ou de l'adresse réglée,
    -- l'intégré seulement quand rien n'est installé), « builtin » (toujours
    -- l'intégré, choisi par l'utilisateur).
    source     TEXT    NOT NULL DEFAULT 'auto' CHECK (source IN ('auto', 'builtin')),
    -- Dossier des modèles de l'intégré ; NULL : <données de l'app>/ollama/models.
    models_dir TEXT
);

INSERT INTO builtin_ollama (id) VALUES (1);
