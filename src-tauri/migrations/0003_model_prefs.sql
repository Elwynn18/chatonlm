-- Préférences mémorisées par modèle (écart à l'officiel, qui ne retient rien).

CREATE TABLE model_prefs (
    -- Nom affiché du modèle (sans le suffixe « :latest »).
    model TEXT PRIMARY KEY,
    -- Dernier réglage du raisonnement, en JSON : true, false ou un niveau
    -- (« "high" »...). Une valeur qu'Ollama n'annonce plus pour ce modèle est
    -- ignorée par l'interface, qui reprend alors le défaut.
    think TEXT NOT NULL
          CHECK (json_valid(think) AND json_type(think) IN ('true', 'false', 'text'))
);
