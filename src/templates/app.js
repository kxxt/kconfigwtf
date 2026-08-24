const script = document.currentScript;
const apiBase = (script?.dataset.apiBase || "").replace(/\/$/, "");
const manifestFile = apiBase
  ? `${apiBase}/configs`
  : script?.dataset.indexManifest || "indexes.json";

const form = document.querySelector("#search-form");
const input = document.querySelector("#config-input");
const options = document.querySelector("#config-options");
const title = document.querySelector("#result-title");
const count = document.querySelector("#result-count");
const tbody = document.querySelector("#results-body");
const resultHeading = document.querySelector(".result-heading");
const configViewer = document.querySelector("#config-viewer");
const configTitle = document.querySelector("#config-title");
const configLink = document.querySelector("#config-link");
const configBody = document.querySelector("#config-body");
const resultsColumnCount = 6;
const siteTitle = document.querySelector("h1")?.textContent || "kconfigwtf";

let manifest = null;
let manifestPromise = null;
let configNames = [];
let isNavigating = false;
let previousInputValue = "";
let activeConfigRequestId = 0;
let activeSearchRequestId = 0;
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
    let detail = "";
    try {
      detail = (await response.json()).error || "";
    } catch (_) {
      // The status below is enough when an intermediary returns a non-JSON error.
    }
    throw new Error(detail || `Unable to load ${path}: ${response.status}`);
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
  resultHeading.querySelector(".result-links")?.remove();
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
  configViewer.scrollIntoView({ block: "start" });
}

function groupBy(values, key) {
  const groups = new Map();
  for (const value of values) {
    const group = groups.get(value[key]) || [];
    group.push(value);
    groups.set(value[key], group);
  }
  return groups;
}

function groupedRowCount(records, keys) {
  if (!keys.length) return 1;
  let total = 0;
  for (const values of groupBy(records, keys[0]).values()) {
    total += groupedRowCount(values, keys.slice(1));
  }
  return total;
}

function groupCell(text, rowSpan, className) {
  const cell = document.createElement("td");
  cell.rowSpan = rowSpan;
  cell.className = `group-cell ${className}`;
  if (className.includes("group-cell-distribution") || className.includes("group-cell-release")) {
    const label = document.createElement("span");
    label.className = "sticky-group-label";
    label.textContent = text;
    cell.append(label);
  } else {
    cell.textContent = text;
  }
  return cell;
}

function archButton(record) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "arch-button";
  button.dataset.configUrl = record.config_url;
  button.dataset.configTitle = `${record.package_name} ${record.version} ${record.architecture}`;
  button.textContent = record.architecture;
  return button;
}

function renderVersionTags(records) {
  const list = document.createElement("div");
  list.className = "tag-list";

  for (const [version, versionRecords] of groupBy(records, "version")) {
    const architectures = new Map();
    for (const record of versionRecords) {
      if (!architectures.has(record.architecture)) {
        architectures.set(record.architecture, record);
      }
    }

    const tag = document.createElement("div");
    tag.className = "kernel-tag";
    tag.title = `${version}: ${Array.from(architectures.keys()).join(", ")}`;
    const versionLabel = document.createElement("span");
    versionLabel.className = "tag-version";
    versionLabel.textContent = version;
    tag.append(versionLabel);

    const buttons = document.createElement("span");
    buttons.className = "tag-architectures";
    buttons.append(...Array.from(architectures.values(), archButton));
    if (architectures.size > 4) {
      const details = document.createElement("details");
      details.className = "arch-details";
      const summary = document.createElement("summary");
      summary.textContent = `${architectures.size} archs`;
      details.append(summary, buttons);
      tag.append(details);
    } else {
      tag.append(buttons);
    }
    list.append(tag);
  }
  return list;
}

function renderSources(records) {
  const cell = document.createElement("td");
  const sources = Array.from(new Set(records.map((record) => record.source).filter(Boolean)));
  if (sources.length === 1) {
    const link = document.createElement("a");
    link.href = sources[0];
    link.textContent = "package";
    cell.append(link);
  } else if (sources.length > 1) {
    cell.textContent = `${sources.length} packages`;
  }
  return cell;
}

