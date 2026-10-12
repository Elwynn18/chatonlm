// Briques visuelles du cours (ui.tsx).
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "@/components/study/ui";

describe("Markdown du cours", () => {
  it("retire la marge haute du premier bloc et la marge basse du dernier", () => {
    // Structure rendue : .prose > div > p… ; sans ce rognage, la marge du
    // dernier paragraphe laisse un vide sous le texte des bulles et des
    // cartes (vu dans l'app réelle).
    const { container } = render(
      <Markdown content={"First paragraph.\n\nSecond paragraph."} />,
    );
    const wrapper = container.querySelector("[data-study-markdown]");
    expect(wrapper).not.toBeNull();
    const classes = wrapper!.className.split(" ");
    expect(classes).toContain("[&_.prose>div>:first-child]:mt-0");
    expect(classes).toContain("[&_.prose>div>:last-child]:mb-0");
    // Le sélecteur vise bien la structure rendue.
    expect(
      wrapper!.querySelectorAll(".prose > div > :last-child").length,
    ).toBeGreaterThan(0);
  });
});
