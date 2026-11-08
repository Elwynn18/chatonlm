// Carte « Report a bug » de Settings › General face au simulateur : juste
// au-dessus des dons ; rien ne part avant « Send » ni sous 10 caractères ;
// envoi du message, de l'adresse et du choix du diagnostic ; diagnostic
// montré à la demande ; chaque suite de l'envoi a son texte ; dossier des
// journaux ; textes des catalogues.

import {
  act,
  configure,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ReportOutcome } from "@/bindings/ReportOutcome";
import { setLanguage } from "@/i18n";
import { fakeBackend } from "@/test/backend";
import { untranslatedTexts, usePseudoLanguage } from "@/test/i18n";
import { renderApp } from "@/test/renderApp";
import { installSimulator } from "@/test/simulator";

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

const DIAGNOSTIC = "ChatonLM: 1.0.0\nSystem: Arch Linux\n";

function setup(outcome: ReportOutcome = "sent", path = "/settings") {
  const simulator = installSimulator();
  fakeBackend(simulator);
  simulator.handle("bug_report_diagnostic", () => DIAGNOSTIC);
  simulator.handle("send_bug_report", () => outcome);
  simulator.handle("open_app_logs", () => null);
  renderApp(path);
  return { simulator };
}

async function card() {
  return screen.findByRole("region", { name: "Report a bug" });
}

async function write(text: string) {
  const region = await card();
  const field = within(region).getByRole("textbox", {
    name: "What happened?",
  });
  await userEvent.type(field, text);
  return region;
}

