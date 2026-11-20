-- Réglages avancés d'un modèle d'images choisis par l'utilisateur (étapes,
-- CFG, prompt négatif, graine), en JSON ; NULL : ceux de la famille du modèle.
-- Gardés par l'enregistrement d'un modèle déjà là (ON CONFLICT ... DO UPDATE
-- ne touche pas cette colonne).
ALTER TABLE image_models ADD COLUMN settings TEXT;
