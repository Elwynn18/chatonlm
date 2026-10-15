-- Pré-prompt : message système envoyé en tête de chaque requête des
-- conversations (page, mode vocal, Easy Link), jamais enregistré dans
-- l'historique. Vide par défaut ; se règle dans Settings > Models.
ALTER TABLE settings ADD COLUMN system_prompt TEXT NOT NULL DEFAULT '';
