// Texte montré de l'erreur d'un appel d'outil : phrase du code dans la
// langue de la page, sinon le texte enregistré (donné au modèle).
import { describe, expect, it } from "vitest";
import type { ToolCallErrorCode } from "@/bindings/ToolCallErrorCode";
import { translator } from "@/i18n";
import { toolCallErrorText } from "./toolCallError";

const en = translator("en");
const fr = translator("fr");
const FORBIDDEN =
  "failed to fetch the page: the server answered with status 403";

describe("toolCallErrorText", () => {
  it("shows the sentence of the code, with its status, in the page's language", () => {
    const call = {
      error: FORBIDDEN,
      errorCode: "accessDenied" as const,
      errorStatus: 403,
    };
    expect(toolCallErrorText(en, call)).toBe(
      "Error: The website refused access to the page (error 403). Some websites block automated reading.",
    );
    expect(toolCallErrorText(fr, call)).toBe(
      "Erreur\u00a0: Le site a refusé l'accès à la page (erreur 403). Certains sites bloquent les lectures automatiques.",
    );
  });

  it("shows a code without status", () => {
    expect(
      toolCallErrorText(fr, {
        error: "failed to fetch the page: timed out",
        errorCode: "timeout",
      }),
    ).toBe("Erreur\u00a0: La page a mis trop de temps à répondre.");
  });

  it("falls back to the saved text without a code (older chats, images)", () => {
    expect(toolCallErrorText(fr, { error: FORBIDDEN })).toBe(
      `Erreur\u00a0: ${FORBIDDEN}`,
    );
  });

  it("falls back to the saved text when the status of the sentence is missing", () => {
    expect(
      toolCallErrorText(en, { error: FORBIDDEN, errorCode: "accessDenied" }),
    ).toBe(`Error: ${FORBIDDEN}`);
  });

  it("shows nothing without an error", () => {
    expect(
      toolCallErrorText(en, { error: null, errorCode: "timeout" }),
    ).toBeNull();
  });

  it("has a sentence for every code, in every language, with its status", () => {
    const codes: Record<ToolCallErrorCode, true> = {
      accessDenied: true,
      pageNotFound: true,
      tooManyRequests: true,
      serverError: true,
      httpStatus: true,
      timeout: true,
      network: true,
      unknownHost: true,
      privateAddress: true,
      invalidUrl: true,
      tooManyRedirects: true,
      unsupportedContent: true,
      tooLarge: true,
      urlNotProvided: true,
      searchFailed: true,
      invalidQuery: true,
      unknownTool: true,
      invalidArguments: true,
      internal: true,
    };
    for (const t of [en, fr, translator("es")]) {
      for (const code of Object.keys(codes) as ToolCallErrorCode[]) {
        const text = toolCallErrorText(t, {
          error: "raw",
          errorCode: code,
          errorStatus: 418,
        });
        expect(text, code).not.toContain("raw");
        expect(text, code).not.toContain("{");
        expect(text, code).not.toContain("chat.tools");
      }
    }
  });
});
