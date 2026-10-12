-- Chargement des modèles (onglet Models des réglages, propre à ChatonLM).
-- keep_alive : durée en secondes envoyée en `keep_alive` avec chaque requête
-- de ChatonLM qui charge un modèle ; 0 = rien envoyé (défaut du serveur),
-- -1 = garder chargé jusqu'au déchargement.
ALTER TABLE settings ADD COLUMN keep_alive INTEGER NOT NULL DEFAULT 0
    CHECK (keep_alive >= -1);
-- Préchargement du modèle choisi dans le chat (au choix et au démarrage).
ALTER TABLE settings ADD COLUMN preload_model INTEGER NOT NULL DEFAULT 0
    CHECK (preload_model IN (0, 1));
