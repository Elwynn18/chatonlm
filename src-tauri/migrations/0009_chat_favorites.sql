-- Conversations favorites (propre à ChatonLM) : rubrique « Favorites » en tête
-- de l'historique. Ne change pas la date d'activité.
ALTER TABLE chats ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0
    CHECK (favorite IN (0, 1));
