#!/usr/bin/env node
// Drives one real FormatWright Release UI conversion over CDP.
// The caller launches one isolated application process per invocation
// (scripts/test_desktop_release_conversion.ps1), so this script performs a
// single conversion from a fresh React state and never navigates between
// rounds inside the same page.
import fs from "node:fs";
import path from "node:path";

const [portText, artifactRootValue, inputValue, targetFormat, outputPathValue] = process.argv.slice(2);
const port = Number(portText);
const artifactRoot = path.resolve(artifactRootValue);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("invalid DevTools port");
if (!inputValue || !targetFormat || !outputPathValue) {
  throw new Error("usage: cdp_desktop_conversion_e2e.mjs PORT ARTIFACT_ROOT INPUT TARGET_FORMAT OUTPUT");
}
const outputPath = path.resolve(outputPathValue);
fs.mkdirSync(artifactRoot, { recursive: true });

const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function getTarget() {
  const deadline = Date.now() + 30_000;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
        signal: AbortSignal.timeout(1_000),
      });
      const targets = await response.json();
      const target = targets.find((candidate) =>
        candidate.type === "page" && candidate.url === "http://tauri.localhost/" &&
        candidate.webSocketDebuggerUrl
      );
      if (target) return target;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  throw new Error(`WebView DevTools target did not appear: ${lastError ?? "no target"}`);
}

class CdpClient {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.sequence = 0;
    this.pending = new Map();
  }

  async open() {
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      const request = this.pending.get(message.id);
      if (!request) return;
      this.pending.delete(message.id);
      if (message.error) request.reject(new Error(`${request.method}: ${message.error.message}`));
      else request.resolve(message.result);
    });
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("DevTools WebSocket open timed out")), 10_000);
      this.socket.addEventListener("open", () => { clearTimeout(timer); resolve(); }, { once: true });
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
        reject(new Error(`${method} timed out`));
      }, 20_000);
      this.pending.set(id, {
        method,
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const response = await this.command("Runtime.evaluate", { expression, returnByValue: true });
    if (response.exceptionDetails) {
      throw new Error(response.exceptionDetails.exception?.description ?? "Runtime.evaluate failed");
    }
    return response.result.value;
  }

  close() { this.socket.close(); }
}

