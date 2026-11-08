import { execSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, watch } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const cargoToml = join(root, "src-tauri", "Cargo.toml");
const cargoBin = join(homedir(), ".cargo", "bin");
const cargoExe = join(cargoBin, process.platform === "win32" ? "cargo.exe" : "cargo");
if (existsSync(cargoExe)) {
  process.env.PATH = `${cargoBin}${delimiter}${process.env.PATH ?? ""}`;
}

function bridgeBin() {
  const toml = readFileSync(cargoToml, "utf8");
  const m = toml.match(/\[\[bin\]\]\s*name\s*=\s*"([^"]+)"/);
  return m?.[1] ?? "arduloops-bridge";
}

function bridgeArgs() {
  return [
    "run",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--bin",
    bridgeBin(),
    "--no-default-features",
    "--",
    "--migrate-force",
  ];
}

function startApi() {
  const apiRoot = join(root, "..", "arduloops-api");
  // The API is TypeScript (src/server.ts, run with its own tsx). Older checkouts had src/server.mjs.
  const tsx = join(apiRoot, "node_modules", "tsx", "dist", "cli.mjs");
  const ts = join(apiRoot, "src", "server.ts");
  const legacy = join(apiRoot, "src", "server.mjs");
  const args = existsSync(ts) && existsSync(tsx) ? [tsx, ts] : existsSync(legacy) ? [legacy] : null;
  if (!args) return;
  process.env.ARDULOOPS_API = process.env.ARDULOOPS_API || "https://api-qm4zps6yqa-ew.a.run.app";
  const child = spawn(process.execPath, args, {
    stdio: "inherit",
    cwd: apiRoot,
    env: { ...process.env, ARDULOOPS_DEV: process.env.ARDULOOPS_DEV || "1", PORT: "8788" },
  });
  child.on("exit", (code) => {
    if (stopping) return;
    console.log(`arduloops-api exited (${code ?? 0})`);
  });
  return child;
}

let apiProc = null;
let stopping = false;
let viteProc = null;
let bridgeProc = null;
let bridgeGen = 0;
let restartTimer = 0;

function killTree(child) {
  if (!child?.pid) return;
  if (process.platform === "win32") {
    spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    child.kill("SIGTERM");
  }
}

/** Drop the bridge exe without /T so ArduCopter/ArduPlane (its child) can stay up. */
function killBridgeBinary() {
  if (process.platform !== "win32") return;
  spawn("taskkill", ["/IM", `${bridgeBin()}.exe`, "/F"], { stdio: "ignore" });
}

function listeningPid(port) {
  try {
    const out = execSync("netstat -ano", { encoding: "utf8", windowsHide: true });
    const re = new RegExp(`[:\\[]${port}(?:\\]|\\s)\\s+\\S+\\s+LISTENING\\s+(\\d+)`, "i");
    const m = out.match(re);
    return m ? m[1] : null;
  } catch {
    return null;
  }
}

function assertPortFree(port, label) {
  const pid = listeningPid(port);
  if (!pid) return;
  const kill = process.platform === "win32" ? `taskkill /PID ${pid} /F` : `kill ${pid}`;
  console.error(`${label} :${port} is already in use (PID ${pid}).`);
  console.error(`  ${kill}`);
  process.exit(1);
}

function startVite() {
  viteProc = spawn(process.execPath, [join(root, "node_modules", "vite", "bin", "vite.js")], {
    stdio: "inherit",
    cwd: root,
    env: process.env,
  });
  viteProc.on("exit", (code) => {
    if (stopping) return;
    stopping = true;
    killTree(bridgeProc);
    process.exit(code ?? 1);
  });
}

function startBridge() {
  const gen = ++bridgeGen;
  console.log("headless MAVLink: cargo run (watch src-tauri)");
  const child = spawn(cargoExe, bridgeArgs(), {
    stdio: "inherit",
    cwd: root,
    env: process.env,
  });
  bridgeProc = child;
  child.on("exit", (code) => {
    if (stopping || gen !== bridgeGen) return;
    bridgeProc = null;
    if (code) {
      console.log(`headless MAVLink exited (${code}). waiting for src-tauri change…`);
    }
  });
}

function restartBridge() {
  const gen = ++bridgeGen;
  const old = bridgeProc;
  bridgeProc = null;
  const launch = () => {
    if (stopping || gen !== bridgeGen) return;
    startBridge();
  };
  if (old?.pid) {
    old.once("exit", launch);
    killBridgeBinary();
    setTimeout(() => killTree(old), 700);
    setTimeout(launch, 2500);
  } else {
    launch();
  }
}

function scheduleBridgeRestart(reason) {
  clearTimeout(restartTimer);
  restartTimer = setTimeout(() => {
    console.log(`bridge: reload (${reason})`);
    restartBridge();
  }, 400);
}

function shouldReload(filename) {
  if (!filename) return false;
  const n = filename.replaceAll("\\", "/");
  if (n.includes("/target/") || n.startsWith("target/") || n.includes("/gen/")) return false;
  if (n.includes("/migrations/") && n.endsWith(".sql")) return true;
  if (!/\.(rs|toml)$/.test(n)) return false;
  // tests.rs is included under cfg(test). The running bridge does not contain it.
  if (/(^|\/)tests\.rs$/.test(n) || /_tests\.rs$/.test(n) || /(^|\/)(tests|benches)\//.test(n)) return false;
  return true;
}

function onStop() {
  stopping = true;
  clearTimeout(restartTimer);
  killTree(bridgeProc);
  killTree(viteProc);
  killTree(apiProc);
  process.exit(0);
}
process.on("SIGINT", onStop);
process.on("SIGTERM", onStop);

if (!existsSync(cargoExe)) {
  console.error("cargo not found. Install rustup (https://rustup.rs).");
  process.exit(1);
}

console.log("ArduLoops  http://127.0.0.1:5173  (Vite HMR + Rust watch)");
assertPortFree(5173, "Vite");
assertPortFree(8767, "MAVLink HTTP");
assertPortFree(8788, "ArduLoops API");
apiProc = startApi();
startBridge();
startVite();

// Windows reports change events for files whose bytes did not change (an indexer, antivirus,
// an editor touching the file). A restart kills the bridge mid log download, so restart only
// when the content is new.
const seen = new Map();
function contentHash(path) {
  try {
    return createHash("sha1").update(readFileSync(path)).digest("hex");
  } catch {
    return "gone";
  }
}
function changed(path) {
  const next = contentHash(path);
  if (seen.get(path) === next) return false;
  seen.set(path, next);
  return true;
}
function remember(dir, rel = "") {
  for (const entry of readdirSync(join(dir, rel), { withFileTypes: true })) {
    const name = rel ? `${rel}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      if (entry.name === "target" || entry.name.startsWith("target") || entry.name === "gen" || entry.name === "node_modules") continue;
      remember(dir, name);
    } else if (shouldReload(name)) {
      seen.set(join(dir, name), contentHash(join(dir, name)));
    }
  }
}

const tauriDir = join(root, "src-tauri");
remember(tauriDir);
watch(tauriDir, { recursive: true }, (_event, filename) => {
  if (stopping || !shouldReload(filename ?? "")) return;
  if (!changed(join(tauriDir, filename))) return;
  scheduleBridgeRestart(filename);
});
const skillFile = join(root, "skills", "ROOT.md");
if (existsSync(skillFile)) {
  seen.set(skillFile, contentHash(skillFile));
  watch(skillFile, () => {
    if (stopping || !changed(skillFile)) return;
    scheduleBridgeRestart("skills/ROOT.md");
  });
}
