// Runs the end-to-end suite in VS Code against a real `annox lsp`.
//
//   ANNOX_BIN=target/debug/annox npm run test:e2e
//
// VSCODE_BIN picks the VS Code to run; by default the latest stable release
// is downloaded into .vscode-test/.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { runTests } from "@vscode/test-electron";
import * as esbuild from "esbuild";

const here = path.dirname(fileURLToPath(import.meta.url));
const extension = path.resolve(here, "../..");
const bin = path.resolve(process.env.ANNOX_BIN ?? path.join(extension, "../../target/debug/annox"));
if (!fs.existsSync(bin)) {
  console.error(`annox binary not found at ${bin}; build it with \`cargo build -p annox-lsp\` or set ANNOX_BIN`);
  process.exit(1);
}

// The extension and the suite, built the way `npm run build` builds the extension.
const build = { bundle: true, external: ["vscode"], format: "cjs", platform: "node", target: "node20", sourcemap: true };
await esbuild.build({ ...build, entryPoints: [path.join(extension, "src/extension.ts")], outfile: path.join(extension, "dist/extension.js") });
await esbuild.build({
  bundle: true,
  format: "iife",
  platform: "browser",
  target: "es2022",
  entryPoints: [path.join(extension, "src/sidebar/webview.ts")],
  outfile: path.join(extension, "dist/sidebar.js"),
});
await esbuild.build({ ...build, entryPoints: [path.join(here, "suite.ts")], outfile: path.join(extension, "out/suite.js") });

const log = path.join(extension, "out/e2e.log");
fs.rmSync(log, { force: true });
const root = fs.mkdtempSync(path.join(os.tmpdir(), "annox-vscode-"));
fs.mkdirSync(path.join(root, ".annox"));
fs.writeFileSync(path.join(root, ".annox/annox.json"), '{ "format": 1 }\n');
fs.writeFileSync(
  path.join(root, "paper.tex"),
  [
    "\\section{Results}",
    "In Section 3, we prove that the bound is tight.",
    "The constant is small and the proof is short.",
    "We conclude with open problems.",
    "Suggestion mode writes here.",
    "",
  ].join("\n"),
);
fs.mkdirSync(path.join(root, ".vscode"));
fs.writeFileSync(
  path.join(root, ".vscode/settings.json"),
  JSON.stringify({
    "annox.path": bin,
    "annox.author.id": "mailto:ada@example.org",
    "annox.author.name": "Ada",
    "annox.suggestionDelay": 150,
    "git.enabled": false,
    "workbench.startupEditor": "none",
  }),
);

try {
  await runTests({
    vscodeExecutablePath: process.env.VSCODE_BIN || undefined,
    extensionDevelopmentPath: extension,
    extensionTestsPath: path.join(extension, "out/suite.js"),
    extensionTestsEnv: { ANNOX_TEST_ROOT: root, ANNOX_TEST_LOG: log },
    launchArgs: [
      root,
      "--disable-extensions",
      "--disable-workspace-trust",
      "--skip-welcome",
      "--skip-release-notes",
      "--user-data-dir",
      path.join(os.tmpdir(), `annox-vscode-user-${process.pid}`),
    ],
  });
  if (!fs.readFileSync(log, "utf8").includes("annox vscode e2e: OK")) throw new Error("the suite did not finish");
} catch (e) {
  if (fs.existsSync(log)) console.error(fs.readFileSync(log, "utf8"));
  console.error("annox vscode e2e: FAILED", e);
  process.exit(1);
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
