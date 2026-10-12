-- Correction (page Proofread et raccourci global, propres à ChatonLM).
-- proofread_model : modèle de la correction ; '' = celui du chat
-- (selected_model).
ALTER TABLE settings ADD COLUMN proofread_model TEXT NOT NULL DEFAULT '';
-- proofread_shortcut : raccourci global « Correct selected text » demandé
-- (lié par le portail du bureau) ; coupé par défaut : la première liaison
-- ouvre une boîte du bureau.
ALTER TABLE settings ADD COLUMN proofread_shortcut INTEGER NOT NULL DEFAULT 0
    CHECK (proofread_shortcut IN (0, 1));
