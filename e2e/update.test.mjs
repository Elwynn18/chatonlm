// Mises à jour de ChatonLM (étape 18c) sur de vraies AppImages, dans la
// session virtuelle : 0.0.1 et 0.0.2 construites par scripts/updater-e2e.sh
// (fonctionnalité e2e, clé publique de test, 0.0.2 signée par la clé de
// test), servies par un faux serveur de mises à jour (e2e/lib/update-server.mjs,
// CHATONLM_UPDATE_URL). Chaque cas copie la 0.0.1 dans un dossier à lui, avec
// son propre dossier de données :
// 1. auto-download : téléchargement au démarrage, notification « Update
//    available » une fois, « Restart to update » dans la zone de
//    notification ; clic : l'app se relance en 0.0.2 (fichier remplacé,
//    version affichée dans Settings), à jour, sans nouvelle notification ;
//    aucun processus restant à la fin ;
// 2. signature fausse : erreur « signature », AppImage intacte, rien gardé ;
// 3. dossier non inscriptible : « non pris en charge » avant tout
//    téléchargement ;
// 4. par la carte Updates de Settings : Auto-download coupé (interrupteur),
//    « Check for updates » : seulement signalée (zone de notification :
//    « An update is available », qui ouvre Settings sur la carte, défilée et
//    focalisée), « Download » : prête, une notification ; relancée sans
//    installer : prête sans nouveau téléchargement ni seconde notification,
//    vérifiée de nouveau ; « Restart to update » de la zone de notification
//    refusé (dossier devenu non inscriptible) : Settings s'ouvre sur la
//    carte, qui le dit une fois sous la mise à jour toujours prête ; dossier
//    rendu, « Check for updates » (le refus s'efface) puis « Restart to
//    update » de la carte : relance en 0.0.2.
//   scripts/e2e-session.sh e2e/run.sh e2e/update.test.mjs "$(scripts/updater-e2e.sh)"
// Donné un binaire (boucle des e2e de scripts/check.sh --full, avec celui de
// scripts/e2e-build.sh) au lieu du dossier des AppImages : ignoré, en le
// disant (deux constructions release, à lancer à part).
import assert from "node:assert/strict";
import { createHash, randomUUID } from "node:crypto";
import {
  accessSync,
  chmodSync,
  constants,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { ROOT, withApp } from "./lib/app.mjs";
import { poll, startDesktopServices } from "./lib/desktop.mjs";
import { startFake } from "./lib/fake.mjs";
import { startUpdateServer, updateManifest } from "./lib/update-server.mjs";

const DIR = process.argv[2];
assert.ok(DIR, "dossier des AppImages d'essai (scripts/updater-e2e.sh)");
if (statSync(DIR).isFile()) {
  console.log(
    "update.test.mjs : ignoré (AppImages d'essai de scripts/updater-e2e.sh requises)",
  );
  process.exit(0);
}
const OLD = path.join(DIR, "chatonlm_0.0.1_amd64.AppImage");
const NEW = path.join(DIR, "chatonlm_0.0.2_amd64.AppImage");
for (const file of [OLD, NEW, `${NEW}.sig`]) {
  assert.ok(existsSync(file), `absent : ${file}`);
}
const SIGNATURE = readFileSync(`${NEW}.sig`, "utf8").trim();
const WORK = `${ROOT}.tools/tmp/update-e2e-${process.pid}`;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const sha256 = (file) =>
  createHash("sha256").update(readFileSync(file)).digest("hex");
const OLD_SHA = sha256(OLD);
const NEW_SHA = sha256(NEW);
assert.notEqual(OLD_SHA, NEW_SHA);
const labels = (menu) =>
  menu.map((item) => (item.type === "separator" ? "-" : item.label));

// Sans FUSE utilisable (/dev/fuse), le runtime de l'AppImage l'extrait dans
// TMPDIR (.tools/tmp) au lieu de la monter.
let fuse = true;
try {
  accessSync("/dev/fuse", constants.R_OK | constants.W_OK);
} catch {
  fuse = false;
}

/** Processus marqués par `tag` (variable héritée par l'app relancée et ses
 * processus WebKit et FUSE). */
function tagged(tag) {
  const marker = `CHATONLM_E2E_UPDATE_TAG=${tag}`;
  const found = [];
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const environ = readFileSync(`/proc/${entry}/environ`, "utf8");
      if (environ.split("\0").includes(marker)) found.push(Number(entry));
    } catch {
      // Terminé, ou d'un autre utilisateur.
    }
  }
  return found;
}

