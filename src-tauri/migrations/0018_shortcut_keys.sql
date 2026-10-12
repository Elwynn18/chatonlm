-- Combinaisons des raccourcis globaux choisies dans ChatonLM (Windows, où
-- aucun bureau ne les choisit à sa place) : forme « Ctrl+Alt+D » ; NULL :
-- combinaison par défaut (Ctrl+Alt+D pour la dictée, Ctrl+Alt+C pour la
-- correction). Jamais lues sous Linux, où le portail du bureau les choisit.
ALTER TABLE desktop_settings ADD COLUMN dictation_keys TEXT;
ALTER TABLE desktop_settings ADD COLUMN proofread_keys TEXT;
