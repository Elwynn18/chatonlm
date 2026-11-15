-- Code de l'erreur d'un appel d'outil, pour la page (`ToolCallErrorCode` :
-- 'accessDenied', 'pageNotFound', 'httpStatus'…), et statut HTTP de la page
-- refusée quand le code en a un ; NULL : pas d'erreur, erreur sans code
-- (image) ou enregistrée avant cette migration (la page montre alors le texte
-- de `error`, inchangé : c'est celui que le modèle reçoit).

ALTER TABLE tool_calls ADD COLUMN error_code TEXT;
ALTER TABLE tool_calls ADD COLUMN error_status INTEGER;
