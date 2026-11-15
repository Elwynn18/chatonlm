// Préparation de chaque fichier de test : l'interface en anglais (langue
// source, textes de l'officiel), rétablie après un test qui en change ; le
// magasin du mode vocal (global, hors de React) remis à zéro, comme les choix
// des groupes d'étapes de la conversation ; le système
// (Linux par défaut) rétabli après un test qui se place sous Windows.
import { afterEach } from "vitest";
import { resetVoiceModeForTests } from "@/hooks/useVoiceMode";
import { setLanguage, setPseudoForTests } from "@/i18n";
import { resetPlatformForTests } from "@/lib/platform";
import { resetStepChoicesForTests } from "@/lib/stepGroups";

afterEach(() => {
  setLanguage("en");
  setPseudoForTests(false);
  resetVoiceModeForTests();
  resetPlatformForTests();
  resetStepChoicesForTests();
});
