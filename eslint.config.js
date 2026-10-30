import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";
import tseslint from "typescript-eslint";

/** Attributs et propriétés qui portent un texte montré ou lu à l'utilisateur. */
const TEXT_PROPS =
  "/^(aria-label|aria-description|aria-valuetext|aria-roledescription|title|placeholder|alt|label|description|message|hint|tooltip|heading|text|confirmLabel|cancelLabel)$/";

/** Expression enfant d'un élément JSX ({…} entre ses balises). */
const JSX_CHILD = ":matches(JSXElement, JSXFragment) > JSXExpressionContainer";
/** Branche d'un choix (a ? b : c, a && b, a ?? b). */
const BRANCH = ":matches(ConditionalExpression, LogicalExpression)";
/** Chaîne avec des lettres qui n'est pas une clé de traduction. */
const UI_TEXT =
  "/^(?!(common|chat|settings|dictation|hub|vscode)[.][A-Za-z]).*[A-Za-z]{2,}/s";

/** Appel qui montre un texte dans VS Code (hôte de l'extension). */
const VSCODE_SHOW =
  "CallExpression[callee.property.name=/^show(Information|Warning|Error)Message$/]";
/** Propriétés d'options de VS Code qui portent un texte montré. */
const VSCODE_TEXT_PROPS = "/^(title|prompt|placeHolder|tooltip|label|detail)$/";

export default defineConfig([
  globalIgnores([
    ".tools/",
    "dist/",
    "vscode/dist/",
    "site/dist/",
    "src-tauri/",
    "src/bindings/",
    "src/routeTree.gen.ts",
    "test-results/",
    "playwright-report/",
  ]),
  {
    files: ["**/*.{ts,tsx}"],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2023,
      globals: globals.browser,
    },
  },
  {
    // Un fichier de route TanStack exporte toujours « Route » à côté de son
    // composant ; le plugin du routeur gère le rechargement à chaud.
    files: ["src/routes/**/*.tsx"],
    rules: { "react-refresh/only-export-components": "off" },
  },
  {
    // Textes de l'interface : toujours par t() (src/i18n), dans les trois
    // langues (src/locales/*). Les essais peuvent écrire du texte en dur.
    files: ["src/**/*.tsx", "vscode/webview/**/*.tsx"],
    ignores: ["src/**/*.test.tsx", "src/test/**", "vscode/**/*.test.tsx"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector: "JSXText[value=/[A-Za-z]{2,}/]",
          message:
            "Texte d'interface en dur : passer par t() (src/i18n) et l'écrire dans src/locales/{en,es,fr}.",
        },
        {
          selector: `JSXAttribute[name.name=${TEXT_PROPS}] > Literal[value=/[A-Za-z]{2,}/]`,
          message:
            "Texte d'interface en dur dans un attribut : passer par t() (src/i18n).",
        },
        {
          selector: `JSXAttribute[name.name=${TEXT_PROPS}] > JSXExpressionContainer > Literal[value=/[A-Za-z]{2,}/]`,
          message:
            "Texte d'interface en dur dans un attribut : passer par t() (src/i18n).",
        },
        {
          selector: `JSXAttribute[name.name=${TEXT_PROPS}] TemplateElement[value.raw=/[A-Za-z]{2,}/]`,
          message:
            "Texte d'interface en dur dans un attribut : passer par t() (src/i18n).",
        },
        {
          // {saving ? "Saving…" : "Save"}, {open && "Close"}, {"Retry"},
          // {`${n} models`}, aria-label={open ? "Hide" : "Show"} ; pas les
          // clés de traduction ni les classes d'un attribut className.
          selector: [
            `${JSX_CHILD} > Literal[value=${UI_TEXT}]`,
            `${JSX_CHILD} > ${BRANCH} > Literal[value=${UI_TEXT}]`,
            `${JSX_CHILD} > ${BRANCH} > ${BRANCH} > Literal[value=${UI_TEXT}]`,
            `${JSX_CHILD} > TemplateLiteral > TemplateElement[value.raw=/[A-Za-z]{2,}/]`,
            `${JSX_CHILD} > ${BRANCH} > TemplateLiteral > TemplateElement[value.raw=/[A-Za-z]{2,}/]`,
            `JSXAttribute[name.name=${TEXT_PROPS}] > JSXExpressionContainer > ${BRANCH} > Literal[value=${UI_TEXT}]`,
          ].join(", "),
          message:
            "Texte d'interface en dur dans une expression JSX : passer par t() (src/i18n).",
        },
      ],
    },
  },
  {
    // Hôte de l'extension VS Code : Node, textes par vscode.l10n.t (anglais
    // = clé, traductions dans vscode/l10n/bundle.l10n.{es,fr}.json).
    files: ["vscode/src/**/*.ts"],
    languageOptions: { globals: globals.node },
  },
  {
    files: ["vscode/src/**/*.ts"],
    ignores: ["vscode/src/**/*.test.ts", "vscode/src/test/**"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector: `${VSCODE_SHOW} > :matches(Literal, TemplateLiteral)`,
          message:
            "Texte montré en dur : passer par vscode.l10n.t et l'écrire dans vscode/l10n (es, fr).",
        },
        {
          selector: `Property[key.name=${VSCODE_TEXT_PROPS}] > :matches(Literal[value=/[A-Za-z]{2,}/], TemplateLiteral)`,
          message:
            "Texte montré en dur : passer par vscode.l10n.t et l'écrire dans vscode/l10n (es, fr).",
        },
      ],
    },
  },
  {
    files: ["**/*.{js,mjs}"],
    extends: [js.configs.recommended],
    languageOptions: {
      ecmaVersion: 2023,
      globals: globals.node,
    },
  },
  {
    // Scripts des pages du site de présentation : ils tournent dans le navigateur.
    files: ["site/src/**/*.js"],
    languageOptions: { globals: globals.browser },
  },
]);