function renderResultRows(records) {
  const rows = [];
  for (const [distribution, distributionRecords] of groupBy(records, "distribution")) {
    let wroteDistribution = false;
    for (const [release, releaseRecords] of groupBy(distributionRecords, "release")) {
      let wroteRelease = false;
      for (const [packageName, packageRecords] of groupBy(releaseRecords, "package_name")) {
        let wrotePackage = false;
        for (const [value, valueRecords] of groupBy(packageRecords, "value")) {
          const row = document.createElement("tr");
          if (!wroteDistribution) {
            row.append(groupCell(
              distribution,
              groupedRowCount(distributionRecords, ["release", "package_name", "value"]),
              "group-cell-distribution",
            ));
            wroteDistribution = true;
          }
          if (!wroteRelease) {
            row.append(groupCell(
              release,
              groupedRowCount(releaseRecords, ["package_name", "value"]),
              "group-cell-release",
            ));
            wroteRelease = true;
          }
          if (!wrotePackage) {
            row.append(groupCell(
              packageName,
              groupedRowCount(packageRecords, ["value"]),
              "package-cell",
            ));
            wrotePackage = true;
          }
          const valueCell = document.createElement("td");
          valueCell.textContent = value;
          const versionsCell = document.createElement("td");
          versionsCell.append(renderVersionTags(valueRecords));
          row.append(valueCell, versionsCell, renderSources(valueRecords));
          rows.push(row);
        }
      }
    }
  }
  tbody.replaceChildren(...rows);
}

function renderReferenceLinks(configName) {
  resultHeading.querySelector(".result-links")?.remove();
  const bareName = bareConfigName(configName);
  const links = document.createElement("div");
  links.className = "result-links";
  for (const [label, href] of [
    ["lkddb", `https://cateee.net/lkddb/web-lkddb/${encodeURIComponent(bareName)}.html`],
    ["kernelconfig.io", `https://www.kernelconfig.io/CONFIG_${encodeURIComponent(bareName)}`],
  ]) {
    const link = document.createElement("a");
    link.href = href;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = label;
    links.append(link);
  }
  resultHeading.append(links);
}

async function loadConfigResults(configName) {
  const requestId = ++activeSearchRequestId;
  title.textContent = `Loading CONFIG_${configName}...`;
  count.textContent = "";
  configViewer.hidden = true;
  try {
    const result = await fetchJson(`${apiBase}/configs/${encodeURIComponent(configName)}`);
    if (requestId !== activeSearchRequestId) return;
    title.textContent = result.config;
    count.textContent = `${result.records.length} match${result.records.length === 1 ? "" : "es"}`;
    renderReferenceLinks(result.config);
    renderResultRows(result.records);
    document.title = `${result.config} - ${siteTitle}`;
  } catch (error) {
    if (requestId === activeSearchRequestId) renderSearchError(error.message);
  }
}

function navigateToConfig(configName) {
  isNavigating = true;
  if (!apiBase) {
    window.location.href = configPageUrl(configName);
    return;
  }
  const url = configPageUrl(configName);
  if (window.location.pathname !== new URL(url).pathname) {
    window.history.pushState({}, "", url);
  }
  input.value = configName;
  previousInputValue = configName;
  isNavigating = false;
  loadConfigResults(configName);
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

  if (!currentValue || currentValue === previousValue) return false;

  const inputType = event.inputType || "";
  if (inputType === "insertReplacementText") return true;

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
    if (requestId !== activeConfigRequestId) return;
    if (!response.ok) {
      configBody.textContent = `Unable to load config: ${response.status}`;
      return;
    }
    configBody.innerHTML = renderConfigText(await response.text());
  } catch (error) {
    if (requestId !== activeConfigRequestId) return;
    configBody.textContent = `Unable to load config: ${error.message}`;
  }
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const configName = bareConfigName(input.value);

  try {
    const siteManifest = await ensureManifest();
    if (!siteManifest.configs.includes(configName)) {
      renderSearchError("No indexed config entry exists with that name.");
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
  ensureManifest().catch(() => options.replaceChildren());
});

input.addEventListener("input", (event) => {
  const shouldNavigate = shouldNavigateFromInputEvent(event);
  if (manifest) {
    updateAutocomplete();
    if (shouldNavigate) navigateIfExactConfig();
    return;
  }
  ensureManifest()
    .then(() => {
      if (shouldNavigate) navigateIfExactConfig();
    })
    .catch(() => options.replaceChildren());
});

input.addEventListener("change", () => {
  if (manifest) {
    navigateIfExactConfig();
    return;
  }
  ensureManifest()
    .then(navigateIfExactConfig)
    .catch(() => options.replaceChildren());
});

if (apiBase) {
  window.addEventListener("popstate", () => {
    const match = window.location.pathname.match(/\/CONFIG_\/([^/]+)\/?$/);
    if (match) {
      const configName = bareConfigName(decodeURIComponent(match[1]));
      input.value = configName;
      loadConfigResults(configName);
    } else {
      window.location.reload();
    }
  });

  const initialMatch = window.location.pathname.match(/\/CONFIG_\/([^/]+)\/?$/);
  if (initialMatch) {
    const configName = bareConfigName(decodeURIComponent(initialMatch[1]));
    input.value = configName;
    previousInputValue = configName;
    loadConfigResults(configName);
  }
  ensureManifest().catch(() => options.replaceChildren());
}
