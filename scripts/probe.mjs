/**
 * Print the value of an expression evaluated in the running window.
 *
 * `shot.mjs` is for pixels; this is for facts a screenshot cannot prove - the order of
 * rows in the DOM, the text of a live region, which element holds focus. Every run
 * navigates to the base URL first, so a second run after a change is a genuine reload
 * and not a stale page.
 *
 * Usage:
 *   node scripts/probe.mjs --read "document.title"
 *   node scripts/probe.mjs --eval "<js>" --read "<js>"
 *
 * Requires Chrome started with `--remote-debugging-port=9222` and the app served.
 */
import { setTimeout as sleep } from "node:timers/promises";
import { readFileSync } from "node:fs";

const PORT = 9222;

function readFlag(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  return index === -1 ? fallback : (process.argv[index + 1] ?? fallback);
}

/** A one-off expression is too easy to mangle on a command line, so it can live in a file. */
function readExpression(flag) {
  const fromFile = readFlag(`${flag}-file`, null);
  if (fromFile !== null) {
    return readFileSync(fromFile, "utf8");
  }
  return readFlag(flag, null);
}

const base = readFlag("base", "http://localhost:4173/");
const before = readExpression("eval");
const read = readExpression("read");
const settle = Number(readFlag("wait", "700"));
/** Preference emulation, so a media query can be checked as the browser applies it. */
const reducedMotion = process.argv.includes("--reduced-motion");

if (read === null) {
  console.error("--read or --read-file is required");
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

await send("Page.enable");
await send("Runtime.enable");
if (reducedMotion) {
  await send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-reduced-motion", value: "reduce" }],
  });
}
await send("Page.navigate", { url: base });
await sleep(1600);

if (before !== null) {
  await evaluate(before);
  await sleep(settle);
}

console.log(JSON.stringify(await evaluate(read), null, 2));
socket.close();
