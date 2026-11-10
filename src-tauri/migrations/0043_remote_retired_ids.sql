-- Identifiants des fournisseurs distants retirés (correction de la relecture
-- du lot L7) : jamais redonnés à un nouveau fournisseur. Les conversations et
-- le modèle choisi gardent `@remote/<id>/<modèle>` : un identifiant réemployé
-- enverrait ces conversations à un autre serveur.
CREATE TABLE remote_retired_ids (
    id TEXT PRIMARY KEY NOT NULL
);
