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

let manifest = null;
let manifestPromise = null;
let configNames = [];
let isNavigating = false;
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
  td.colSpan = 5;
  td.className = "empty";
  td.textContent = message;
  row.append(td);
  tbody.append(row);
}

function configPageUrl(configName) {
  return `${script.src.replace(/app\.js$/, "")}CONFIG_/${encodeURIComponent(configName)}/`;
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

async function showConfigFromButton(button) {
  configViewer.hidden = false;
  configTitle.textContent = button.dataset.configTitle || "Config";
  configLink.href = button.dataset.configUrl;
  configBody.textContent = "Loading...";

  const response = await fetch(button.dataset.configUrl);
  if (!response.ok) {
    configBody.textContent = `Unable to load config: ${response.status}`;
    return;
  }
  configBody.textContent = await response.text();
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

input.addEventListener("input", () => {
  if (manifest) {
    updateAutocomplete();
    navigateIfExactConfig();
    return;
  }
  ensureManifest()
    .then(navigateIfExactConfig)
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
