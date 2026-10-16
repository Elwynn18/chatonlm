-- Temps de la dernière image faite avec ce modèle sur cette machine (contrat
-- 85 § 2), en secondes : de la première étape d'échantillonnage à l'image,
-- sans le démarrage du moteur, le chargement ni la compilation des shaders.
-- NULL : aucune image encore.
ALTER TABLE image_models ADD COLUMN last_seconds INTEGER;
