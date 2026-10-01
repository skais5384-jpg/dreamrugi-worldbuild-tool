import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import eslintConfigPrettier from "eslint-config-prettier/flat";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default defineConfig([
  globalIgnores([
    "node_modules/**",
    "dist/**",
    "src-tauri/**",
    "logs/**",
    ".agents/**",
    "design-system/**",
  ]),
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["*.{js,ts}", "scripts/*.mjs"],
    languageOptions: {
      globals: globals.node,
    },
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      globals: globals.browser,
    },
    plugins: reactHooks.configs.flat.recommended.plugins,
    rules: {
      ...reactHooks.configs.flat.recommended.rules,
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@fluentui/react-components",
              importNames: ["MessageBar", "MessageBarBody"],
              message:
                "Route notifications through ui/FloatingNotice and the lower-left activity log.",
            },
          ],
        },
      ],
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
        },
      ],
    },
  },
  eslintConfigPrettier,
]);
