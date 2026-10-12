-- Lecture à voix haute (fonction propre à ChatonLM) : lire chaque nouvelle
-- réponse pendant qu'elle s'écrit. Coupée par défaut ; « Reset to defaults »
-- la coupe.
ALTER TABLE settings ADD COLUMN auto_read INTEGER NOT NULL DEFAULT 0
    CHECK (auto_read IN (0, 1));
