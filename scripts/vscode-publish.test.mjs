// @vitest-environment node
// Essais des fichiers de publication de l'extension VS Code : notices
// (vscode-notices.mjs), fichiers du paquet .vsix (.vscodeignore, LICENSE,
// CHANGELOG, README, icône) et workflow .github/workflows/vscode-publish.yml
// (outils épinglés hors de package.json, publication sans secret depuis main).
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { renderVscodeNotices } from "./vscode-notices.mjs";

const root = path.resolve(import.meta.dirname, "..");
const read = (file) => readFileSync(path.join(root, file), "utf8");
const manifest = JSON.parse(read("vscode/package.json"));
const workflow = read(".github/workflows/vscode-publish.yml");

describe("notices", () => {
  it("puts the source notices, then the npm packages, under a title", () => {
    const text = renderVscodeNotices({
      thirdParty: "Part 1. Ollama\n\n",
      npmPackages: "react 19 (MIT)\n",
    });
    expect(
      text.startsWith("ChatonLM for VS Code - Third-party notices\n"),
    ).toBe(true);
    expect(text.indexOf("Part 1. Ollama")).toBeLessThan(
      text.indexOf("react 19 (MIT)"),
    );
    expect(text.endsWith("react 19 (MIT)\n")).toBe(true);
  });
});

describe("package files", () => {
  it("leaves the sources, the tests and the source maps out of the .vsix", () => {
    const ignored = read("vscode/.vscodeignore")
      .split("\n")
      .filter((line) => line && !line.startsWith("#"));
    for (const pattern of [
      "src/**",
      "webview/**",
      "shared/**",
      "dist/**/*.map",
      "dist/webview/.vite/**",
      "**/*.test.*",
      "node_modules/**",
    ]) {
      expect(ignored).toContain(pattern);
    }
    // Rien de ce qui sert à l'exécution n'est exclu.
    for (const kept of ["dist/**", "media/**", "l10n/**", "README.md"]) {
      expect(ignored).not.toContain(kept);
    }
  });

  it("ships the license of the repository", () => {
    expect(manifest.license).toBe("MIT");
    expect(read("vscode/LICENSE")).toBe(read("LICENSE"));
  });

  it("has a changelog entry for the version of the manifest", () => {
    expect(read("vscode/CHANGELOG.md")).toMatch(
      new RegExp(`^## ${manifest.version.replaceAll(".", "\\.")}$`, "m"),
    );
  });

  it("has a README with absolute HTTPS links only", () => {
    const readme = read("vscode/README.md");
    const links = [...readme.matchAll(/\]\(([^)]+)\)/g)].map((m) => m[1]);
    expect(links.length).toBeGreaterThan(0);
    for (const link of links) expect(link).toMatch(/^https:\/\//);
    expect(readme).not.toMatch(/http:\/\//);
    expect(readme).not.toMatch(/!\[/); // pas d'image : elles devraient être en ligne
  });

  it("names a PNG icon and the gallery details", () => {
    expect(manifest.icon).toBe("media/icon.png");
    expect(manifest.publisher).toBe("chatonlm");
    expect(manifest.repository.url).toBe(
      "https://github.com/Elwynn18/chatonlm",
    );
    expect(manifest.bugs.url).toBe(
      "https://github.com/Elwynn18/chatonlm/issues",
    );
  });
});

describe("publish workflow", () => {
  it("installs vsce and ovsx at pinned versions, never from package.json", () => {
    expect(workflow).toMatch(/^ {2}VSCE_VERSION: \d+\.\d+\.\d+$/m);
    expect(workflow).toMatch(/^ {2}OVSX_VERSION: \d+\.\d+\.\d+$/m);
    for (const file of ["package.json", "vscode/package.json"]) {
      expect(read(file)).not.toMatch(/"(@vscode\/vsce|ovsx)"/);
    }
    expect(read("package-lock.json")).not.toMatch(
      /"node_modules\/(@vscode\/vsce|ovsx)"/,
    );
    expect(workflow).toMatch(/npm install --prefix "\$tools" --ignore-scripts/);
  });

  it("starts only by hand and publishes only from main", () => {
    expect(workflow).toMatch(/^on:\n {2}workflow_dispatch:\n/m);
    expect(workflow).not.toMatch(/^ {2}(push|pull_request|schedule):/m);
    expect(workflow).toMatch(
      /if: inputs\.publish && github\.ref == 'refs\/heads\/main'/,
    );
    expect(workflow).toMatch(
      /^ {6}publish:\n(?: {8}.*\n)*? {8}default: false$/m,
    );
  });

  it("publishes without stored secrets, except the first Open VSX token", () => {
    expect(workflow).toMatch(/vsce" publish --oidc/);
    expect(workflow).toMatch(/publish "\$VSIX" --trusted-publishing/);
    expect(workflow).toMatch(/id-token: write/);
    const secrets = [...workflow.matchAll(/secrets\.(\w+)/g)].map((m) => m[1]);
    expect(secrets).toEqual(["OVSX_PAT"]);
    // L'autorisation OIDC n'est donnée qu'au travail de publication.
    expect(workflow.split("id-token: write").length).toBe(2);
    expect(workflow.indexOf("id-token: write")).toBeGreaterThan(
      workflow.indexOf("  publish:\n    name:"),
    );
  });
});
