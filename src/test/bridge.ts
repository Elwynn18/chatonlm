// Faux moteur du pont de l'extension VS Code pour le simulateur (tests vitest
// seulement) : pont coupé par défaut, éditeurs appairés et code d'appairage en
// mémoire. Comme le moteur, chaque commande renvoie l'état d'après.
// bridge_set_enabled ouvre le pont (état `openStatus` : « listening » par
// défaut, « unavailable » ou « failed » pour simuler un refus du système) ou
// le ferme (« off » : code abandonné, éditeurs déconnectés, mais toujours
// appairés). bridge_pair donne un nouveau code (six chiffres, dix minutes,
// remplace le précédent) ; refusé (erreur du moteur, PAIR_REFUSED) tant que
// le pont n'écoute pas. bridge_cancel_pairing l'abandonne, bridge_unlink
// oublie un éditeur. Le test joue ce que fait le moteur de lui-même :
// pairClient() (un éditeur tape le code : appairé, connecté, code consommé),
// setConnected(), expirePairing(), setStatus(), chacun suivi de « bridge ».
// Une panne d'une commande se force par `failures`.

import type { AppError } from "@/bindings/AppError";
import type { BridgeClient } from "@/bindings/BridgeClient";
import type { BridgeState } from "@/bindings/BridgeState";
import type { BridgeStatus } from "@/bindings/BridgeStatus";
import type { Args, Simulator } from "@/test/simulator";

/** Durée de validité d'un code : dix minutes. */
export const BRIDGE_PAIRING_MS = 10 * 60 * 1000;

/** Refus d'un code quand le pont est fermé (texte du moteur, déjà traduit). */
export const PAIR_REFUSED = "Turn on the VS Code bridge first.";

export interface FakeBridge {
  /** État rendu par bridge_state. */
  state: BridgeState;
  /** État que prend le pont quand on l'ouvre (« listening » par défaut). */
  openStatus: BridgeStatus;
  /** Erreur (panne) renvoyée par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Pose le pont ouvert (état `openStatus`), sans évènement : avant le rendu. */
  enable(): void;
  /** Ajoute un éditeur appairé, sans évènement : à appeler avant le rendu. */
  addClient(name: string, fields?: Partial<BridgeClient>): BridgeClient;
  /** Émet « bridge » avec l'état du moment. */
  emit(): Promise<void>;
  /**
   * Un éditeur tape le code : appairé et connecté, code consommé, puis
   * « bridge ». Fait échouer l'essai sans code en cours.
   */
  pairClient(name: string): Promise<BridgeClient>;
  /** Un éditeur se connecte ou se déconnecte : évènement. */
  setConnected(id: string, connected: boolean): Promise<void>;
  /** Le code expire : retiré de l'état, puis « bridge ». */
  expirePairing(): Promise<void>;
  /** Le pont change d'état de lui-même (échec, ouverture) : évènement. */
  setStatus(status: BridgeStatus): Promise<void>;
}

function invalid(message: string): AppError {
  return { kind: "invalid", message };
}

export function fakeBridge(simulator: Simulator): FakeBridge {
  let lastClient = 0;
  let lastCode = 731_204;

  /** Copie : chaque réponse est un nouvel objet, comme celles du moteur. */
  const snapshot = (): BridgeState => structuredClone(bridge.state);

  function updateClient(id: string, fields: Partial<BridgeClient>) {
    const found = bridge.state.clients.some((client) => client.id === id);
    if (!found) throw simulator.fail(`no bridge client "${id}"`);
    bridge.state.clients = bridge.state.clients.map((client) =>
      client.id === id ? { ...client, ...fields } : client,
    );
  }

  const bridge: FakeBridge = {
    state: { enabled: false, status: "off", clients: [], pairing: null },
    openStatus: "listening",
    failures: new Map(),
    enable() {
      bridge.state.enabled = true;
      bridge.state.status = bridge.openStatus;
    },
    addClient(name, fields = {}) {
      lastClient += 1;
      const client: BridgeClient = {
        id: `client-${lastClient}`,
        name,
        pairedAt: Date.now(),
        lastSeenAt: null,
        connected: false,
        ...fields,
      };
      bridge.state.clients = [...bridge.state.clients, client];
      return client;
    },
    emit: () => simulator.emitEvent("bridge", snapshot()),
    async pairClient(name) {
      if (!bridge.state.pairing) {
        throw simulator.fail("pairClient: no pairing code");
      }
      const now = Date.now();
      const client = bridge.addClient(name, {
        pairedAt: now,
        lastSeenAt: now,
        connected: true,
      });
      bridge.state.pairing = null;
      await bridge.emit();
      return client;
    },
    async setConnected(id, connected) {
      updateClient(id, { connected, lastSeenAt: Date.now() });
      await bridge.emit();
    },
    async expirePairing() {
      bridge.state.pairing = null;
      await bridge.emit();
    },
    async setStatus(status) {
      bridge.state.status = status;
      if (status !== "listening") bridge.state.pairing = null;
      await bridge.emit();
    },
  };

  function failure(command: string) {
    const error = bridge.failures.get(command);
    if (error) throw error;
  }

  simulator.handle("bridge_state", () => {
    failure("bridge_state");
    return snapshot();
  });
  simulator.handle("bridge_set_enabled", (args: Args) => {
    if (typeof args.enabled !== "boolean") {
      throw simulator.fail(
        `bridge_set_enabled: bad arguments ${JSON.stringify(args)}`,
      );
    }
    failure("bridge_set_enabled");
    bridge.state.enabled = args.enabled;
    bridge.state.status = args.enabled ? bridge.openStatus : "off";
    if (!args.enabled) {
      // Pont fermé : plus de code, connexions fermées ; l'appairage reste.
      bridge.state.pairing = null;
      const now = Date.now();
      bridge.state.clients = bridge.state.clients.map((client) =>
        client.connected
          ? { ...client, connected: false, lastSeenAt: now }
          : client,
      );
    }
    return snapshot();
  });
  simulator.handle("bridge_pair", () => {
    failure("bridge_pair");
    if (!bridge.state.enabled || bridge.state.status !== "listening") {
      throw invalid(PAIR_REFUSED);
    }
    lastCode += 1;
    bridge.state.pairing = {
      code: String(lastCode),
      expiresAt: Date.now() + BRIDGE_PAIRING_MS,
    };
    return snapshot();
  });
  simulator.handle("bridge_cancel_pairing", () => {
    failure("bridge_cancel_pairing");
    bridge.state.pairing = null;
    return snapshot();
  });
  simulator.handle("bridge_unlink", (args: Args) => {
    if (typeof args.clientId !== "string") {
      throw simulator.fail(
        `bridge_unlink: bad arguments ${JSON.stringify(args)}`,
      );
    }
    failure("bridge_unlink");
    bridge.state.clients = bridge.state.clients.filter(
      (client) => client.id !== args.clientId,
    );
    return snapshot();
  });

  return bridge;
}
