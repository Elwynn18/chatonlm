-- ModelHub : rubrique « Uncensored » (dérivés communautaires sans refus d'un
-- modèle officiel). Masquée par défaut ; se règle dans Settings > Models.
ALTER TABLE settings ADD COLUMN show_uncensored INTEGER NOT NULL DEFAULT 0;
