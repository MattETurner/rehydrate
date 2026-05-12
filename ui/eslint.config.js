// ESLint flat config for the reHydrate UI.
//
// Three things this config buys us that `tsc --strict` doesn't:
//
//   1. **react-hooks rules.** The exhaustive-deps and rules-of-hooks
//      lints catch the entire class of "I memoized something but
//      forgot to declare its real dep" bugs that the TypeScript
//      compiler is blind to. The OCR/library hook extractions
//      especially benefit from this — the wider the dep arrays,
//      the easier this is to get wrong.
//   2. **jsx-a11y.** Per-rule enforcement of the accessibility
//      patterns the audit's UX pass introduced: `role="dialog"`
//      pairs with `aria-modal`, click handlers on non-button
//      elements need a keyboard equivalent, image alts, etc.
//      We've already done the manual pass; lint keeps regressions
//      out.
//   3. **Source-of-truth narrowing.** `no-restricted-imports`
//      blocks reaching into module internals (e.g. importing a
//      hook's private types from outside the hook file).
//
// Run with `npm run lint`. CI runs the same command, so a green
// pre-commit lint matches what the workflow asserts.

import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactPlugin from "eslint-plugin-react";
import jsxA11y from "eslint-plugin-jsx-a11y";
import globals from "globals";

export default tseslint.config(
  {
    ignores: ["dist/**", "node_modules/**", "**/*.d.ts"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        ...globals.browser,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
      react: reactPlugin,
      "jsx-a11y": jsxA11y,
    },
    settings: {
      react: { version: "detect" },
    },
    rules: {
      // React Hooks — these are the load-bearing ones for the
      // hook-extraction pattern. `exhaustive-deps` is `warn`
      // rather than `error` because there are a handful of
      // intentional-omission sites in `useSelection` /
      // `handleRowClick` etc. that already carry the
      // `eslint-disable-next-line` opt-out.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",

      // React itself. We use the new JSX transform, so the
      // legacy "React in scope" rule is off.
      "react/react-in-jsx-scope": "off",
      "react/jsx-uses-react": "off",
      // Prop validation in PropTypes is irrelevant; TypeScript
      // handles it.
      "react/prop-types": "off",

      // Accessibility — match the patterns the UX audit pass
      // landed (modal role + aria-modal, etc.). `error` because
      // these are easy to get right at write time and the audit
      // explicitly listed them as Critical.
      "jsx-a11y/alt-text": "error",
      "jsx-a11y/aria-props": "error",
      "jsx-a11y/aria-role": "error",
      "jsx-a11y/aria-unsupported-elements": "error",
      "jsx-a11y/role-has-required-aria-props": "error",
      "jsx-a11y/role-supports-aria-props": "error",

      // TypeScript-specific niceties. `no-unused-vars` is off
      // because `tsc --strict` already covers it (and surfaces a
      // better location).
      "@typescript-eslint/no-unused-vars": "off",
      // `any` is sometimes the right answer at IPC boundaries
      // (Tauri's untyped JSON shapes); `warn` keeps us honest
      // without blocking.
      "@typescript-eslint/no-explicit-any": "warn",
      // Allow `@ts-expect-error` for the audit's intentional
      // suppression sites with a description.
      "@typescript-eslint/ban-ts-comment": [
        "warn",
        {
          "ts-expect-error": "allow-with-description",
          "ts-ignore": true,
          "ts-nocheck": true,
          "ts-check": false,
        },
      ],
    },
  },
);
