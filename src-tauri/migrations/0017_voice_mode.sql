-- Mode conversation vocale (propre à ChatonLM) : réglages, une seule ligne.
-- Hors de « Reset to defaults » (comme la dictée) ; la consigne a son propre
-- bouton Reset.
CREATE TABLE voice_mode_settings (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    -- « handsFree » : fin du tour au silence ; « pushToTalk » : le tour dure
    -- tant que le bouton (ou Espace) est maintenu.
    turn_mode    TEXT    NOT NULL DEFAULT 'handsFree'
        CHECK (turn_mode IN ('handsFree', 'pushToTalk')),
    -- Silence de fin d'un tour : 0,6 s, 1,0 s ou 1,6 s.
    pause        TEXT    NOT NULL DEFAULT 'normal'
        CHECK (pause IN ('short', 'normal', 'long')),
    -- Parler coupe la réponse (annulation d'écho).
    barge_in     INTEGER NOT NULL DEFAULT 1 CHECK (barge_in IN (0, 1)),
    -- Garder le choix du bouton Think pour les envois vocaux.
    keep_think   INTEGER NOT NULL DEFAULT 0 CHECK (keep_think IN (0, 1)),
    -- Consigne vocale ; vide = consigne par défaut.
    instructions TEXT    NOT NULL DEFAULT ''
);

INSERT INTO voice_mode_settings (id) VALUES (1);
