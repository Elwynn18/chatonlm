-- Recherche web faite depuis l'ordinateur (propre à ChatonLM) : DuckDuckGo
-- par défaut, sans compte ; moteur personnel en option (clé Brave Search API,
-- clé Mojeek, adresse d'une instance SearXNG), essayé d'abord. Une seule
-- ligne. Les clés, comme les jetons d'Easy Link, ne sortent d'ici que vers le
-- moteur choisi : la page ne reçoit que « configurée : oui ou non ».
CREATE TABLE web_search_engine (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    engine TEXT NOT NULL DEFAULT 'duckDuckGo'
        CHECK (engine IN ('duckDuckGo', 'brave', 'mojeek', 'searxng')),
    brave_key TEXT NOT NULL DEFAULT '',
    mojeek_key TEXT NOT NULL DEFAULT '',
    searxng_url TEXT NOT NULL DEFAULT ''
);
INSERT INTO web_search_engine (id) VALUES (1);
