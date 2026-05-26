const script = document.currentScript;
const manifestFile = script?.dataset.indexManifest || "indexes.json";

const form = document.querySelector("#search-form");
const input = document.querySelector("#config-input");
const options = document.querySelector("#config-options");
const title = document.querySelector("#result-title");
const count = document.querySelector("#result-count");
const tbody = document.querySelector("#results-body");
const configViewer = document.querySelector("#config-viewer");
const configTitle = document.querySelector("#config-title");
const configLink = document.querySelector("#config-link");
const configBody = document.querySelector("#config-body");
const resultsColumnCount = 6;

let manifest = null;
let manifestPromise = null;
let configNames = [];
let isNavigating = false;
let previousInputValue = "";
let activeConfigRequestId = 0;
const maxSuggestions = 200;

function bareConfigName(value) {
  const normalized = value.trim().toUpperCase();
  return normalized.startsWith("CONFIG_")
    ? normalized.slice("CONFIG_".length)
    : normalized;
}

async function fetchJson(path) {
  const response = await fetch(path);
  if (!response.ok) {
    throw new Error(`Unable to load ${path}: ${response.status}`);
  }
  return response.json();
}

async function ensureManifest() {
  if (manifest) return manifest;
  manifestPromise ||= fetchJson(manifestFile);
  try {
    manifest = await manifestPromise;
    configNames = (manifest.configs || []).slice().sort((left, right) =>
      left.localeCompare(right),
    );
    updateAutocomplete();
    return manifest;
  } catch (error) {
    manifestPromise = null;
    throw error;
  }
}

function updateAutocomplete() {
  const raw = input.value.trim().toUpperCase();
  const usesPrefix = raw.startsWith("CONFIG_");
  const query = usesPrefix ? raw.slice("CONFIG_".length) : raw;
  const matches = configNames
    .filter((name) => name.startsWith(query))
    .slice(0, maxSuggestions);

  options.replaceChildren(
    ...matches.map((name) => {
      const option = document.createElement("option");
      option.value = usesPrefix ? `CONFIG_${name}` : name;
      return option;
    }),
  );
}

function renderSearchError(message) {
  title.textContent = "Search failed";
  count.textContent = "";
  configViewer.hidden = true;
  tbody.replaceChildren();
  const row = document.createElement("tr");
  const td = document.createElement("td");
  td.colSpan = resultsColumnCount;
  td.className = "empty";
  td.textContent = message;
  row.append(td);
  tbody.append(row);
}

function configPageUrl(configName) {
  return `${script.src.replace(/app\.js$/, "")}CONFIG_/${encodeURIComponent(configName)}/`;
}

function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function renderConfigLine(line) {
  let match = line.match(/^(CONFIG_[A-Z0-9_]+)(=.*)$/);
  if (match) {
    const [, configName, suffix] = match;
    return `<a class="config-entry-link" href="${configPageUrl(
      bareConfigName(configName),
    )}">${escapeHtml(configName)}</a>${escapeHtml(suffix)}`;
  }

  match = line.match(/^(# )(CONFIG_[A-Z0-9_]+)( is not set)$/);
  if (match) {
    const [, prefix, configName, suffix] = match;
    return `${escapeHtml(prefix)}<a class="config-entry-link" href="${configPageUrl(
      bareConfigName(configName),
    )}">${escapeHtml(configName)}</a>${escapeHtml(suffix)}`;
  }

  return escapeHtml(line);
}

function renderConfigText(configText) {
  return configText
    .replaceAll("\r\n", "\n")
    .replaceAll("\r", "\n")
    .split("\n")
    .map(renderConfigLine)
    .join("\n");
}

function scrollConfigViewerIntoView() {
  configViewer.scrollIntoView({
    block: "start",
  });
}

function navigateToConfig(configName) {
  isNavigating = true;
  window.location.href = configPageUrl(configName);
}

function navigateIfExactConfig() {
  if (isNavigating) return;
  const configName = bareConfigName(input.value);
  if (!configName) return;
  if (configNames.includes(configName)) {
    navigateToConfig(configName);
  }
}

function shouldNavigateFromInputEvent(event) {
  const currentValue = input.value;
  const previousValue = previousInputValue;
  previousInputValue = currentValue;

  if (!currentValue || currentValue === previousValue) {
    return false;
  }

  const inputType = event.inputType || "";
  if (inputType === "insertReplacementText") {
    return true;
  }

  if (
    inputType === "insertText" ||
    inputType.startsWith("delete") ||
    inputType.startsWith("history")
  ) {
    return false;
  }

  return Math.abs(currentValue.length - previousValue.length) > 1;
}

async function showConfigFromButton(button) {
  const requestId = ++activeConfigRequestId;
  configViewer.hidden = false;
  configTitle.textContent = button.dataset.configTitle || "Config";
  configLink.href = button.dataset.configUrl;
  configBody.textContent = "Loading...";
  scrollConfigViewerIntoView();

  try {
    const response = await fetch(button.dataset.configUrl);
    if (requestId !== activeConfigRequestId) {
      return;
    }
    if (!response.ok) {
      configBody.textContent = `Unable to load config: ${response.status}`;
      return;
    }
    configBody.innerHTML = renderConfigText(await response.text());
  } catch (error) {
    if (requestId !== activeConfigRequestId) {
      return;
    }
    configBody.textContent = `Unable to load config: ${error.message}`;
  }
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const configName = bareConfigName(input.value);

  try {
    const siteManifest = await ensureManifest();
    if (!siteManifest.configs.includes(configName)) {
      renderSearchError("No generated page exists for this config entry.");
      return;
    }
    navigateToConfig(configName);
  } catch (error) {
    renderSearchError(error.message);
  }
});

document.addEventListener("click", (event) => {
  const button = event.target.closest(".arch-button[data-config-url]");
  if (!button) return;
  showConfigFromButton(button);
});

input.addEventListener("focus", () => {
  ensureManifest().catch(() => {
    options.replaceChildren();
  });
});

input.addEventListener("input", (event) => {
  const shouldNavigate = shouldNavigateFromInputEvent(event);
  if (manifest) {
    updateAutocomplete();
    if (shouldNavigate) {
      navigateIfExactConfig();
    }
    return;
  }
  ensureManifest()
    .then(() => {
      if (shouldNavigate) {
        navigateIfExactConfig();
      }
    })
    .catch(() => {
      options.replaceChildren();
    });
});

input.addEventListener("change", () => {
  if (manifest) {
    navigateIfExactConfig();
    return;
  }
  ensureManifest()
    .then(navigateIfExactConfig)
    .catch(() => {
      options.replaceChildren();
    });
});