describe("Report a bug card", () => {
  it("sits in General, right above the donation card", async () => {
    setup();
    const region = await card();
    const donate = screen.getByTestId("donate");
    const sections = [
      ...screen.getByTestId("settings").querySelectorAll("section"),
    ];
    expect(sections.indexOf(region)).toBe(sections.indexOf(donate) - 1);
    const reset = screen.getByRole("button", { name: "Reset to defaults" });
    expect(
      reset.compareDocumentPosition(region) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("is not shown in the other tabs", async () => {
    setup("sent", "/settings?section=models");
    await screen.findByRole("link", { name: "Models", current: "page" });
    expect(screen.queryByTestId("bug-report")).toBeNull();
  });

  it("can't be sent with fewer than 10 characters", async () => {
    const { simulator } = setup();
    const region = await write("  too short");
    const send = within(region).getByRole("button", { name: "Send" });
    // « too short » : 9 caractères une fois les espaces ôtés.
    expect((send as HTMLButtonElement).disabled).toBe(true);
    await userEvent.type(
      within(region).getByRole("textbox", { name: "What happened?" }),
      "!",
    );
    expect((send as HTMLButtonElement).disabled).toBe(false);
    expect(simulator.callsTo("send_bug_report")).toEqual([]);
  });

  it("sends the message, the reply address and the diagnostic choice", async () => {
    const { simulator } = setup();
    const region = await write("The window stays blank.");
    await userEvent.type(
      within(region).getByRole("textbox", { name: "Your email (optional)" }),
      "  me@example.org ",
    );
    await userEvent.click(within(region).getByRole("button", { name: "Send" }));
    expect(
      await within(region).findByText("Thank you! Your report has been sent."),
    ).toBeTruthy();
    expect(simulator.callsTo("send_bug_report")).toEqual([
      {
        report: {
          message: "The window stays blank.",
          email: "me@example.org",
          includeDiagnostic: true,
        },
      },
    ]);
    // Envoyé : le message est vidé, rien ne repart par mégarde.
    const field = within(region).getByRole("textbox", {
      name: "What happened?",
    }) as HTMLTextAreaElement;
    expect(field.value).toBe("");
  });

  it("can be sent without the diagnostic", async () => {
    const { simulator } = setup();
    const region = await write("The window stays blank.");
    await userEvent.click(
      within(region).getByRole("checkbox", {
        name: "Attach diagnostic information",
      }),
    );
    await userEvent.click(within(region).getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(simulator.callsTo("send_bug_report")).toEqual([
        {
          report: {
            message: "The window stays blank.",
            email: "",
            includeDiagnostic: false,
          },
        },
      ]),
    );
  });

  it("shows the diagnostic before sending, on demand", async () => {
    const { simulator } = setup();
    const region = await card();
    expect(simulator.callsTo("bug_report_diagnostic")).toEqual([]);
    const show = within(region).getByRole("button", { name: "Show" });
    expect(show.getAttribute("aria-expanded")).toBe("false");
    await userEvent.click(show);
    const shown = await within(region).findByLabelText(
      "Attach diagnostic information",
      { selector: "pre" },
    );
    expect(shown.textContent).toBe(DIAGNOSTIC);
    const hide = within(region).getByRole("button", { name: "Hide" });
    expect(hide.getAttribute("aria-expanded")).toBe("true");
    await userEvent.click(hide);
    expect(region.querySelector("pre")).toBeNull();
  });

  it.each([
    ["tooMany", "Too many reports were sent recently. Try again in an hour."],
    ["invalidEmail", "This email address doesn't look valid."],
    ["invalidMessage", "Write between 10 and 5000 characters."],
    [
      "unavailable",
      "The report couldn't be sent. Check your connection and try again, or write to contact.chatonlm@gmail.com.",
    ],
  ] as const)("a %s answer has its own text", async (outcome, text) => {
    setup(outcome);
    const region = await write("The window stays blank.");
    await userEvent.click(within(region).getByRole("button", { name: "Send" }));
    expect(await within(region).findByText(text)).toBeTruthy();
    // Pas envoyé : le message reste, pour réessayer.
    const field = within(region).getByRole("textbox", {
      name: "What happened?",
    }) as HTMLTextAreaElement;
    expect(field.value).toBe("The window stays blank.");
  });

  it("an address refused by the service marks the field", async () => {
    setup("invalidEmail");
    const region = await write("The window stays blank.");
    const email = within(region).getByRole("textbox", {
      name: "Your email (optional)",
    });
    await userEvent.type(email, "me@x");
    await userEvent.click(within(region).getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(email.getAttribute("aria-invalid")).toBe("true"),
    );
    await userEvent.type(email, ".org");
    expect(email.getAttribute("aria-invalid")).toBeNull();
  });

  it("a failed command shows its reason and the contact address", async () => {
    const { simulator } = setup();
    simulator.handle("send_bug_report", () => {
      throw { kind: "internal", message: "boom" };
    });
    const region = await write("The window stays blank.");
    await userEvent.click(within(region).getByRole("button", { name: "Send" }));
    expect(
      await within(region).findByText(
        "The report couldn't be sent (boom). Try again, or write to contact.chatonlm@gmail.com.",
      ),
    ).toBeTruthy();
  });

  it("opens the logs folder", async () => {
    const { simulator } = setup();
    const region = await card();
    await userEvent.click(
      within(region).getByRole("button", { name: "Open logs folder" }),
    );
    await waitFor(() =>
      expect(simulator.callsTo("open_app_logs")).toHaveLength(1),
    );
    simulator.handle("open_app_logs", () => {
      throw { kind: "system", message: "no file manager" };
    });
    await userEvent.click(
      within(region).getByRole("button", { name: "Open logs folder" }),
    );
    expect(
      await within(region).findByText(
        "Couldn't open the logs folder: no file manager",
      ),
    ).toBeTruthy();
  });

  it("has no untranslated text", async () => {
    setup();
    const region = await card();
    await userEvent.click(within(region).getByRole("button", { name: "Show" }));
    await within(region).findByText(/System: Arch Linux/);
    usePseudoLanguage();
    await screen.findByText("⟦Report a bug⟧");
    // Le diagnostic est en anglais technique, pas un texte d'interface.
    const texts = untranslatedTexts(screen.getByTestId("bug-report")).filter(
      (text) => !text.startsWith("ChatonLM: 1.0.0"),
    );
    expect(texts).toEqual([]);
  });

  it("speaks French and Spanish", async () => {
    setup();
    await card();
    act(() => setLanguage("fr"));
    expect(
      await screen.findByRole("region", { name: "Signaler un bug" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Envoyer" })).toBeTruthy();
    act(() => setLanguage("es"));
    expect(
      await screen.findByRole("region", { name: "Informar de un error" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Enviar" })).toBeTruthy();
  });
});
