-- Atelier : le nom du dossier d'un projet vient de l'utilisateur (gardé), ou
-- il est provisoire et sera remplacé par le titre que le cadrage donne au
-- projet (0). Projets déjà créés : leur nom est gardé.
ALTER TABLE workshop_projects ADD COLUMN folder_named INTEGER NOT NULL DEFAULT 1
    CHECK (folder_named IN (0, 1));
