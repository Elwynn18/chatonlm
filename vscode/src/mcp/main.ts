// Point d'entrée de `dist/mcp.cjs`, serveur MCP de ChatonLM lancé par un
// agent (Copilot, Cline, Cursor…) avec le Node de l'éditeur :
// `<node> dist/mcp.cjs --state <dossier> --session <séance>`. Rien n'est
// écrit sur la sortie standard hors des messages MCP. Contrat :
// .tools/research/95-lot4b-contract.md (2).
import { readFileSync } from "node:fs";
import path from "node:path";
import { currentPlatform } from "../bridge/discovery";
import { parseArgs, runMcp } from "./run";

function version(): string {
  try {
    const manifest = JSON.parse(
      readFileSync(path.join(__dirname, "..", "package.json"), "utf8"),
    ) as { version?: unknown };
    return typeof manifest.version === "string" ? manifest.version : "0";
  } catch {
    return "0";
  }
}

const args = parseArgs(process.argv.slice(2));
if (!args) {
  process.stderr.write(
    "chatonlm-mcp: no state folder given; every tool is refused.\n",
  );
}
runMcp(
  {
    version: version(),
    platform: currentPlatform(),
    env: process.env,
    input: process.stdin,
    write: (line) => void process.stdout.write(`${line}\n`),
    onEnd: () => process.exit(0),
  },
  args,
);
