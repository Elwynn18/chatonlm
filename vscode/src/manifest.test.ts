// @vitest-environment node
// Manifeste et traductions de l'hôte : commandes déclarées et enregistrées,
// mêmes clés et mêmes {0} dans les trois langues, chaque vscode.l10n.t
// présent dans les catalogues.
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const root = path.resolve(import.meta.dirname, "..");
const read = (file: string) => readFileSync(path.join(root, file), "utf8");
const json = (file: string) =>
  JSON.parse(read(file)) as Record<string, unknown>;

type Manifest = {
  engines: { vscode: string };
  devDependencies: Record<string, string>;
  main: string;
  l10n: string;
  contributes: {
    commands: { command: string; title: string }[];
    viewsContainers: { activitybar: { icon: string }[] };
    views: Record<string, { id: string; name: string }[]>;
  };
};
const manifest = json("package.json") as unknown as Manifest;

function sources(dir = path.join(root, "src")): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "test" ? [] : sources(file);
    return file.endsWith(".ts") && !file.endsWith(".test.ts") ? [file] : [];
  });
}

const placeholders = (text: string) =>
  [...text.matchAll(/\{(\d+)\}/g)].map((m) => m[1]).sort();

describe("manifest", () => {
  it("translates every %key% in the three languages", () => {
    const keys = [...JSON.stringify(manifest).matchAll(/"%([\w.]+)%"/g)].map(
      (m) => m[1],
    );
    expect(keys.length).toBeGreaterThan(5);
    for (const file of [
      "package.nls.json",
      "package.nls.es.json",
      "package.nls.fr.json",
    ]) {
      const nls = json(file);
      expect(Object.keys(nls).sort(), file).toEqual([...new Set(keys)].sort());
      for (const value of Object.values(nls))
        expect(String(value).trim()).not.toBe("");
    }
  });

  it("declares every command the extension registers, and nothing else", () => {
    const code = sources()
      .map((file) => readFileSync(file, "utf8"))
      .join("\n");
    const registered = new Set([
      ...[...code.matchAll(/registerCommand\(\s*"(chatonlm\.\w+)"/g)].map(
        (m) => m[1],
      ),
      ...[...code.matchAll(/\baction\("(\w+)"\)/g)].map(
        (m) => `chatonlm.${m[1]}`,
      ),
    ]);
    const declared = new Set(
      manifest.contributes.commands.map((c) => c.command),
    );
    expect([...registered].sort()).toEqual([...declared].sort());
  });

  it("points at existing files and the right API version", () => {
    expect(manifest.main).toBe("./dist/extension.cjs");
    expect(manifest.l10n).toBe("./l10n");
    const icon = manifest.contributes.viewsContainers.activitybar[0].icon;
    expect(read(icon)).toMatch(/^<!--[^]*<svg /);
    expect(manifest.contributes.views.chatonlm[0].id).toBe("chatonlm.chat");
    expect(`^${manifest.devDependencies["@types/vscode"]}`).toBe(
      manifest.engines.vscode,
    );
  });

  it("stays off in a folder that is not trusted, and never starts the app on its own", () => {
    expect(
      (manifest as unknown as { capabilities: unknown }).capabilities,
    ).toEqual({ untrustedWorkspaces: { supported: false } });
    // Aucun lancement de l'app à l'activation (au démarrage de VS Code) :
    // seulement sur un geste (panneau, commande).
    const activate = read("src/extension.ts").split(
      "export function activate",
    )[1];
    expect(activate).not.toMatch(/launch\(true\)|launchApp\(/);
  });
});

describe("l10n", () => {
  const bundles = {
    en: json("l10n/bundle.l10n.json"),
    es: json("l10n/bundle.l10n.es.json"),
    fr: json("l10n/bundle.l10n.fr.json"),
  };

  it("has the same keys and placeholders in the three languages", () => {
    const keys = Object.keys(bundles.en).sort();
    for (const [lang, bundle] of Object.entries(bundles)) {
      expect(Object.keys(bundle).sort(), lang).toEqual(keys);
      for (const key of keys) {
        expect(placeholders(String(bundle[key])), `${lang}: ${key}`).toEqual(
          placeholders(key),
        );
      }
    }
    for (const key of keys) expect(bundles.en[key]).toBe(key);
  });

  it("knows every text the extension shows, and only those", () => {
    const used = new Set<string>();
    for (const file of sources()) {
      const code = readFileSync(file, "utf8");
      for (const m of code.matchAll(/l10n\.t\(\s*("(?:[^"\\]|\\.)*")/g)) {
        used.add(JSON.parse(m[1]) as string);
      }
      // Un appel à l10n.t qui ne commence pas par une chaîne échapperait à cet essai.
      expect(code.match(/l10n\.t\(\s*[^\s"]/g), file).toBeNull();
    }
    expect([...used].sort()).toEqual(Object.keys(bundles.en).sort());
  });

  it("follows French typography (no-break space before : ; ! ?)", () => {
    for (const [key, value] of Object.entries(bundles.fr)) {
      expect(String(value), key).not.toMatch(/ [:;!?]|[^\u00a0 ][:;!?](\s|$)/u);
    }
  });
});
