-- Cours générés (onglet Study, propre à ChatonLM). Dates en millisecondes
-- depuis l'époque Unix (UTC) ; contenus générés en JSON (types internes de
-- `src-tauri/src/study`), relus à chaque lecture du cours.

-- study_model : modèle des cours ; '' = celui du chat (selected_model).
ALTER TABLE settings ADD COLUMN study_model TEXT NOT NULL DEFAULT '';

CREATE TABLE courses (
    -- UUID v7 : l'ordre alphabétique suit l'ordre de création.
    id          TEXT    PRIMARY KEY,
    -- Sujet demandé (vide : cours tiré des documents seuls).
    topic       TEXT    NOT NULL,
    level       TEXT    NOT NULL CHECK (level IN ('beginner', 'intermediate', 'advanced')),
    minutes     INTEGER NOT NULL CHECK (minutes IN (10, 20, 30)),
    -- Langue du cours (celle de l'interface à la création).
    language    TEXT    NOT NULL CHECK (language IN ('en', 'es', 'fr')),
    -- Modèle qui a écrit le plan.
    model       TEXT    NOT NULL DEFAULT '',
    -- Recherche web demandée.
    web         INTEGER NOT NULL DEFAULT 0 CHECK (web IN (0, 1)),
    status      TEXT    NOT NULL
        CHECK (status IN ('planning', 'unclear', 'planned', 'active', 'finished', 'failed')),
    -- Erreur de la préparation (status 'failed') : JSON StudyError.
    error       TEXT,
    -- Question posée quand le sujet est ambigu (status 'unclear') : JSON.
    clarify     TEXT,
    -- Plan : JSON.
    plan        TEXT,
    -- Bilan final (questions, état de génération) : JSON.
    final       TEXT,
    -- Étape où l'apprenant s'est arrêté (clé choisie par l'interface).
    position    TEXT    NOT NULL DEFAULT '',
    -- Temps passé dans le cours, page visible.
    elapsed_ms  INTEGER NOT NULL DEFAULT 0 CHECK (elapsed_ms >= 0),
    created_at  INTEGER NOT NULL,
    -- Dernière activité (génération, réponse, avancement).
    updated_at  INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX courses_by_activity ON courses (updated_at DESC, id DESC);

-- Sources du cours : documents fournis (texte extrait) et résultats de la
-- recherche web.
CREATE TABLE course_sources (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    course_id TEXT    NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    kind      TEXT    NOT NULL CHECK (kind IN ('document', 'web')),
    -- Nom du fichier, ou titre de la page.
    title     TEXT    NOT NULL,
    url       TEXT    NOT NULL DEFAULT '',
    text      TEXT    NOT NULL
);
CREATE INDEX course_sources_by_course ON course_sources (course_id, id);

-- Paliers : leçon, exercices, essais, explications et questions posées (JSON).
CREATE TABLE course_levels (
    course_id TEXT    NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    idx       INTEGER NOT NULL CHECK (idx >= 0),
    data      TEXT    NOT NULL,
    PRIMARY KEY (course_id, idx)
);

-- Réponses de l'apprenant, une par question (la première est définitive).
CREATE TABLE course_answers (
    course_id   TEXT    NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    question_id TEXT    NOT NULL,
    data        TEXT    NOT NULL,
    PRIMARY KEY (course_id, question_id)
);
