-- Fournisseurs des modèles distants (contrat 102 § 5) : un par ligne,
-- compatibles avec l'API d'OpenAI. `id` : identifiant court, repris dans
-- l'identifiant des modèles (`@remote/<id>/<modèle>`) ; `base_url` : adresse
-- d'un fournisseur « custom » (les autres ont la leur). La clé d'API n'est
-- JAMAIS ici : fichier à part, lisible du seul utilisateur
-- (`<données>/remote/keys.json`).
CREATE TABLE remote_providers (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('ollamaCloud', 'openRouter', 'mistral', 'custom')),
    name TEXT NOT NULL,
    base_url TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
