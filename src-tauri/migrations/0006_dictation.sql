-- Dictée (étape 17, propre à ChatonLM) : réglages, une seule ligne.
CREATE TABLE dictation_settings (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    -- Raccourci global lié et dictée proposée dans la zone de notification.
    enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    -- « toggle » : un appui commence, le suivant arrête ; « hold » : parler
    -- en maintenant le raccourci (push-to-talk).
    mode            TEXT    NOT NULL DEFAULT 'toggle' CHECK (mode IN ('toggle', 'hold')),
    -- Identifiant du catalogue des modèles de transcription ; vide = aucun.
    speech_model    TEXT    NOT NULL DEFAULT '',
    -- Code de langue (« fr », « en »...) ou « auto ».
    language        TEXT    NOT NULL DEFAULT 'auto',
    -- Modèle Ollama du nettoyage ; vide = texte copié sans nettoyage.
    polish_model    TEXT    NOT NULL DEFAULT '',
    -- Consignes du nettoyage ; vide = consignes par défaut.
    polish_prompt   TEXT    NOT NULL DEFAULT '',
    -- Dictionnaire personnalisé : tableau JSON de chaînes.
    dictionary      TEXT    NOT NULL DEFAULT '[]',
    -- Texte dicté montré dans la notification (sinon « Text copied »).
    notify_text     INTEGER NOT NULL DEFAULT 1 CHECK (notify_text IN (0, 1)),
    -- Notification « toujours lancée dans la zone de notification » déjà
    -- montrée (à la première fermeture de la fenêtre).
    tray_hint_shown INTEGER NOT NULL DEFAULT 0 CHECK (tray_hint_shown IN (0, 1))
);

INSERT INTO dictation_settings (id) VALUES (1);
