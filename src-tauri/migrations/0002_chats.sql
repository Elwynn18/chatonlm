-- Conversations, messages et pièces jointes. Les dates sont des millisecondes
-- depuis l'époque Unix (UTC).

CREATE TABLE chats (
    -- UUID v7 : l'ordre alphabétique suit l'ordre de création.
    id         TEXT    PRIMARY KEY,
    -- Vide tant que la conversation n'a pas été renommée.
    title      TEXT    NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    -- Dernière activité (nouveau message) ; le renommage ne la change pas.
    updated_at INTEGER NOT NULL
);
CREATE INDEX chats_by_activity ON chats (updated_at DESC, id DESC);

CREATE TABLE messages (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id             TEXT    NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
    role                TEXT    NOT NULL CHECK (role IN ('user', 'assistant')),
    content             TEXT    NOT NULL DEFAULT '',
    thinking            TEXT    NOT NULL DEFAULT '',
    -- Modèle qui a produit la réponse (messages de l'assistant).
    model               TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    thinking_started_at INTEGER,
    thinking_ended_at   INTEGER
);
CREATE INDEX messages_by_chat ON messages (chat_id, id);

CREATE TABLE attachments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    filename   TEXT    NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('image', 'text')),
    mime       TEXT    NOT NULL DEFAULT '',
    size       INTEGER NOT NULL CHECK (size >= 0),
    -- Contenu d'origine du fichier.
    data       BLOB    NOT NULL,
    -- Texte extrait (fichiers texte, code, PDF), calculé une seule fois.
    text       TEXT
);
CREATE INDEX attachments_by_message ON attachments (message_id);
