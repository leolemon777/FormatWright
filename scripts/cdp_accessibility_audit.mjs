import fs from "node:fs";
import path from "node:path";

const port = Number(process.argv[2]);
const artifactDirectory = path.resolve(process.argv[3]);
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error(`invalid DevTools port: ${process.argv[2]}`);
}
fs.mkdirSync(artifactDirectory, { recursive: true });

function assert(condition, message) {
  if (!condition) throw new Error(`accessibility assertion failed: ${message}`);
}

async function getTarget() {
  const deadline = Date.now() + 30_000;
  let lastError;
  let stableTargetId;
  let stableSince = 0;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
        signal: AbortSignal.timeout(1_000),
      });
      const targets = await response.json();
      const target = targets.find((candidate) =>
        candidate.type === "page" &&
        candidate.url === "http://tauri.localhost/" &&
        candidate.webSocketDebuggerUrl
      );
      if (target) {
        if (target.id !== stableTargetId) {
          stableTargetId = target.id;
          stableSince = Date.now();
        } else if (Date.now() - stableSince >= 1_500) {
          return target;
        }
      }
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`WebView DevTools target did not appear: ${lastError ?? "no page target"}`);
}

class CdpClient {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.sequence = 0;
    this.pending = new Map();
    this.socket.addEventListener("close", (event) => {
      const error = new Error(`DevTools WebSocket closed (${event.code}: ${event.reason})`);
      for (const request of this.pending.values()) request.reject(error);
      this.pending.clear();
    });
  }

  async open() {
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) return;
      const request = this.pending.get(message.id);
      if (!request) return;
      this.pending.delete(message.id);
      if (message.error) request.reject(new Error(`${request.method}: ${message.error.message}`));
      else request.resolve(message.result);
    });
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("DevTools WebSocket open timed out")), 10_000);
      this.socket.addEventListener("open", () => {
        clearTimeout(timer);
        resolve();
      }, { once: true });
      this.socket.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error("DevTools WebSocket failed to open"));
      }, { once: true });
    });
  }

  command(method, params = {}) {
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out${params.expression ? `: ${params.expression.slice(0, 80)}` : ""}`));
      }, 15_000);
      this.pending.set(id, {
        method,
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const response = await this.command("Runtime.evaluate", {
      expression,
      returnByValue: true,
    });
    if (response.exceptionDetails) {
      throw new Error(response.exceptionDetails.exception?.description ?? "Runtime.evaluate failed");
    }
    return response.result.value;
  }

  close() {
    this.socket.close();
  }
}

function axValue(node, property) {
  return node.properties?.find((candidate) => candidate.name === property)?.value?.value;
}

async function waitFor(client, expression, description) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (await client.evaluate(expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function key(client, key, code) {
  await client.command("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code });
  await client.command("Input.dispatchKeyEvent", { type: "keyUp", key, code });
}

const target = await getTarget();
const client = new CdpClient(target.webSocketDebuggerUrl);
await client.open();

try {
  await client.command("Runtime.enable");
  await client.command("Accessibility.enable");
  await new Promise((resolve) => setTimeout(resolve, 1_000));
  await waitFor(
    client,
    "document.readyState === 'complete' && Boolean(document.querySelector('.shell'))",
    "FormatWright document",
  );

  const initial = await client.evaluate(`(() => ({
    lang: document.documentElement.lang,
    navigationLabel: document.querySelector('header nav')?.getAttribute('aria-label'),
    activeNavigation: document.querySelector('header nav [aria-current="page"]')?.textContent?.trim(),
    pressed: [...document.querySelectorAll('[aria-pressed="true"]')].map((node) => node.textContent?.trim()),
    inputPath: document.querySelector('#input-path')?.value,
    inputDirection: document.querySelector('#input-path')?.getAttribute('dir'),
    inputLabel: document.querySelector('#input-path')?.labels?.[0]?.textContent?.trim(),
    nestedInteractiveLabels: [...document.querySelectorAll('label button, label a')].length,
  }))()`);
  assert(["zh-CN", "en"].includes(initial.lang), `unexpected document language ${initial.lang}`);
  assert(Boolean(initial.navigationLabel), "primary navigation has no localized accessible name");
  assert(Boolean(initial.activeNavigation), "active navigation item has no aria-current=page");
  assert(initial.pressed.length >= 2, "mode selections are not exposed with aria-pressed");
  assert(initial.inputDirection === "auto", "path input does not opt into bidi-safe direction detection");
  assert(Boolean(initial.inputLabel), "input path field has no explicit associated label");
  assert(initial.nestedInteractiveLabels === 0, "interactive controls remain nested inside a label");
  if (!/(?:مرحبا|Ù…Ø±Ø­Ø¨Ø§)/.test(initial.inputPath ?? "")) {
    await client.evaluate(`(() => {
      const input = document.querySelector('#input-path');
      const rtlPath = 'C:\\\\fixtures RTL 空格\\\\مرحبا שלום 名字.json';
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, rtlPath);
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
  }
  const rtlFixture = await client.evaluate(`(() => {
    const input = document.querySelector('#input-path');
    return {
      value: input?.value,
      direction: input ? getComputedStyle(input).direction : null,
      bidi: input ? getComputedStyle(input).unicodeBidi : null,
    };
  })()`);
  assert(/مرحبا/.test(rtlFixture.value ?? ""), `RTL fixture was not rendered in the real WebView: ${JSON.stringify(rtlFixture)}`);

  const tree = await client.command("Accessibility.getFullAXTree");
  const interactiveRoles = new Set(["button", "textbox", "combobox", "link", "checkbox"]);
  const unnamedInteractive = tree.nodes.filter((node) =>
    interactiveRoles.has(node.role?.value) &&
    axValue(node, "focusable") === true &&
    !String(node.name?.value ?? "").trim()
  );
  assert(unnamedInteractive.length === 0, `${unnamedInteractive.length} focusable controls have no accessible name`);
  assert(tree.nodes.some((node) => node.role?.value === "navigation" && node.name?.value === initial.navigationLabel), "localized navigation landmark missing from AX tree");
  assert(tree.nodes.some((node) => node.role?.value === "main"), "main landmark missing from AX tree");
  assert(tree.nodes.some((node) => node.role?.value === "link" && /跳到主要内容|Skip to main content/.test(node.name?.value ?? "")), "skip link missing from AX tree");
  const pressedAxNodes = tree.nodes.filter((node) =>
    node.role?.value === "button" &&
    (node.properties ?? []).some((property) =>
      property.name === "pressed" && [true, "true"].includes(property.value?.value)
    )
  );
  const pressedDomCount = await client.evaluate("document.querySelectorAll('[aria-pressed=\"true\"]').length");
  assert(pressedDomCount >= 2, "pressed state missing from the live accessibility DOM");
  assert(pressedAxNodes.length >= 2, "pressed state missing from the WebView accessibility tree");

  await client.evaluate("document.activeElement?.blur(); window.scrollTo(0, 0)");
  await key(client, "Tab", "Tab");
  const firstTab = await client.evaluate(`(() => ({
    className: document.activeElement?.className,
    text: document.activeElement?.textContent?.trim(),
  }))()`);
  assert(firstTab.className === "skip-link", `first Tab focused ${JSON.stringify(firstTab)}`);
  await key(client, "Enter", "Enter");
  await waitFor(client, "document.activeElement?.id === 'main-content'", "skip-link target focus");

  await client.command("Emulation.setDeviceMetricsOverride", {
    width: 590,
    height: 390,
    deviceScaleFactor: 2,
    mobile: false,
    screenWidth: 1180,
    screenHeight: 780,
  });
  await new Promise((resolve) => setTimeout(resolve, 150));
  const zoom = await client.evaluate(`(() => ({
    innerWidth,
    innerHeight,
    devicePixelRatio,
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
    pathClientWidth: document.querySelector('#input-path')?.clientWidth,
    pathScrollWidth: document.querySelector('#input-path')?.scrollWidth,
  }))()`);
  assert(zoom.innerWidth === 590 && zoom.devicePixelRatio === 2, `unexpected 200% equivalent viewport ${JSON.stringify(zoom)}`);
  assert(zoom.scrollWidth <= zoom.clientWidth, `document has horizontal overflow at 200%: ${JSON.stringify(zoom)}`);
  assert(zoom.pathClientWidth > 0, "path input collapsed at 200% equivalent scaling");

  const scaledShot = await client.command("Page.captureScreenshot", { format: "png", fromSurface: true });
  fs.writeFileSync(path.join(artifactDirectory, "desktop-200-percent.png"), Buffer.from(scaledShot.data, "base64"));

  await client.command("Emulation.setEmulatedMedia", {
    media: "screen",
    features: [
      { name: "prefers-reduced-motion", value: "reduce" },
      { name: "prefers-contrast", value: "more" },
      { name: "forced-colors", value: "active" },
    ],
  });
  await new Promise((resolve) => setTimeout(resolve, 100));
  const media = await client.evaluate(`(() => ({
    reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
    moreContrast: matchMedia('(prefers-contrast: more)').matches,
    forcedColors: matchMedia('(forced-colors: active)').matches,
    transitionDuration: getComputedStyle(document.querySelector('.drop-zone')).transitionDuration,
    rootBackground: getComputedStyle(document.documentElement).backgroundColor,
  }))()`);
  assert(media.reducedMotion && media.moreContrast && media.forcedColors, `media emulation not active: ${JSON.stringify(media)}`);
  assert(media.transitionDuration === "0s", `motion was not removed: ${media.transitionDuration}`);
  const contrastShot = await client.command("Page.captureScreenshot", { format: "png", fromSurface: true });
  fs.writeFileSync(path.join(artifactDirectory, "desktop-forced-colors.png"), Buffer.from(contrastShot.data, "base64"));

  await client.evaluate("document.querySelectorAll('header nav button').item(document.querySelectorAll('header nav button').length - 1).click()");
  await waitFor(client, "Boolean(document.querySelector('.settings-grid select'))", "settings view");
  const nextLanguage = initial.lang === "zh-CN" ? "en" : "zh-CN";
  await client.evaluate(`(() => {
    const select = document.querySelector('.settings-grid select');
    select.value = ${JSON.stringify(nextLanguage)};
    select.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await waitFor(client, `document.documentElement.lang === ${JSON.stringify(nextLanguage)}`, "language update");
  const localized = await client.evaluate(`(() => ({
    lang: document.documentElement.lang,
    navigationLabel: document.querySelector('header nav')?.getAttribute('aria-label'),
    skipText: document.querySelector('.skip-link')?.textContent?.trim(),
  }))()`);
  assert(localized.navigationLabel !== initial.navigationLabel, "navigation accessible name did not localize");
  assert(localized.skipText !== firstTab.text, "skip-link accessible text did not localize");

  const report = {
    schema_version: 1,
    target: { title: target.title, url: target.url },
    initial,
    rtl_fixture: rtlFixture,
    ax: {
      node_count: tree.nodes.length,
      unnamed_focusable_controls: unnamedInteractive.length,
      pressed_true_count: pressedAxNodes.length,
      pressed_dom_count: pressedDomCount,
      main_landmark: true,
      navigation_landmark: true,
      skip_link: true,
    },
    keyboard: { first_tab: firstTab, skip_target: "main-content" },
    equivalent_200_percent: zoom,
    media,
    localized,
  };
  fs.writeFileSync(path.join(artifactDirectory, "accessibility-audit.json"), `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} finally {
  client.close();
}
