-- Plus d'Ollama derrière l'app (passage au moteur llama.cpp, propre à
-- ChatonLM) : réglages qui ne servent plus effacés. Gardés pour la reprise des
-- modèles et la transition : `builtin_ollama.models_dir` (dossier des modèles
-- de l'ancien Ollama intégré) et `settings.server_url` (adresse de l'Ollama
-- utilisé en 0.9.x, proposée comme serveur compatible OpenAI s'il s'agit
-- d'une autre machine).
ALTER TABLE settings DROP COLUMN unload_on_quit;
ALTER TABLE builtin_ollama DROP COLUMN source;