/** Aucun processus du cas après sa fin (attente, puis arrêt et échec). */
async function noProcessLeft(tag, what) {
  for (let i = 0; i < 100 && tagged(tag).length > 0; i++) await delay(100);
  const left = tagged(tag);
  for (const pid of left) {
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // Déjà terminé.
    }
  }
  assert.deepEqual(left, [], `processus restants (${what})`);
}

/** Dossier du cas : AppImage 0.0.1 dans Apps/, données à part. */
function prepare(name) {
  const root = path.join(WORK, name);
  const apps = path.join(root, "Apps");
  mkdirSync(apps, { recursive: true });
  const appimage = path.join(apps, "ChatonLM.AppImage");
  copyFileSync(OLD, appimage);
  chmodSync(appimage, 0o755);
  const data = path.join(root, "data");
  mkdirSync(data, { recursive: true });
  return { root, apps, appimage, data, tag: randomUUID() };
}

function envFor(place, server) {
  return {
    // Faux serveur Ollama : l'app passe l'écran d'accueil sans dépendre de
    // l'Ollama de la machine (relance comprise : environnement hérité).
    OLLAMA_HOST: fake.url,
    CHATONLM_DATA_DIR: place.data,
    CHATONLM_UPDATE_URL: server.manifestUrl,
    CHATONLM_E2E_UPDATE_TAG: place.tag,
    ...(fuse ? {} : { APPIMAGE_EXTRACT_AND_RUN: "1" }),
  };
}

const status = (driver) => driver.invoke("update_status");
const waitState = (driver, predicate, what, timeoutMs = 60000) =>
  poll(
    () => status(driver),
    (s) => predicate(s.state),
    { what, timeoutMs },
  );

/** Mise à jour gardée sur le disque (fichiers du dossier updates/). */
const kept = (place) => {
  const dir = path.join(place.data, "updates");
  return existsSync(dir) ? readdirSync(dir).sort() : [];
};

const server = await startUpdateServer();
const fake = await startFake("ollama");
const desk = await startDesktopServices({
  notifications: true,
  watcher: true,
  portal: null,
});
const updateNotifications = () =>
  desk.notifications().filter((n) => n.summary === "Update available");

/**
 * Ouvre `route` par l'évènement « navigate », renvoyé jusqu'à ce que la
 * carte Updates soit là (l'écouteur n'existe qu'une fois l'app montée).
 */
async function openSettings(driver, route = "/settings") {
  await poll(
    async () => {
      await driver.tauriReady();
      await driver.execute(
        "window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: 'navigate', payload: arguments[0] }); return true",
        [route],
      );
      await delay(300);
      return driver.execute(
        "return !!document.querySelector('[data-testid=updates]')",
      );
    },
    (open) => open,
    { timeoutMs: 20000, what: "ouverture de Settings" },
  );
}

