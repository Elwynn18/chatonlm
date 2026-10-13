-- Raccourci « Correct selected text » : coller le texte corrigé à la place de
-- la sélection (Ctrl+V envoyé à l'application au premier plan), au lieu de le
-- laisser dans le presse-papiers. Activé par défaut ; se règle dans
-- Settings > Keyboard shortcuts.
ALTER TABLE settings ADD COLUMN proofread_paste INTEGER NOT NULL DEFAULT 1;
