-- Atelier (moteur de tâches complexes, propre à ChatonLM) : projets, pièces
-- jointes, état reprenable, journal d'événements numéroté et questions posées
-- à l'utilisateur. Dates en millisecondes depuis l'époque Unix (UTC). Les
-- énumérations sont écrites comme le JSON de leur type Rust (camelCase,
-- `workshop::pipeline::types`).

CREATE TABLE workshop_projects (
    -- Numéro croissant, jamais redonné (même après une suppression).
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    title           TEXT    NOT NULL,
    task            TEXT    NOT NULL,
    -- Stratégie connue (forcée à la création, ou choisie par le projet) ;
    -- NULL : pas encore choisie.
    strategy        TEXT
        CHECK (strategy IS NULL
               OR strategy IN ('generic', 'longForm', 'research', 'documents', 'code')),
    -- La stratégie a été imposée par l'utilisateur (étape du choix sautée).
    strategy_forced INTEGER NOT NULL DEFAULT 0 CHECK (strategy_forced IN (0, 1)),
    autonomy        TEXT    NOT NULL
        CHECK (autonomy IN ('plan', 'ask', 'acceptActions', 'autonomous')),
    -- Modèle demandé ; '' : choisi au lancement (le plus gros qui tient).
    model           TEXT    NOT NULL DEFAULT '',
    -- Modèle retenu au dernier lancement ; '' : jamais lancé.
    model_used      TEXT    NOT NULL DEFAULT '',
    -- Langue de l'app à la création (code : en, es, fr).
    language        TEXT    NOT NULL,
    state           TEXT    NOT NULL
        CHECK (state IN ('queued', 'running', 'waiting', 'paused', 'yielding', 'done',
                         'failed', 'cancelled')),
    -- Phase en cours ou dernière commencée ; NULL : aucune.
    phase           TEXT
        CHECK (phase IS NULL
               OR phase IN ('collect', 'strategy', 'framing', 'spec', 'plan', 'build',
                            'assemble', 'verify', 'deliver')),
    -- Dossier du projet (chemin absolu, livrables à sa racine, travail dans
    -- `.atelier/`).
    folder          TEXT    NOT NULL,
    -- Rang dans la file (ordre d'arrivée, refait à chaque mise en file).
    queue_seq       INTEGER NOT NULL DEFAULT 0,
    -- Code de l'erreur (état 'failed').
    error           TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    finished_at     INTEGER
);
CREATE INDEX workshop_projects_by_date ON workshop_projects (created_at DESC, id DESC);

-- Pièces jointes : texte extrait à la création (copie gardée aussi dans
-- `.atelier/sources/`).
CREATE TABLE workshop_attachments (
    project_id INTEGER NOT NULL REFERENCES workshop_projects (id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    filename   TEXT    NOT NULL,
    text       TEXT    NOT NULL,
    PRIMARY KEY (project_id, position)
);

-- État reprenable écrit par le projet (stratégie, spécification, plan,
-- cartes, briques, rapport…), en JSON, par clé.
CREATE TABLE workshop_state (
    project_id INTEGER NOT NULL REFERENCES workshop_projects (id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, key)
);

-- Journal numéroté (seq croissant par projet, à partir de 1) : la page, la
-- ligne de commande et l'API le relisent depuis un numéro.
CREATE TABLE workshop_events (
    project_id INTEGER NOT NULL REFERENCES workshop_projects (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL CHECK (seq >= 1),
    at         INTEGER NOT NULL,
    event      TEXT    NOT NULL,
    PRIMARY KEY (project_id, seq)
);

-- Questions posées à l'utilisateur (« J'ai besoin de toi »).
CREATE TABLE workshop_questions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES workshop_projects (id) ON DELETE CASCADE,
    question    TEXT    NOT NULL,
    -- Réponse en JSON ; NULL tant qu'elle n'est pas donnée.
    answer      TEXT,
    -- pending : attend ; answered : réponse donnée, pas encore lue par le
    -- projet ; used : lue ; dropped : abandonnée (projet annulé).
    status      TEXT    NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'answered', 'used', 'dropped')),
    created_at  INTEGER NOT NULL,
    answered_at INTEGER
);
CREATE INDEX workshop_questions_by_project ON workshop_questions (project_id, id);

-- Réglages de l'Atelier (ligne unique des réglages de l'app).
-- Dossier des projets ; '' : `Documents/ChatonLM` de l'utilisateur.
ALTER TABLE settings ADD COLUMN workshop_dir TEXT NOT NULL DEFAULT '';
-- Recherche web permise aux projets.
ALTER TABLE settings ADD COLUMN workshop_web INTEGER NOT NULL DEFAULT 1;
-- Autonomie proposée pour un nouveau projet.
ALTER TABLE settings ADD COLUMN workshop_autonomy TEXT NOT NULL DEFAULT 'ask'
    CHECK (workshop_autonomy IN ('plan', 'ask', 'acceptActions', 'autonomous'));
