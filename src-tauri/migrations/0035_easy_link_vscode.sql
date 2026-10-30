-- Easy Link prévient à la fin d'une tâche de VS Code (lien VS Code, propre à
-- ChatonLM) : « Let VS Code notify me through Easy Link ». Coupé par défaut ;
-- coupé, l'extension ne peut ni envoyer de message aux comptes liés ni
-- montrer ses tâches dans `/status`. À part de `settings` : « Reset to
-- defaults » n'y touche pas.
CREATE TABLE easy_link_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    vscode_notify INTEGER NOT NULL DEFAULT 0 CHECK (vscode_notify IN (0, 1))
);
INSERT INTO easy_link_settings (id) VALUES (1);
