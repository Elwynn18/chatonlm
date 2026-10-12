-- Dictée : texte copié automatiquement dans le presse-papiers (défaut) ;
-- sinon, la notification de fin propose « Copy ».
ALTER TABLE dictation_settings
    ADD COLUMN auto_copy INTEGER NOT NULL DEFAULT 1 CHECK (auto_copy IN (0, 1));
