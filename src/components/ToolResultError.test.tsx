// Erreur d'un appel d'outil web dans la conversation : phrase du code dans la
// langue de la page ; repli sur le texte anglais enregistré (celui du
// modèle) pour une conversation d'avant les codes.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ToolCall } from "@/bindings/ToolCall";
import { ToolResult } from "./ToolCalls";

afterEach(cleanup);

const FORBIDDEN =
  "failed to fetch the page: the server answered with status 403";

const FETCH: ToolCall = {
  name: "web_fetch",
  arguments: JSON.stringify({ url: "https://example.com/page" }),
  result: null,
  error: FORBIDDEN,
};

describe("ToolResult error", () => {
  it("shows the sentence of the code instead of the text for the model", () => {
    render(
      <ToolResult
        call={{ ...FETCH, errorCode: "accessDenied", errorStatus: 403 }}
      />,
    );
    expect(
      screen.getByText(
        "Error: The website refused access to the page (error 403). Some websites block automated reading.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/failed to fetch/)).toBeNull();
  });

  it("shows the saved text of an older chat without a code", () => {
    render(<ToolResult call={FETCH} />);
    expect(screen.getByText(`Error: ${FORBIDDEN}`)).toBeTruthy();
  });
});
