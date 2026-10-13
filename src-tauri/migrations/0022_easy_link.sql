-- Easy Link (propre à ChatonLM) : un bot Telegram ou Discord relie une
-- messagerie au modèle de l'app.

-- Un bot par messagerie. Le jeton reste dans cette base : il n'est jamais
-- renvoyé à la page.
CREATE TABLE easy_link_bots (
    platform TEXT PRIMARY KEY CHECK (platform IN ('telegram', 'discord')),
    token    TEXT NOT NULL DEFAULT '',
    enabled  INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    -- Identité donnée par la messagerie à la dernière connexion réussie.
    bot_id   TEXT NOT NULL DEFAULT '',
    bot_name TEXT NOT NULL DEFAULT ''
);

INSERT INTO easy_link_bots (platform) VALUES ('telegram'), ('discord');

-- Comptes qui ont le droit de parler au bot (liés par un code d'appairage).
CREATE TABLE easy_link_accounts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    platform   TEXT NOT NULL CHECK (platform IN ('telegram', 'discord')),
    -- Identifiant du compte dans la messagerie.
    user_id    TEXT NOT NULL,
    name       TEXT NOT NULL DEFAULT '',
    -- Conversation de l'app où vont ses messages ; NULL : la prochaine en
    -- ouvre une nouvelle (`/new`, ou conversation supprimée dans l'app).
    chat_id    TEXT REFERENCES chats (id) ON DELETE SET NULL,
    -- Modèle choisi par `/model` ; vide : celui de l'app.
    model      TEXT NOT NULL DEFAULT '',
    web_search INTEGER NOT NULL DEFAULT 0 CHECK (web_search IN (0, 1)),
    linked_at  INTEGER NOT NULL,
    UNIQUE (platform, user_id)
);

-- Origine d'une conversation ; NULL : écrite dans l'app.
ALTER TABLE chats ADD COLUMN source TEXT CHECK (source IN ('telegram', 'discord'));