async function waitFor(client, expression, description, timeout = 60_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await client.evaluate(expression)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function capture(client, filename) {
  const result = await client.command("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  fs.writeFileSync(path.join(artifactRoot, filename), Buffer.from(result.data, "base64"));
}

const diagnosticExpression = `(() => {
  const select = document.querySelector('.form-grid select');
  const buttons = [...document.querySelectorAll('.workspace-main > .action-row button')];
  return {
    input: document.querySelector('#input-path')?.value,
    output: document.querySelector('#output-path')?.value,
    selectDisabled: select?.disabled,
    selected: select?.value,
    options: [...(select?.options ?? [])].map((option) => ({ value: option.value, disabled: option.disabled })),
    buttons: buttons.map((button) => ({ text: button.textContent?.trim(), className: button.className, disabled: button.disabled })),
    planHeading: document.querySelector('.plan-card h2')?.textContent?.trim() ?? null,
    capabilityNotice: document.querySelector('.capability-notice')?.textContent?.trim() ?? null,
    progress: document.querySelector('.execution-progress')?.textContent?.trim() ?? null,
    error: document.querySelector('.error-banner')?.textContent?.trim() ?? null,
  };
})()`;

async function failWithDiagnostics(client, step, error) {
  const diagnostic = await client.evaluate(diagnosticExpression);
  await capture(client, `${targetFormat}-${step}-timeout.png`);
  throw new Error(`${error.message}; step=${step}; diagnostic=${JSON.stringify(diagnostic)}`);
}

const target = await getTarget();
const client = new CdpClient(target.webSocketDebuggerUrl);
await client.open();

async function setReactInput(client, selector, value) {
  await client.evaluate(`(() => {
    const input = document.querySelector(${JSON.stringify(selector)});
    input.focus();
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, ${JSON.stringify(value)});
    input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: null }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
}

try {
  await client.command("Runtime.enable");
  await client.command("Page.enable");
  await waitFor(client, "document.readyState === 'complete' && Boolean(document.querySelector('#input-path'))", "FormatWright UI");

  await setReactInput(client, "#input-path", inputValue);
  try {
    await waitFor(
      client,
      `(() => { const select = document.querySelector('.form-grid select'); const option = [...(select?.options ?? [])].find((candidate) => candidate.value === "${targetFormat}"); return select && !select.disabled && option && !option.disabled; })()`,
      `${targetFormat} capability`,
    );
  } catch (error) {
    await failWithDiagnostics(client, "capability", error);
  }
  await client.evaluate(`(() => {
    const select = document.querySelector('.form-grid select');
    const selectSetter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set;
    selectSetter.call(select, ${JSON.stringify(targetFormat)});
    select.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await waitFor(client, `document.querySelector('.form-grid select')?.value === '${targetFormat}'`, `${targetFormat} selection`);
  await setReactInput(client, "#output-path", outputPath);
  try {
    await waitFor(
      client,
      "(() => { const button = document.querySelector('.workspace-main > .action-row button'); return button && !button.disabled; })()",
      "enabled Plan preview button",
    );
  } catch (error) {
    await failWithDiagnostics(client, "plan-button", error);
  }
  await client.evaluate("document.querySelector('.workspace-main > .action-row button').click()");
  // The UI normalizes the submitted target id when rendering the plan heading
  // (jpg -> JPEG), so accept the normalized spelling rather than the raw id.
  const normalizedPlanTarget = targetFormat === "jpg" ? "jpeg" : targetFormat;
  try {
    await waitFor(
      client,
      `Boolean(document.querySelector('.plan-card h2')?.textContent?.includes('PDF → ${normalizedPlanTarget.toUpperCase()}') || document.querySelector('.error-banner'))`,
      `${targetFormat} Plan preview`,
    );
  } catch (error) {
    await failWithDiagnostics(client, "plan", error);
  }
  const previewError = await client.evaluate("document.querySelector('.error-banner')?.textContent?.trim() ?? null");
  if (previewError) throw new Error(`${targetFormat} preview failed: ${previewError}`);
  try {
    await waitFor(
      client,
      "(() => { const button = document.querySelector('.workspace-main > .action-row button.primary'); return button && !button.disabled; })()",
      "enabled conversion button",
    );
  } catch (error) {
    await failWithDiagnostics(client, "run-button", error);
  }
  await client.evaluate("document.querySelector('.workspace-main > .action-row button.primary').click()");
  try {
    await waitFor(
      client,
      "Boolean(document.querySelector('.report-status') || document.querySelector('.error-banner'))",
      `${targetFormat} validation report`,
      90_000,
    );
  } catch (error) {
    await failWithDiagnostics(client, "report", error);
  }
  const result = await client.evaluate(`(() => ({
    target: ${JSON.stringify(targetFormat)},
    error: document.querySelector('.error-banner')?.textContent?.trim() ?? null,
    status: document.querySelector('.report-status')?.textContent?.trim() ?? null,
    output: document.querySelector('.report-summary bdi')?.textContent?.trim() ?? null,
    required: document.querySelector('.report-summary strong')?.textContent?.trim() ?? null,
    failedChecks: [...document.querySelectorAll('.check-list article')]
      .filter((entry) => entry.querySelector('em')?.textContent?.trim() !== 'pass')
      .map((entry) => entry.textContent?.trim()),
  }))()`);
  if (result.error) throw new Error(`${targetFormat} conversion failed: ${result.error}`);
  if (result.status !== "pass" || result.failedChecks.length !== 0 || result.output !== outputPath) {
    throw new Error(`${targetFormat} report did not pass: ${JSON.stringify(result)}`);
  }
  await capture(client, `${targetFormat}-report.png`);
  const summary = { schema_version: 1, input: inputValue, conversions: [result] };
  fs.writeFileSync(path.join(artifactRoot, `desktop-conversion-e2e-${targetFormat}.json`), `${JSON.stringify(summary, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
} finally {
  client.close();
}
