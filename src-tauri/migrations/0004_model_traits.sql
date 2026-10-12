-- Comportements observés d'un modèle (écart à l'officiel, qui ne retient rien).

CREATE TABLE model_traits (
    -- Nom affiché du modèle (sans le suffixe « :latest »), comme model_prefs.
    model TEXT PRIMARY KEY,
    -- 1 : le modèle a raisonné alors que `think: false` était envoyé (son
    -- gabarit ouvre toujours le raisonnement, ex. LFM2.5) ; le bouton Think
    -- n'est plus proposé pour lui.
    always_thinks INTEGER NOT NULL CHECK (always_thinks IN (0, 1))
);
