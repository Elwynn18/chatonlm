-- Conversations éphémères (propres à ChatonLM) : absentes de l'historique,
-- effacées quand on les quitte, à la fin de l'app et au démarrage suivant (app
-- arrêtée brutalement) ; « Keep » les fait passer dans l'historique.
ALTER TABLE chats ADD COLUMN ephemeral INTEGER NOT NULL DEFAULT 0
    CHECK (ephemeral IN (0, 1));

-- Easy Link : mode éphémère d'un compte (`/ephemeral on`) ; ses conversations
-- sont alors ouvertes éphémères.
ALTER TABLE easy_link_accounts ADD COLUMN ephemeral INTEGER NOT NULL DEFAULT 0
    CHECK (ephemeral IN (0, 1));
