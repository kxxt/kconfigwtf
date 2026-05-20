const script = document.currentScript;
const manifestFile = script?.dataset.indexManifest || "indexes.json";

const form = document.querySelector("#search-form");
const input = document.querySelector("#config-input");
const title = document.querySelector("#result-title");
const count = document.querySelector("#result-count");
const tbody = document.querySelector("#results-body");
const configViewer = document.querySelector("#config-viewer");
const configTitle = document.querySelector("#config-title");
const configLink = document.querySelector("#config-link");
const configBody = document.querySelector("#config-body");

let packageIndexes = null;

function normalizeConfigName(value) {
  const normalized = value.trim().toUpperCase();
  return normalized.startsWith("CONFIG_") ? normalized : `CONFIG_${normalized}`;
}

function displayValue(value) {
  if (value === "built_in") return "y";
  if (value === "module") return "m";
  if (value === "-") return "-";
  if (value && typeof value === "object" && "other" in value) return value.other;
  return String(value ?? "");
}

function joinRelative(base, path) {
  const prefix = base.slice(0, base.lastIndexOf("/") + 1);
  return `${prefix}${path}`;
}

function cell(text) {
  const td = document.createElement("td");
  td.textContent = text;
  return td;
}

function renderEmpty(message) {
  tbody.replaceChildren();
  const row = document.createElement("tr");
  const td = document.createElement("td");
  td.colSpan = 7;
  td.className = "empty";
  td.textContent = message;
  row.append(td);
  tbody.append(row);
}

async function showConfig(record) {
  configViewer.hidden = false;
  configTitle.textContent = `${record.packageName} ${record.version} ${record.architecture}`;
  configLink.href = record.configUrl;
  configBody.textContent = "Loading...";

  const response = await fetch(record.configUrl);
  if (!response.ok) {
    configBody.textContent = `Unable to load config: ${response.status}`;
    return;
  }
  configBody.textContent = await response.text();
}

function renderResults(configName, records) {
  title.textContent = configName;
  count.textContent = `${records.length} match${records.length === 1 ? "" : "es"}`;
  tbody.replaceChildren();

  for (const record of records) {
    const row = document.createElement("tr");
    row.append(
      cell(record.distribution),
      cell(record.packageName),
      cell(record.version),
      cell(record.architecture),
      cell(displayValue(record.value)),
    );

    const config = document.createElement("td");
    const configButton = document.createElement("button");
    configButton.type = "button";
    configButton.className = "link-button";
    configButton.textContent = "view";
    configButton.addEventListener("click", () => showConfig(record));
    config.append(configButton);
    row.append(config);

    const source = document.createElement("td");
    if (record.source) {
      const link = document.createElement("a");
      link.href = record.source;
      link.textContent = "package";
      source.append(link);
    } else {
      source.textContent = "";
    }
    row.append(source);
    tbody.append(row);
  }
}

async function fetchJson(path) {
  const response = await fetch(path);
  if (!response.ok) {
    throw new Error(`Unable to load ${path}: ${response.status}`);
  }
  return response.json();
}

async function loadPackageIndexes() {
  const manifest = await fetchJson(manifestFile);
  return Promise.all(
    manifest.indexes.map(async (indexPath) => ({
      indexPath,
      data: await fetchJson(indexPath),
    })),
  );
}

function findRecords(configName, packages) {
  const records = [];
  for (const packageIndex of packages) {
    const occurrences = packageIndex.data.entries[configName] || [];
    for (const occurrence of occurrences) {
      const kernel = packageIndex.data.kernels[occurrence.kernel];
      if (!kernel) continue;
      records.push({
        distribution: packageIndex.data.distribution,
        packageName: packageIndex.data.package_name,
        version: kernel.version,
        architecture: kernel.architecture,
        value: occurrence.value,
        source: kernel.source,
        configUrl: joinRelative(packageIndex.indexPath, kernel.config_path),
      });
    }
  }
  records.sort((left, right) =>
    [
      left.distribution,
      left.packageName,
      left.version,
      left.architecture,
    ].join("\0").localeCompare([
      right.distribution,
      right.packageName,
      right.version,
      right.architecture,
    ].join("\0")),
  );
  return records;
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const configName = normalizeConfigName(input.value);

  try {
    packageIndexes ||= await loadPackageIndexes();
    const records = findRecords(configName, packageIndexes);
    if (records.length === 0) {
      title.textContent = configName;
      count.textContent = "0 matches";
      configViewer.hidden = true;
      renderEmpty("No indexed kernel config contains this entry.");
      return;
    }
    renderResults(configName, records);
  } catch (error) {
    title.textContent = "Index load failed";
    count.textContent = "";
    configViewer.hidden = true;
    renderEmpty(error.message);
  }
});
