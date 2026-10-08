// Builds crates/wasm for the browser and generates the JS bindings into build/wasm.
// Requires `wasm-bindgen-cli` matching the wasm-bindgen crate version
// (`cargo install wasm-bindgen-cli --version <version from Cargo.lock>`).
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const outDir = `${root}/build/wasm`;
const run = (cmd) => {
  console.log(`> ${cmd}`);
  execSync(cmd, { cwd: root, stdio: "inherit" });
};

mkdirSync(outDir, { recursive: true });
run("cargo build --release -p pipit-wasm --target wasm32-unknown-unknown");
run(
  `wasm-bindgen --target web --out-dir "${outDir}" --out-name pipit_wasm ` +
    `"${root}/build/target/wasm32-unknown-unknown/release/pipit_wasm.wasm"`,
);
