-- Génération d'un cours mise en pause par l'apprenant (onglet Study) : ses
-- parties ne sont plus écrites tant qu'il ne la reprend pas (la file des
-- autres cours continue). Enregistré : la pause survit au redémarrage.
ALTER TABLE courses ADD COLUMN paused INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1));
