-- Croix de la fenêtre (Settings › General, propre à ChatonLM) : 1 = la fenêtre
-- est masquée et l'app reste dans la zone de notification (raccourcis et bots
-- Easy Link toujours là) ; 0 = l'app quitte, comme Ctrl+Q. Sans zone de
-- notification, la croix quitte toujours. Activé par défaut (comportement
-- d'avant) ; à part de `settings` : « Reset to defaults » n'y touche pas.
ALTER TABLE desktop_settings
    ADD COLUMN close_to_tray INTEGER NOT NULL DEFAULT 1 CHECK (close_to_tray IN (0, 1));
