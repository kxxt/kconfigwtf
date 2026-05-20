const script = document.currentScript;
const indexFile = script?.dataset.indexFile || "index.json";

const form = document.querySelector("#search-form");
const input = document.querySelector("#config-input");
const title = document.querySelector("#result-title");
const count = document.querySelector("#result-count");
const tbody = document.querySelector("#results-body");

let configIndex = null;

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

function cell(text) {
  const td = document.createElement("td");
  td.textContent = text;
  return td;
}

function renderEmpty(message) {
  tbody.replaceChildren();
  const row = document.createElement("tr");
  const td = document.createElement("td");
  td.colSpan = 6;
  td.className = "empty";
  td.textContent = message;
  row.append(td);
  tbody.append(row);
}

function renderResults(configName, records) {
  title.textContent = configName;
  count.textContent = `${records.length} match${records.length === 1 ? "" : "es"}`;
  tbody.replaceChildren();

  for (const record of records) {
    const row = document.createElement("tr");
    row.append(
      cell(record.distribution),
      cell(record.package_name),
      cell(record.package_version),
      cell(record.architecture),
      cell(displayValue(record.value)),
    );

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

async function loadIndex() {
  const response = await fetch(indexFile);
  if (!response.ok) {
    throw new Error(`Unable to load ${indexFile}: ${response.status}`);
  }
  return response.json();
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const configName = normalizeConfigName(input.value);

  try {
    configIndex ||= await loadIndex();
    const records = configIndex.entries[configName] || [];
    if (records.length === 0) {
      title.textContent = configName;
      count.textContent = "0 matches";
      renderEmpty("No indexed distribution enables this config entry.");
      return;
    }
    renderResults(configName, records);
  } catch (error) {
    title.textContent = "Index load failed";
    count.textContent = "";
    renderEmpty(error.message);
  }
});
