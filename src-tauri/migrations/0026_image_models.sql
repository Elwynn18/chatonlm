-- Modèles d'images (choix libre sur Hugging Face, contrat 83) : un modèle par
-- fichier choisi (`id` = « <dépôt>/<chemin du fichier> »), ses fichiers (le
-- modèle et les pièces annexes de sa famille : dépôt, révision, taille,
-- SHA-256) relevés au moment du choix ; les fichiers sont rangés par
-- empreinte et partagés entre modèles.
CREATE TABLE image_models (
    id TEXT PRIMARY KEY,
    repo TEXT NOT NULL,
    revision TEXT NOT NULL,
    file TEXT NOT NULL,
    family TEXT NOT NULL,
    name TEXT NOT NULL,
    license TEXT,
    uncensored INTEGER NOT NULL DEFAULT 0,
    -- Installé au moins une fois (tous ses fichiers reçus et vérifiés) : un
    -- retrait de la liste des téléchargements ne l'oublie plus.
    installed INTEGER NOT NULL DEFAULT 0,
    components TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
-- Modèle choisi pour la génération (vide : aucun).
ALTER TABLE settings ADD COLUMN image_model TEXT NOT NULL DEFAULT '';
