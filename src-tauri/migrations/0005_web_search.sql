-- Recherche web (outils web_search et web_fetch exécutés par le serveur
-- Ollama).

-- Appels d'outils d'une réponse de l'assistant, dans l'ordre, avec leur
-- résultat (écart à l'officiel, qui enregistre les résultats en messages
-- « tool » séparés ; l'historique envoyé au modèle est le même).
CREATE TABLE tool_calls (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    -- Arguments donnés par le modèle : objet JSON (texte compact).
    arguments  TEXT    NOT NULL,
    -- JSON renvoyé par l'outil ; NULL = pas de résultat.
    result     TEXT,
    -- Erreur de l'outil (sans « Error: ») ; NULL sinon. Ni résultat ni erreur :
    -- appel interrompu (arrêt, app fermée), jamais renvoyé au modèle.
    error      TEXT,
    UNIQUE (message_id, position)
);

-- Bouton de recherche web du composer, gardé d'un lancement à l'autre ;
-- « Reset to defaults » n'y touche pas.
ALTER TABLE settings ADD COLUMN web_search_enabled INTEGER NOT NULL DEFAULT 0
    CHECK (web_search_enabled IN (0, 1));
