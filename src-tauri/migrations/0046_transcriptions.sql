-- Transcriptions (outil Transcription, propre à ChatonLM) : historique des
-- fichiers et adresses transcrits. Dates en millisecondes depuis l'époque
-- Unix (UTC) ; passages horodatés en JSON (`transcription::paragraphs::Segment`).

CREATE TABLE transcriptions (
    -- UUID v7 : l'ordre alphabétique suit l'ordre de création.
    id            TEXT    PRIMARY KEY,
    source        TEXT    NOT NULL CHECK (source IN ('file', 'url')),
    -- Chemin du fichier local, ou adresse donnée par l'utilisateur.
    origin        TEXT    NOT NULL,
    -- Adresse canonique de la vidéo et site (yt-dlp) ; vides pour un fichier.
    url           TEXT    NOT NULL DEFAULT '',
    site          TEXT    NOT NULL DEFAULT '',
    -- Titre (de la vidéo, ou nom du fichier ; renommable) et chaîne.
    title         TEXT    NOT NULL DEFAULT '',
    channel       TEXT    NOT NULL DEFAULT '',
    -- Durée du son décodé (ou annoncée tant qu'il ne l'est pas).
    duration_ms   INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    engine        TEXT    NOT NULL CHECK (engine IN ('parakeet', 'whisper')),
    -- Identifiant du catalogue des modèles de transcription.
    model         TEXT    NOT NULL,
    -- Code de langue demandé ou détecté ; '' : inconnue (détection du moteur).
    language      TEXT    NOT NULL DEFAULT '',
    status        TEXT    NOT NULL
        CHECK (status IN ('running', 'done', 'failed', 'cancelled', 'interrupted')),
    -- Code de l'erreur (status 'failed') : `TranscriptionErrorCode`.
    error         TEXT,
    -- Passages horodatés : JSON [{start, end, text}], secondes.
    segments      TEXT    NOT NULL DEFAULT '[]',
    -- Texte en paragraphes, sans horodatage.
    text          TEXT    NOT NULL DEFAULT '',
    -- Synthèse (« Synthétiser », lot 6) : document Markdown, modèle, date.
    summary       TEXT,
    summary_model TEXT    NOT NULL DEFAULT '',
    summary_at    INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    finished_at   INTEGER
);
CREATE INDEX transcriptions_by_date ON transcriptions (created_at DESC, id DESC);