/** Ce que montre la carte : état, textes, boutons, focus, visibilité. */
const readCard = (driver) =>
  driver.execute(`
    const card = document.querySelector("[data-testid=updates]");
    if (!card) return null;
    const title = card.querySelector("h2");
    const scroller = card.closest(".overflow-y-auto");
    const box = card.getBoundingClientRect();
    const view = scroller ? scroller.getBoundingClientRect() : { top: 0, bottom: innerHeight };
    return {
      state: card.querySelector("[data-testid=update-state]")?.textContent ?? null,
      text: card.innerText,
      buttons: [...card.querySelectorAll("button:not([role=switch])")].map((b) => b.textContent.trim()),
      alerts: [...card.querySelectorAll("[role=alert]")].map((a) => a.textContent),
      autoDownload: card.querySelector("[role=switch]")?.getAttribute("aria-checked") ?? null,
      focused: document.activeElement === title,
      titleInView: title.getBoundingClientRect().top >= view.top && title.getBoundingClientRect().bottom <= view.bottom,
      cardTop: box.top - view.top,
    };`);

const waitCard = (driver, predicate, what, timeoutMs = 60000) =>
  poll(
    () => readCard(driver),
    (card) => card !== null && predicate(card),
    { what, timeoutMs },
  );

/** Clic (geste de l'utilisateur, en JS) sur un bouton de la carte. */
async function clickCard(driver, label) {
  const clicked = await driver.execute(
    `const button = [...document.querySelectorAll("[data-testid=updates] button")]
       .find((b) => b.textContent.trim() === arguments[0] && b.getAttribute("aria-disabled") !== "true");
     if (!button) return false;
     button.click();
     return true;`,
    [label],
  );
  assert.ok(clicked, `bouton « ${label} » absent ou inactif`);
}

/** Focus mis ailleurs (bouton Reset de la page) : une demande doit le reprendre. */
const focusElsewhere = (driver) =>
  driver.execute(
    `const reset = [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Reset to defaults");
     reset?.focus();
     return document.activeElement === reset;`,
  );

