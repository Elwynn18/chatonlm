// « Erase my data » : chaque magasin de module de la page qui garde un texte
// de l'utilisateur est vidé par forgetPageMemory.

import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  claimSelectionRequest,
  getProofreadState,
  setDraft,
} from "@/hooks/useProofread";
import { askKey, startStudyStream, useStudyStream } from "@/hooks/useStudy";
import {
  adoptVoiceModeChat,
  peekVoiceModeDraft,
  registerVoiceModeDraft,
  startVoiceMode,
} from "@/hooks/useVoiceMode";
import { newChatIsEphemeral, setNewChatEphemeral } from "@/lib/ephemeral";
import { forgetPageMemory } from "@/lib/forgetPage";
import { resetStreams, start, streamsFor } from "@/lib/streams";
import { fakeBackend } from "@/test/backend";
import { installSimulator } from "@/test/simulator";

describe("forgetPageMemory", () => {
  it("empties every page store that holds the user's text", async () => {
    const simulator = installSimulator();
    fakeBackend(simulator);
    simulator.handle("study_ask", () => new Promise(() => {}));

    // Page Proofread : texte, et une demande « Show » déjà servie.
    act(() => setDraft("my secret letter"));
    expect(claimSelectionRequest("show-1")).toBe(true);
    // Étude : une question posée en cours.
    const key = askKey("course", 0);
    const study = renderHook(() => useStudyStream(key));
    act(() =>
      startStudyStream(
        key,
        { kind: "ask", courseId: "course", level: 0, question: "secret?" },
        0,
      ),
    );
    expect(study.result.current?.question).toBe("secret?");
    // Conversation : un envoi en cours.
    start({ streamId: "s1", chatId: "c1", content: "secret", model: "m" });
    expect(streamsFor("c1")).toHaveLength(1);
    // Mode vocal : brouillon cédé à la conversation créée.
    void startVoiceMode("new");
    registerVoiceModeDraft("new", () => ({ content: "draft", files: [] }));
    adoptVoiceModeChat("new", "c2");
    expect(peekVoiceModeDraft("c2")?.content).toBe("draft");
    setNewChatEphemeral(true);

    act(() => forgetPageMemory());

    expect(getProofreadState().draft).toBe("");
    expect(claimSelectionRequest("show-1")).toBe(true);
    expect(study.result.current).toBeNull();
    expect(streamsFor("c1")).toHaveLength(0);
    expect(peekVoiceModeDraft("c2")).toBeNull();
    expect(newChatIsEphemeral()).toBe(false);
    resetStreams();
  });
});
