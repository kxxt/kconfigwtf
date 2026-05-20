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

let packageIndexes = null;
let packageIndexesPromise = null;
let manifest = null;
let manifestPromise = null;
let configNames = [];
const maxSuggestions = 200;
const maxArchitecturesPerTag = 4;

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
  td.colSpan = 5;
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

async function showConfigGroup(records) {
  const sorted = records
    .slice()
    .sort((left, right) =>
      [left.version, left.architecture].join("\0").localeCompare(
        [right.version, right.architecture].join("\0"),
      ),
    );
  await showConfig(sorted[0]);
  if (sorted.length > 1) {
    configTitle.textContent = `${sorted[0].packageName} ${sorted[0].version} ${sorted[0].architecture} (first of ${sorted.length})`;
  }
}

function groupRecords(records) {
  const distributionMap = new Map();
  for (const record of records) {
    let distribution = distributionMap.get(record.distribution);
    if (!distribution) {
      distribution = {
        distribution: record.distribution,
        packageMap: new Map(),
        packages: [],
      };
      distributionMap.set(record.distribution, distribution);
    }

    let packageGroup = distribution.packageMap.get(record.packageName);
    if (!packageGroup) {
      packageGroup = {
        packageName: record.packageName,
        valueMap: new Map(),
        valueGroups: [],
      };
      distribution.packageMap.set(record.packageName, packageGroup);
      distribution.packages.push(packageGroup);
    }

    const value = displayValue(record.value);
    let valueGroup = packageGroup.valueMap.get(value);
    if (!valueGroup) {
      valueGroup = {
        value,
        records: [],
      };
      packageGroup.valueMap.set(value, valueGroup);
      packageGroup.valueGroups.push(valueGroup);
    }
    valueGroup.records.push(record);
  }

  const distributions = Array.from(distributionMap.values()).sort((left, right) =>
    left.distribution.localeCompare(right.distribution),
  );

  for (const distribution of distributions) {
    distribution.packages.sort((left, right) =>
      left.packageName.localeCompare(right.packageName),
    );
    distribution.rowSpan = 0;

    for (const packageGroup of distribution.packages) {
      packageGroup.valueGroups.sort((left, right) =>
        left.value.localeCompare(right.value),
      );
      packageGroup.rowSpan = packageGroup.valueGroups.length;
      distribution.rowSpan += packageGroup.rowSpan;
    }
  }

  return distributions;
}

function versionTags(records) {
  const versionMap = new Map();
  for (const record of records) {
    let version = versionMap.get(record.version);
    if (!version) {
      version = {
        version: record.version,
        architectures: new Map(),
      };
      versionMap.set(record.version, version);
    }
    if (!version.architectures.has(record.architecture)) {
      version.architectures.set(record.architecture, []);
    }
    version.architectures.get(record.architecture).push(record);
  }

  return Array.from(versionMap.values())
    .sort((left, right) => left.version.localeCompare(right.version))
    .map((version) => {
      const architectures = Array.from(version.architectures.keys()).sort();
      const records = architectures.flatMap((architecture) =>
        version.architectures.get(architecture),
      );
      return {
        version: version.version,
        architectures:
          architectures.length > maxArchitecturesPerTag
            ? `${architectures.length} archs`
            : architectures.join(", "),
        title: `${version.version}: ${architectures.join(", ")}`,
        records,
      };
    });
}

function tagsCell(records) {
  const td = document.createElement("td");
  const tags = document.createElement("div");
  tags.className = "tag-list";

  for (const tag of versionTags(records)) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "kernel-tag";
    button.title = tag.title;
    const version = document.createElement("span");
    version.className = "tag-version";
    version.textContent = tag.version;
    const architectures = document.createElement("span");
    architectures.className = "tag-architectures";
    architectures.textContent = tag.architectures;
    button.append(version, architectures);
    button.addEventListener("click", () => showConfigGroup(tag.records));
    tags.append(button);
  }

  td.append(tags);
  return td;
}

function sourcesCell(records) {
  const td = document.createElement("td");
  const sources = Array.from(
    new Map(
      records
        .filter((record) => record.source)
        .map((record) => [record.source, record]),
    ).values(),
  );

  if (sources.length === 0) {
    td.textContent = "";
  } else if (sources.length === 1) {
    const link = document.createElement("a");
    link.href = sources[0].source;
    link.textContent = "package";
    td.append(link);
  } else {
    td.textContent = `${sources.length} packages`;
  }

  return td;
}

function renderResults(configName, records) {
  title.textContent = configName;
  count.textContent = `${records.length} match${records.length === 1 ? "" : "es"}`;
  tbody.replaceChildren();

  for (const distribution of groupRecords(records)) {
    let wroteDistribution = false;
    for (const packageGroup of distribution.packages) {
      let wrotePackage = false;
      for (const valueGroup of packageGroup.valueGroups) {
        const row = document.createElement("tr");

        if (!wroteDistribution) {
          const distributionCell = cell(distribution.distribution);
          distributionCell.rowSpan = distribution.rowSpan;
          distributionCell.className = "group-cell";
          row.append(distributionCell);
          wroteDistribution = true;
        }

        if (!wrotePackage) {
          const packageCell = cell(packageGroup.packageName);
          packageCell.rowSpan = packageGroup.rowSpan;
          packageCell.className = "group-cell package-cell";
          row.append(packageCell);
          wrotePackage = true;
        }

        row.append(
          cell(valueGroup.value),
          tagsCell(valueGroup.records),
          sourcesCell(valueGroup.records),
        );
        tbody.append(row);
      }
    }
  }
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

async function loadPackageIndexes() {
  const siteManifest = await ensureManifest();
  return Promise.all(
    siteManifest.indexes.map(async (indexPath) => ({
      indexPath,
      data: await fetchJson(indexPath),
    })),
  );
}

async function ensurePackageIndexes() {
  if (packageIndexes) return packageIndexes;
  packageIndexesPromise ||= loadPackageIndexes();
  try {
    packageIndexes = await packageIndexesPromise;
    return packageIndexes;
  } catch (error) {
    packageIndexesPromise = null;
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
    await ensurePackageIndexes();
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

input.addEventListener("focus", () => {
  ensureManifest().catch(() => {
    options.replaceChildren();
  });
});

input.addEventListener("input", () => {
  if (packageIndexes) {
    updateAutocomplete();
    return;
  }
  ensureManifest().catch(() => {
    options.replaceChildren();
  });
});
