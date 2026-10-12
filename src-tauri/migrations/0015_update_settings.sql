-- Mises à jour de ChatonLM (étape 18c) : une seule ligne, à part de la table
-- settings (« Reset to defaults » n'y touche jamais). Numéro 0015 : 0014 est
-- celle de l'étape 18b (Ollama intégré).
CREATE TABLE update_settings (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    -- « Auto-download updates » (Settings › General), activé par défaut
    -- comme dans l'app officielle : téléchargement en fond ; 0 : la mise à
    -- jour est seulement signalée.
    auto_download    INTEGER NOT NULL DEFAULT 1 CHECK (auto_download IN (0, 1)),
    -- Dernière version annoncée par la notification « Update available »
    -- (une fois par version, même après une relance).
    notified_version TEXT
);

INSERT INTO update_settings (id) VALUES (1);
