-- Réglages de l'application : une seule ligne.
CREATE TABLE settings (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    -- Adresse du serveur Ollama ; vide = OLLAMA_HOST, sinon http://localhost:11434.
    server_url     TEXT    NOT NULL DEFAULT '',
    -- Longueur de contexte envoyée en options.num_ctx ; 0 = défaut du serveur.
    context_length INTEGER NOT NULL DEFAULT 0 CHECK (context_length >= 0),
    theme          TEXT    NOT NULL DEFAULT 'system'
                   CHECK (theme IN ('system', 'light', 'dark')),
    -- Dernier modèle choisi ; vide = aucun.
    selected_model TEXT    NOT NULL DEFAULT '',
    -- Taille de la fenêtre, mémorisée à la fermeture ; NULL = taille par défaut.
    window_width   INTEGER CHECK (window_width > 0),
    window_height  INTEGER CHECK (window_height > 0)
);

INSERT INTO settings (id) VALUES (1);
