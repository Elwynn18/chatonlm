-- Modèles distants (passage à llama-server, contrat 102 § 1.3) : la colonne
-- `cloud_models` (modèles cloud d'Ollama, migration 0029) porte désormais le
-- réglage `remoteModels` (fournisseurs compatibles OpenAI avec la clé de
-- l'utilisateur). Le sens change : remise à 0 pour toutes les bases, le
-- réglage reste COUPÉ tant que l'utilisateur ne l'a pas choisi de nouveau.
UPDATE settings SET cloud_models = 0;

-- Codes d'erreur de l'Étude renommés (StudyErrorCode : `unreachable` →
-- `engineFailed`, `signedOut` → `webSearch`) dans le JSON des cours
-- enregistrés (préparation, paliers, bilan) : sinon un ancien cours ne se
-- relit plus. Clés JSON non échappées : le texte d'une leçon (chaîne JSON,
-- guillemets échappés) n'est jamais touché.
UPDATE courses
SET error = replace(replace(error, '"code":"unreachable"', '"code":"engineFailed"'),
                    '"code":"signedOut"', '"code":"webSearch"')
WHERE error IS NOT NULL;
UPDATE courses
SET final = replace(replace(final, '"code":"unreachable"', '"code":"engineFailed"'),
                    '"code":"signedOut"', '"code":"webSearch"')
WHERE final IS NOT NULL;
UPDATE course_levels
SET data = replace(replace(data, '"code":"unreachable"', '"code":"engineFailed"'),
                   '"code":"signedOut"', '"code":"webSearch"');
