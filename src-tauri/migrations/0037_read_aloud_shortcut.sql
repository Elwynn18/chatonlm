-- Raccourci global « Read selected text aloud » (propre à ChatonLM).
-- read_aloud_shortcut : raccourci demandé (lié par le portail du bureau,
-- enregistré auprès de Windows) ; coupé par défaut, comme celui de la
-- correction : la première liaison ouvre une boîte du bureau.
ALTER TABLE settings ADD COLUMN read_aloud_shortcut INTEGER NOT NULL DEFAULT 0
    CHECK (read_aloud_shortcut IN (0, 1));
-- Combinaison choisie dans ChatonLM (Windows) ; NULL : Ctrl+Alt+R.
ALTER TABLE desktop_settings ADD COLUMN read_aloud_keys TEXT;