try {
  // 1. Auto-download, Restart to update, relance en 0.0.2.
  {
    const place = prepare("auto");
    server.set({
      manifest: updateManifest({
        version: "0.0.2",
        url: `${server.url}/chatonlm_0.0.2_amd64.AppImage`,
        signature: SIGNATURE,
      }),
      files: { "/chatonlm_0.0.2_amd64.AppImage": NEW },
    });
    await withApp(
      place.appimage,
      { name: "update-auto", env: envFor(place, server) },
      async (driver, app) => {
        // Ollama intégré (étape 18b) : présent ou non selon le paquet.
        assert.equal((await driver.invoke("app_versions")).chatonlm, "0.0.1");
        const ready = await waitState(
          driver,
          (s) => s.state === "ready",
          "mise à jour prête",
        );
        assert.equal(ready.state.version, "0.0.2");
        assert.equal(ready.installer, "appImage");
        assert.equal(ready.autoDownload, true);
        assert.equal(server.count("/chatonlm_0.0.2_amd64.AppImage"), 1);
        assert.deepEqual(kept(place), ["chatonlm-0.0.2.update", "update.json"]);

        const notification = await desk.waitFor(
          (e) => e.event === "notify" && e.summary === "Update available",
        );
        assert.equal(
          notification.body,
          "ChatonLM version 0.0.2 is ready to install",
        );
        await desk.waitFor((e) => e.event === "item-registered");
        const menu = await poll(
          () => desk.command("menu"),
          (items) => labels(items).includes("Restart to update"),
          { what: "menu de mise à jour" },
        );
        assert.deepEqual(labels(menu).slice(-5), [
          "-",
          "An update is available",
          "Restart to update",
          "-",
          "Quit ChatonLM",
        ]);
        assert.equal(
          menu.find((item) => item.label === "An update is available").enabled,
          false,
        );

        // Clic sur « Restart to update » : l'app installe et se relance.
        await desk.command("menu-click", { label: "Restart to update" });
        const code = await Promise.race([
          app.exited,
          delay(60000).then(() => "délai"),
        ]);
        assert.equal(code, 0, "fin de la 0.0.1 pour la relance");
        assert.equal(sha256(place.appimage), NEW_SHA, "AppImage remplacée");
        assert.equal(
          readFileSync(place.appimage).length,
          readFileSync(NEW).length,
        );

        // La relance : même port WebDriver (environnement hérité).
        await driver.start({ timeoutMs: 60000 });
        assert.equal((await driver.invoke("app_versions")).chatonlm, "0.0.2");
        const upToDate = await waitState(
          driver,
          (s) => s.state === "upToDate",
          "à jour après la relance",
        );
        assert.equal(upToDate.installer, "appImage");
        assert.deepEqual(kept(place), [], "mise à jour installée supprimée");
        // Version affichée dans Settings › General.
        // L'écouteur « navigate » n'existe qu'une fois l'app montée : on
        // renvoie l'événement jusqu'à l'ouverture de Settings.
        await poll(
          async () => {
            await driver.tauriReady();
            await driver.execute(
              "window.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: 'navigate', payload: '/settings' }); return true",
            );
            await delay(300);
            return driver.execute(
              "return !!document.querySelector('[data-testid=settings]')",
            );
          },
          (open) => open,
          { timeoutMs: 20000, what: "ouverture de Settings" },
        );
        await poll(
          () => driver.execute("return document.body.innerText"),
          (text) => text.includes("0.0.2"),
          { what: "version 0.0.2 affichée" },
        );
        await driver.screenshot(".tools/shots/update-relaunched.png");
        // Plus d'entrée de mise à jour, pas de seconde notification.
        const after = await desk.command("menu");
        assert.ok(!labels(after).includes("Restart to update"));
        assert.equal(updateNotifications().length, 1);

        await driver.closeWindow();
        await noProcessLeft(place.tag, "après la relance");
      },
    );
    await noProcessLeft(place.tag, "cas 1");
  }

  // 2. Signature fausse : rien n'est installé ni gardé.
  {
    const place = prepare("bad-signature");
    const tampered = Buffer.concat([readFileSync(NEW), Buffer.from([0])]);
    server.set({
      manifest: updateManifest({
        version: "0.0.2",
        url: `${server.url}/tampered.AppImage`,
        signature: SIGNATURE,
      }),
      files: { "/tampered.AppImage": tampered },
    });
    await withApp(
      place.appimage,
      { name: "update-bad-signature", env: envFor(place, server) },
      async (driver) => {
        const failed = await waitState(
          driver,
          (s) => s.state === "error",
          "erreur de signature",
        );
        assert.equal(failed.state.code, "signature", failed.state.detail);
        assert.deepEqual(kept(place), []);
        assert.equal(sha256(place.appimage), OLD_SHA);
      },
    );
    await noProcessLeft(place.tag, "cas 2");
  }

  // 3. Dossier de l'AppImage non inscriptible : dit avant tout téléchargement.
  {
    const place = prepare("read-only");
    server.set({
      manifest: updateManifest({
        version: "0.0.2",
        url: `${server.url}/chatonlm_0.0.2_amd64.AppImage`,
        signature: SIGNATURE,
      }),
      files: { "/chatonlm_0.0.2_amd64.AppImage": NEW },
    });
    const downloads = server.count("/chatonlm_0.0.2_amd64.AppImage");
    chmodSync(place.apps, 0o555);
    try {
      await withApp(
        place.appimage,
        { name: "update-read-only", env: envFor(place, server) },
        async (driver) => {
          const refused = await waitState(
            driver,
            (s) => s.state === "unsupported",
            "dossier non inscriptible",
          );
          assert.deepEqual(refused.state, {
            state: "unsupported",
            reason: "readOnlyFolder",
            version: "0.0.2",
          });
          await delay(2000);
          assert.equal(
            server.count("/chatonlm_0.0.2_amd64.AppImage"),
            downloads,
          );
          assert.equal((await status(driver)).state.state, "unsupported");
        },
      );
    } finally {
      chmodSync(place.apps, 0o755);
    }
    assert.equal(sha256(place.appimage), OLD_SHA);
    await noProcessLeft(place.tag, "cas 3");
  }

  // 4. Par la carte de Settings : signalée, téléchargée sur demande,
  // gardée à la relance, refus montré dans la carte, installée.
  {
    const place = prepare("manual");
    server.set({ manifest: null, files: {} });
    const name = "/chatonlm_0.0.2_amd64.AppImage";
    // Les compteurs du faux serveur et du bureau couvrent tous les cas : on
    // part de là.
    const base = server.count(name);
    const notesBefore = updateNotifications().length;
    await withApp(
      place.appimage,
      { name: "update-manual", env: envFor(place, server) },
      async (driver) => {
        await waitState(driver, (s) => s.state === "upToDate", "à jour");
        await openSettings(driver);
        await waitCard(
          driver,
          (card) => card.state === "ChatonLM is up to date",
          "carte à jour",
        );
        // Interrupteur Auto-download de la carte.
        await driver.execute(
          "document.querySelector('[data-testid=updates] [role=switch]').click(); return true",
        );
        await waitCard(
          driver,
          (card) => card.autoDownload === "false",
          "Auto-download coupé",
        );
        assert.equal((await status(driver)).autoDownload, false);
        server.set({
          manifest: updateManifest({
            version: "0.0.2",
            url: `${server.url}${name}`,
            signature: SIGNATURE,
          }),
          files: { [name]: NEW },
        });

        // « Check for updates » de la carte : signalée, rien téléchargé.
        await clickCard(driver, "Check for updates");
        const available = await waitCard(
          driver,
          (card) => card.state === "ChatonLM version 0.0.2 is available",
          "annonce dans la carte",
        );
        assert.ok(
          available.text.includes("ChatonLM 0.0.2 for the update test."),
        );
        assert.ok(available.buttons.includes("Download"), available.buttons);
        const checked = await status(driver);
        assert.equal(checked.state.state, "available");
        assert.equal(checked.state.version, "0.0.2");
        await delay(2000);
        assert.equal(server.count(name), base, "rien téléchargé");
        const menu = await poll(
          () => desk.command("menu"),
          (items) => labels(items).includes("An update is available"),
          { what: "annonce dans la zone de notification" },
        );
        assert.ok(!labels(menu).includes("Restart to update"));

        // « An update is available » : Settings sur la carte, défilée et
        // focalisée, même avec le focus ailleurs.
        assert.ok(await focusElsewhere(driver), "focus sur Reset");
        await desk.command("menu-click", { label: "An update is available" });
        const shown = await waitCard(
          driver,
          (card) => card.focused,
          "carte focalisée depuis la zone de notification",
          20000,
        );
        assert.ok(shown.titleInView, `titre hors de la vue (${shown.cardTop})`);

        // « Download » de la carte : prête, une notification.
        await clickCard(driver, "Download");
        const ready = await waitCard(
          driver,
          (card) => card.state === "ChatonLM version 0.0.2 is ready to install",
          "prête dans la carte",
        );
        assert.ok(ready.buttons.includes("Restart to update"), ready.buttons);
        assert.equal((await status(driver)).state.state, "ready");
        assert.equal(server.count(name), base + 1);
        await poll(
          () => updateNotifications().length,
          (count) => count === notesBefore + 1,
          { what: "notification de la mise à jour prête" },
        );
        await driver.screenshot(".tools/shots/update-card-ready.png");
      },
    );
    await noProcessLeft(place.tag, "cas 4, 1er lancement");
    assert.equal(sha256(place.appimage), OLD_SHA, "rien installé seul");

    const manifests = server.count("/latest.json");
    const notes = updateNotifications().length;
    await withApp(
      place.appimage,
      { name: "update-manual-relaunch", env: envFor(place, server) },
      async (driver, app) => {
        // Prête dès le démarrage (signature revérifiée, Auto-download coupé :
        // sans elle, la vérification la dirait seulement disponible), puis
        // encore après la vérification, sans téléchargement ni seconde
        // notification.
        const ready = await waitState(
          driver,
          (s) => s.state === "ready",
          "prête à la relance",
          20000,
        );
        assert.equal(ready.autoDownload, false);
        await poll(
          () => server.count("/latest.json"),
          (count) => count > manifests,
          { what: "vérification après la relance", timeoutMs: 20000 },
        );
        await delay(2000);
        assert.equal((await status(driver)).state.state, "ready");
        assert.equal(server.count(name), base + 1);
        assert.equal(
          updateNotifications().length,
          notes,
          "une notification par version, même après une relance",
        );

        // « Restart to update » de la zone de notification refusé : le
        // dossier n'est plus inscriptible. Settings s'ouvre sur la carte,
        // qui le dit une seule fois, sous la mise à jour toujours prête.
        await openSettings(driver);
        assert.ok(await focusElsewhere(driver), "focus sur Reset");
        await desk.waitFor((e) => e.event === "item-registered");
        await poll(
          () => desk.command("menu"),
          (items) => labels(items).includes("Restart to update"),
          { what: "Restart to update dans la zone de notification" },
        );
        chmodSync(place.apps, 0o555);
        try {
          await desk.command("menu-click", { label: "Restart to update" });
          const refused = await waitCard(
            driver,
            (card) => card.focused && card.alerts.length > 0,
            "refus montré dans la carte",
            20000,
          );
          assert.equal(
            refused.state,
            "ChatonLM version 0.0.2 is ready to install",
          );
          assert.deepEqual(
            refused.alerts,
            [
              "Couldn't install the update" +
                "ChatonLM cannot replace its AppImage in this folder. Move the AppImage to a folder of your own (for example ~/Applications), then start ChatonLM from there.",
            ],
            "refus dit une seule fois",
          );
          assert.ok(refused.buttons.includes("Restart to update"));
          assert.ok(!refused.buttons.includes("Retry"), refused.buttons);
          assert.equal((await status(driver)).state.state, "ready");
          assert.ok(
            refused.titleInView,
            `titre hors de la vue (${refused.cardTop})`,
          );
          await driver.screenshot(".tools/shots/update-card-refused.png");
        } finally {
          chmodSync(place.apps, 0o755);
        }
        assert.equal(sha256(place.appimage), OLD_SHA, "rien installé");

        // Dossier rendu : « Check for updates » (vérification réussie : le
        // refus s'efface), puis « Restart to update » de la carte.
        const checks = server.count("/latest.json");
        await clickCard(driver, "Check for updates");
        await waitCard(
          driver,
          (card) =>
            card.state === "ChatonLM version 0.0.2 is ready to install" &&
            card.alerts.length === 0,
          "prête, refus effacé",
        );
        assert.ok(server.count("/latest.json") > checks);
        assert.equal(
          server.count(name),
          base + 1,
          "pas de nouveau téléchargement",
        );
        await clickCard(driver, "Restart to update");
        const code = await Promise.race([
          app.exited,
          delay(60000).then(() => "délai"),
        ]);
        assert.equal(code, 0, "fin de la 0.0.1 pour la relance");
        assert.equal(sha256(place.appimage), NEW_SHA, "AppImage remplacée");

        await driver.start({ timeoutMs: 60000 });
        assert.equal((await driver.invoke("app_versions")).chatonlm, "0.0.2");
        await waitState(
          driver,
          (s) => s.state === "upToDate",
          "à jour après la relance",
        );
        assert.deepEqual(kept(place), [], "mise à jour installée supprimée");
        await driver.closeWindow();
        await noProcessLeft(place.tag, "cas 4, après la relance");
      },
    );
    await noProcessLeft(place.tag, "cas 4, relance");
  }
} finally {
  await desk.stop();
  await server.stop();
  await fake.stop();
  if (!process.env.E2E_KEEP) rmSync(WORK, { recursive: true, force: true });
}
console.log("update.test.mjs : OK");
