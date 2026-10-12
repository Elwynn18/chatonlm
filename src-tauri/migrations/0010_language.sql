-- Langue de l'interface (Settings › General) : « system » = langue du bureau
-- (LANGUAGE, LC_ALL, LC_MESSAGES, LANG), anglais si elle n'est pas proposée.
ALTER TABLE settings ADD COLUMN language TEXT NOT NULL DEFAULT 'system'
    CHECK (language IN ('system', 'en', 'es', 'fr'));
