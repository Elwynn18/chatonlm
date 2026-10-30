-- Lien VS Code (propre à ChatonLM) : conversation ouverte par l'extension
-- ChatonLM pour VS Code, marquée « VS Code » dans l'historique. Colonne à
-- part plutôt qu'une valeur de plus pour `source` : sa contrainte CHECK ne se
-- change qu'en reconstruisant `chats`, dont les messages dépendent.
ALTER TABLE chats ADD COLUMN editor INTEGER NOT NULL DEFAULT 0
    CHECK (editor IN (0, 1));
