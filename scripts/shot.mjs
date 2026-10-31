/**
 * One screenshot of one view.
 *
 * `shoot.mjs` walks the original eighteen-screen tour; this is the smaller tool a new
 * view needs: point at a rail label, pick a theme and a viewport, capture. The app must
 * already be served (see `--base`) and Chrome must already be running with
 * `--remote-debugging-port=9222`.
 *
 * Usage:
 *   node scripts/shot.mjs --name extensions-1440-carbon --label Extensions
 *   node scripts/shot.mjs --name extensions-960-stage --label Extensions --width 960 --height 620 --theme stage
 *
 * `--theme` sets `data-theme` on the root element directly. That is a development
 * shortcut: it repaints the window with the real token block without walking the
 * Settings sheet. The Settings select itself is not moved.
 */
import { writeFileSync, mkdirSync, readFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const PORT = 9222;
const OUT = ".impeccable/review";

function readFlag(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index === -1 ? fallback : (process.argv[index + 1] ?? fallback);
}

const base = readFlag("base", "http://localhost:4173/");
const name = readFlag("name", null);
const labels = readFlag("label", "").split(",").map((value) => value.trim()).filter(Boolean);
const width = Number(readFlag("width", "1440"));
const height = Number(readFlag("height", "900"));
const theme = readFlag("theme", null);
const settle = Number(readFlag("wait", "700"));
const evalFile = readFlag("eval-file", null);
const evaluateBefore = evalFile === null ? readFlag("eval", null) : readFileSync(evalFile, "utf8");

if (!name) {
  console.error("--name is required");
  process.exit(1);
}

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((target) => target.type === "page");
if (!page) {
  console.error("No page target; start Chrome with --remote-debugging-port=9222 first.");
  process.exit(1);
}

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  const settlePromise = pending.get(message.id);
  if (settlePromise) {
    pending.delete(message.id);
    settlePromise(message);
  }
});

function send(method, params = {}) {
  const id = ++nextId;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
}

async function evaluate(expression) {
  const reply = await send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (reply.result?.exceptionDetails) {
    throw new Error(reply.result.exceptionDetails.exception?.description ?? "evaluate failed");
  }
  return reply.result?.result?.value;
}

async function clickByText(selector, text) {
  const result = await evaluate(`(() => {
    const node = [...document.querySelectorAll(${JSON.stringify(selector)})]
      .find((element) => element.textContent.trim().startsWith(${JSON.stringify(text)}));
    if (!node) return "missing:" + ${JSON.stringify(text)};
    node.click();
    return "clicked";
  })()`);
  if (result !== "clicked") {
    throw new Error(`could not find ${selector} matching ${text}`);
  }
}

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url: base });
await sleep(1600);

await send("Emulation.setDeviceMetricsOverride", {
  width,
  height,
  deviceScaleFactor: 1,
  mobile: false,
});
await sleep(300);

if (theme) {
  await evaluate(`document.documentElement.dataset.theme = ${JSON.stringify(theme)}`);
  await sleep(250);
}

for (const label of labels) {
  await clickByText(".rail-item", label);
  await sleep(settle);
}

if (evaluateBefore) {
  await evaluate(evaluateBefore);
  await sleep(settle);
}

const reply = await send("Page.captureScreenshot", {
  format: "png",
  captureBeyondViewport: false,
});

mkdirSync(OUT, { recursive: true });
writeFileSync(`${OUT}/${name}.png`, Buffer.from(reply.result.data, "base64"));
console.log(`${OUT}/${name}.png`);

socket.close();
