-- Décharger les modèles en quittant (onglet Models des réglages, propre à
-- ChatonLM) : à la fin de l'app, les modèles encore en mémoire dans l'Ollama
-- de la machine sont déchargés. Activé par défaut, bases existantes comprises.
ALTER TABLE settings ADD COLUMN unload_on_quit INTEGER NOT NULL DEFAULT 1
    CHECK (unload_on_quit IN (0, 1));
