-- Intégration au bureau (étape 18a, propre à ChatonLM) : une seule ligne, à
-- part de la table settings (« Reset to defaults » n'y touche jamais).
CREATE TABLE desktop_settings (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    -- Lancé depuis une AppImage, ChatonLM s'inscrit dans le menu des
    -- applications (fichier .desktop et icône sous XDG_DATA_HOME) ; 0 :
    -- retiré par l'utilisateur (Settings › General), plus jamais réécrit.
    menu_entry INTEGER NOT NULL DEFAULT 1 CHECK (menu_entry IN (0, 1))
);

INSERT INTO desktop_settings (id) VALUES (1);
