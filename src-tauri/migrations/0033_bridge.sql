-- Lien VS Code (propre à ChatonLM) : pont local entre l'app et l'extension
-- ChatonLM pour VS Code. Coupé par défaut ; « Reset to defaults » n'y touche
-- pas. Un éditeur appairé garde un secret partagé (32 octets) qui sert à
-- prouver chaque connexion dans les deux sens (HMAC) ; il ne passe jamais sur
-- le fil après l'appairage et n'est jamais montré à la page.
CREATE TABLE bridge_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1))
);
INSERT INTO bridge_settings (id) VALUES (1);

CREATE TABLE bridge_clients (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret BLOB NOT NULL CHECK (length(secret) = 32),
    paired_at INTEGER NOT NULL,
    last_seen_at INTEGER
);
