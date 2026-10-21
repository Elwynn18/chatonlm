-- Modèles cloud d'Ollama (onglet Models des réglages) : montrés dans le
-- sélecteur et utilisables seulement une fois activés. Coupé par défaut,
-- bases existantes comprises : rien ne part sur ollama.com sans ce choix.
ALTER TABLE settings ADD COLUMN cloud_models INTEGER NOT NULL DEFAULT 0
    CHECK (cloud_models IN (0, 1));
