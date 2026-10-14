-- Contexte occupé par conversation (propre à ChatonLM) : jetons que la
-- conversation occupe, jusqu'au message `context_message_id` compris (prompt
-- de la dernière requête finie et texte de sa réponse, sans le raisonnement
-- que le modèle ne relit pas), et taille du contexte du modèle à ce moment ;
-- NULL = inconnu.
ALTER TABLE chats ADD COLUMN context_used INTEGER;
ALTER TABLE chats ADD COLUMN context_size INTEGER;
ALTER TABLE chats ADD COLUMN context_message_id INTEGER;

-- Compactages : résumé, écrit par le modèle, des messages jusqu'à
-- `through_message_id` compris ; une requête n'envoie que le dernier résumé
-- et les messages qui le suivent. Les messages restent affichés.
CREATE TABLE compactions (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id            TEXT    NOT NULL REFERENCES chats (id) ON DELETE CASCADE,
    through_message_id INTEGER NOT NULL,
    summary            TEXT    NOT NULL,
    automatic          INTEGER NOT NULL,
    created_at         INTEGER NOT NULL
);
CREATE INDEX compactions_by_chat ON compactions (chat_id, id);
