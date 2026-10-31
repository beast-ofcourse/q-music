/**
 * Drives the built window through Chrome's DevTools protocol so the interface can be
 * clicked and photographed without the Tauri process attached. Development tool only:
 * it is never imported by the app and never shipped in the bundle.
 *
 * Usage: node scripts/shoot.mjs [base-url]
 * Requires Chrome started with --remote-debugging-port=9222.
 */
import { writeFileSync, mkdirSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const BASE = process.argv[2] ?? "http://localhost:4173/";
const OUT = ".impeccable/review";
const PORT = 9222;

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((target) => target.type === "page");
if (!page) throw new Error("No page target; is Chrome running with --remote-debugging-port?");

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 0;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  const settle = pending.get(message.id);
  if (settle) {
    pending.delete(message.id);
    settle(message);
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

async function viewport(width, height) {
  await send("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: false,
  });
}

async function shot(name) {
  const reply = await send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  mkdirSync(OUT, { recursive: true });
  writeFileSync(`${OUT}/${name}.png`, Buffer.from(reply.result.data, "base64"));
  console.log(`${name}.png`);
}

/** Clicks the first element whose trimmed text starts with `text`. */
function clickByText(selector, text) {
  return evaluate(`(() => {
    const node = [...document.querySelectorAll(${JSON.stringify(selector)})]
      .find((element) => element.textContent.trim().startsWith(${JSON.stringify(text)}));
    if (!node) return "missing:" + ${JSON.stringify(text)};
    node.click();
    return "clicked";
  })()`);
}

await send("Page.enable");
await send("Runtime.enable");
await send("Page.navigate", { url: BASE });
await sleep(1500);

await viewport(1440, 900);
await sleep(300);
await shot("01-library-idle");

// A double click on a row is the gesture that starts playback.
await evaluate(`(() => {
  document.querySelector(".track-row").dispatchEvent(
    new MouseEvent("dblclick", { bubbles: true }),
  );
  return "playing";
})()`);
await sleep(1600);
await shot("02-library-playing");

// Right-click the third row and add it to the first manual playlist.
await evaluate(`(() => {
  const row = document.querySelectorAll(".track-row")[2];
  const rect = row.getBoundingClientRect();
  row.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      clientX: Math.round(rect.left + 220),
      clientY: Math.round(rect.top + 12),
    }),
  );
  return "menu";
})()`);
await sleep(400);
await shot("03-row-menu");

await clickByText(".row-menu-item", "Late listening");
await sleep(700);
await clickByText(".rail-item", "Late listening");
await sleep(700);
await shot("04-playlist-after-add");

// Typing goes through the native setter so React's controlled input sees the change.
await clickByText(".rail-item", "Library");
await sleep(500);
await evaluate(`(() => {
  const input = document.getElementById("library-search");
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  setter.call(input, "blue");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  return "typed";
})()`);
await sleep(800);
await shot("05-search");
await evaluate(`(() => {
  const input = document.getElementById("library-search");
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
  setter.call(input, "");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  return "cleared";
})()`);
await sleep(700);

await clickByText(".now-tab", "Lyrics");
await sleep(400);
await shot("06-lyrics-empty");

await clickByText(".rail-label", "Playlists");
await sleep(600);
await shot("07-playlists");

await clickByText(".rail-label", "Folders");
await sleep(600);
await shot("08-folders");

await clickByText(".rail-item", "Albums");
await sleep(600);
await shot("09-albums");

await clickByText(".rail-item", "Artists");
await sleep(600);
await shot("10-artists");

await clickByText(".rail-item", "Equaliser");
await sleep(500);
await shot("11-equaliser");

await clickByText(".rail-item", "Settings");
await sleep(500);
await shot("12-settings-top");

await evaluate(`(() => {
  const body = document.querySelector(".view-body");
  body.scrollTop = body.scrollHeight;
  return "scrolled";
})()`);
await sleep(400);
await shot("13-settings-bottom");

await clickByText(".rail-item", "Library");
await sleep(600);
await evaluate(`(() => {
  const row = document.querySelectorAll(".track-row")[1];
  row.click();
  const rect = row.getBoundingClientRect();
  row.dispatchEvent(new MouseEvent("contextmenu", {
    bubbles: true,
    clientX: Math.round(rect.left + 220),
    clientY: Math.round(rect.top + 12),
  }));
  return "menu";
})()`);
await sleep(400);
await clickByText(".row-menu-item", "Edit tags");
await sleep(700);
await shot("14-editor");

await clickByText(".rail-item", "Library");
await sleep(600);
await viewport(960, 620);
await send("Page.reload");
await sleep(1800);
await shot("15-minimum-960");

await clickByText(".now-toggle", "");
await sleep(500);
await shot("16-minimum-960-panel");

await viewport(1440, 900);
await clickByText(".rail-item", "Settings");
await sleep(400);
await evaluate(`(() => {
  const select = document.querySelector(".panel .select");
  select.value = "paper";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  return select.value;
})()`);
await sleep(500);
await clickByText(".rail-item", "Library");
await sleep(700);
await shot("17-paper-theme");

await clickByText(".rail-item", "Settings");
await sleep(400);
await evaluate(`(() => {
  const select = document.querySelector(".panel .select");
  select.value = "stage";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  return select.value;
})()`);
await sleep(500);
await clickByText(".rail-item", "Library");
await sleep(700);
await shot("18-stage-theme");

socket.close();
