-- Dictée : coller le texte dicté là où se trouve le curseur (Ctrl+V envoyé à
-- l'application au premier plan, après la copie dans le presse-papiers).
-- Activé par défaut ; sans effet quand la copie automatique est coupée.
ALTER TABLE dictation_settings
    ADD COLUMN auto_paste INTEGER NOT NULL DEFAULT 1 CHECK (auto_paste IN (0, 1));
