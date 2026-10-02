#![allow(dead_code)]
use std::{collections::BTreeMap, fs, path::Path};
use serde::Deserialize;

#[derive(Deserialize)]
struct Kernel { version: String, release: usize, architecture: usize, source: Option<String> }
#[derive(Deserialize)]
struct Entry {
    #[serde(default)] built_in: Box<[u32]>,
    #[serde(default)] module: Box<[u32]>,
    #[serde(default)] other: Box<[(u32, String)]>,
    #[serde(default)] missing: Box<[u32]>,
}
#[derive(Deserialize)]
struct Index {
    distribution: String, package_name: String, generated_at: String,
    releases: Vec<String>, architectures: Vec<String>, kernels: Vec<Kernel>,
    entries: BTreeMap<String, Entry>,
}
fn discover(dir: &Path, paths: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() { discover(&entry.path(), paths); }
        else {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "index.json" || (name.starts_with("index_") && name.ends_with(".json")) { paths.push(entry.path()); }
        }
    }
}
fn main() {
    let start = std::time::Instant::now();
    let mut paths = Vec::new();
    discover(Path::new("data"), &mut paths);
    paths.sort();
    let mut indexes = Vec::new();
    let mut occurrences = 0usize;
    let mut entries = 0usize;
    let mut kernels = 0usize;
    for path in &paths {
        let json = fs::read(path).unwrap();
        let index: Index = serde_json::from_slice(&json).unwrap();
        entries += index.entries.len();
        kernels += index.kernels.len();
        occurrences += index.entries.values().map(|e| e.built_in.len() + e.module.len() + e.other.len() + e.missing.len()).sum::<usize>();
        let mut values = vec!["y".to_string(), "m".to_string()];
        let mut value_ids = BTreeMap::<String, u32>::new();
        let entries = index.entries.into_iter().map(|(name, entry)| {
            let mut occurrences = Vec::with_capacity(entry.built_in.len() + entry.module.len() + entry.other.len());
            occurrences.extend(entry.built_in.iter().map(|&kernel| (kernel, 0u32)));
            occurrences.extend(entry.module.iter().map(|&kernel| (kernel, 1u32)));
            for (kernel, value) in entry.other {
                let id = *value_ids.entry(value.clone()).or_insert_with(|| {
                    let id = values.len() as u32;
                    values.push(value);
                    id
                });
                occurrences.push((kernel, id));
            }
            occurrences.sort_unstable();
            (name, occurrences.into_boxed_slice())
        }).collect::<Vec<_>>().into_boxed_slice();
        indexes.push((index.distribution, index.package_name, index.generated_at, index.releases, index.architectures, index.kernels, entries, values));
    }
    let status = fs::read_to_string("/proc/self/status").unwrap();
    println!("{}", serde_json::json!({"indexes": indexes.len(), "entries": entries, "kernels": kernels, "occurrences": occurrences, "startup_seconds": start.elapsed().as_secs_f64(), "memory": status.lines().filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:")).collect::<Vec<_>>() }));
    std::hint::black_box(&indexes);
}
