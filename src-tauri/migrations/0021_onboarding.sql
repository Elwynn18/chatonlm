-- Premier démarrage : choix de la langue, puis courte présentation de l'app
-- (quatre étapes). Montré tant que `onboarding_done` vaut 0. Une installation
-- qui a déjà servi (un modèle choisi ou une conversation) ne le voit pas : ce
-- n'est plus son premier démarrage.
ALTER TABLE settings ADD COLUMN onboarding_done INTEGER NOT NULL DEFAULT 0;
UPDATE settings SET onboarding_done = 1
WHERE selected_model <> '' OR EXISTS (SELECT 1 FROM chats);
