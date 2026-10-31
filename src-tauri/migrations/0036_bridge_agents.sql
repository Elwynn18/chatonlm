-- Lien VS Code (propre à ChatonLM) : accès « agents », propre au serveur MCP
-- de l'extension (outils que Copilot, Cline… appellent). Il est créé par un
-- éditeur appairé (`agents.issue`), lui est rattaché (`parent_id`), et l'app
-- le limite elle-même aux besoins de ces outils ; il se retire seul
-- (`agents.revoke`) ou avec son éditeur. Un éditeur en a un au plus.
ALTER TABLE bridge_clients ADD COLUMN scope TEXT NOT NULL DEFAULT 'editor'
    CHECK (scope IN ('editor', 'agents'));
ALTER TABLE bridge_clients ADD COLUMN parent_id TEXT;
CREATE UNIQUE INDEX bridge_clients_agents_of ON bridge_clients (parent_id)
    WHERE parent_id IS NOT NULL;
