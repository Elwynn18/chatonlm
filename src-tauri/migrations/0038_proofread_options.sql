-- Options de la correction (menu de la page Proofread, propres à ChatonLM),
-- valables aussi pour le raccourci global et le lien VS Code.
-- proofread_keep_order : mots gardés dans leur ordre, seule leur écriture est
-- corrigée ; coupé par défaut.
ALTER TABLE settings ADD COLUMN proofread_keep_order INTEGER NOT NULL DEFAULT 0
    CHECK (proofread_keep_order IN (0, 1));
-- proofread_formatting : mise en forme automatique (retours à la ligne souples
-- joints, majuscule au début, point à la fin) ; activée par défaut.
ALTER TABLE settings ADD COLUMN proofread_formatting INTEGER NOT NULL DEFAULT 1
    CHECK (proofread_formatting IN (0, 1));
-- proofread_digits : nombres écrits en chiffres gardés en chiffres ; coupé
-- par défaut.
ALTER TABLE settings ADD COLUMN proofread_digits INTEGER NOT NULL DEFAULT 0
    CHECK (proofread_digits IN (0, 1));
