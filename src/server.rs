use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use chrono::Utc;
use http::header::{ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use http::{Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::index::{ConfigValue, PackageIndex, normalize_config_name, read_package_index};
use crate::site::{SiteManifest, find_package_indexes, render_server_page};

const API_BASE: &str = "/api/v1";
const HTML_CACHE: &str = "public, max-age=60, s-maxage=300, stale-while-revalidate=3600";
const ASSET_CACHE: &str = "public, max-age=300, s-maxage=3600, stale-while-revalidate=86400";
const DATA_CACHE: &str = "public, max-age=60, s-maxage=300, stale-while-revalidate=86400";
const RAW_CACHE: &str = "public, max-age=300, s-maxage=86400, stale-while-revalidate=604800";
const NEGATIVE_CACHE: &str = "public, max-age=30, s-maxage=300";

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub title: String,
}

pub async fn serve(config: ServerConfig) -> Result<()> {
    let app = Arc::new(App::load(&config.data_dir, &config.title)?);
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding HTTP server to {}", config.listen))?;
    eprintln!(
        "serving {} package indexes from {} on http://{}",
        app.indexes.len(),
        app.data_dir.display(),
        config.listen
    );
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accepting HTTP connection")?;
                let app = Arc::clone(&app);
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let app = Arc::clone(&app);
                        async move { Ok::<_, Infallible>(app.handle(request).await) }
                    });
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        eprintln!("HTTP connection failed: {error}");
                    }
                });
            }
            _ = &mut shutdown => {
                eprintln!("shutting down HTTP server");
                break;
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

struct LoadedPackageIndex {
    relative_dir: PathBuf,
    index: PackageIndex,
}

struct App {
    data_dir: PathBuf,
    indexes: Vec<LoadedPackageIndex>,
    manifest_json: Vec<u8>,
    index_html: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigRecord {
    pub distribution: String,
    pub release: String,
    pub package_name: String,
    pub version: String,
    pub architecture: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub config_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigResponse {
    pub schema_version: u32,
    pub config: String,
    pub records: Vec<ConfigRecord>,
}

impl App {
    fn load(data_dir: &Path, title: &str) -> Result<Self> {
        let data_dir = data_dir
            .canonicalize()
            .with_context(|| format!("opening data directory {}", data_dir.display()))?;
        if !data_dir.is_dir() {
            bail!("data path {} is not a directory", data_dir.display());
        }

        let paths = find_package_indexes(&data_dir)?;
        let mut indexes = Vec::with_capacity(paths.len());
        let mut configs = BTreeSet::new();
        let mut generated_at = None;

        for path in paths {
            let relative_dir = path
                .parent()
                .context("package index has no parent directory")?
                .strip_prefix(&data_dir)
                .with_context(|| format!("{} is outside the data directory", path.display()))?
                .to_path_buf();
            let index = read_package_index(&path)?;
            generated_at = Some(generated_at.map_or(index.generated_at, |current| {
                std::cmp::max(current, index.generated_at)
            }));
            configs.extend(
                index
                    .entries
                    .keys()
                    .map(|name| name.strip_prefix("CONFIG_").unwrap_or(name).to_string()),
            );
            indexes.push(LoadedPackageIndex {
                relative_dir,
                index,
            });
        }

        let manifest = SiteManifest {
            schema_version: 1,
            generated_at: generated_at.unwrap_or_else(Utc::now),
            configs: configs.into_iter().collect(),
        };

        Ok(Self {
            data_dir,
            indexes,
            manifest_json: serde_json::to_vec(&manifest).context("serializing API manifest")?,
            index_html: render_server_page(title, API_BASE)?.into_bytes(),
        })
    }

    async fn handle<B>(&self, request: Request<B>) -> Response<Full<Bytes>> {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return method_not_allowed(&request);
        }

        let path = request.uri().path();
        match path {
            "/" => cached_response(
                &request,
                StatusCode::OK,
                "text/html; charset=utf-8",
                self.index_html.clone(),
                HTML_CACHE,
            ),
            "/app.js" => cached_response(
                &request,
                StatusCode::OK,
                "text/javascript; charset=utf-8",
                include_bytes!("templates/app.js").to_vec(),
                ASSET_CACHE,
            ),
            "/styles.css" => cached_response(
                &request,
                StatusCode::OK,
                "text/css; charset=utf-8",
                include_bytes!("templates/styles.css").to_vec(),
                ASSET_CACHE,
            ),
            "/healthz" => response(
                &request,
                StatusCode::OK,
                "text/plain; charset=utf-8",
                b"ok\n".to_vec(),
                "no-store",
                None,
            ),
            "/api/v1/configs" => cached_response(
                &request,
                StatusCode::OK,
                "application/json; charset=utf-8",
                self.manifest_json.clone(),
                DATA_CACHE,
            ),
            _ if path.starts_with("/api/v1/configs/") => {
                self.handle_config(&request, &path["/api/v1/configs/".len()..])
            }
            _ if path.starts_with("/api/v1/raw/") => {
                self.handle_raw(&request, &path["/api/v1/raw/".len()..])
                    .await
            }
            _ if is_frontend_route(path) => cached_response(
                &request,
                StatusCode::OK,
                "text/html; charset=utf-8",
                self.index_html.clone(),
                HTML_CACHE,
            ),
            _ => error_response(&request, StatusCode::NOT_FOUND, "not found"),
        }
    }

    fn handle_config<B>(&self, request: &Request<B>, encoded_name: &str) -> Response<Full<Bytes>> {
        let Ok(decoded) = percent_decode_str(encoded_name).decode_utf8() else {
            return error_response(request, StatusCode::BAD_REQUEST, "invalid config name");
        };
        if decoded.is_empty() || decoded.contains('/') || decoded.contains('\\') {
            return error_response(request, StatusCode::BAD_REQUEST, "invalid config name");
        }

        let config = normalize_config_name(&decoded);
        let records = self.records_for_config(&config);
        if records.is_empty() {
            return error_response(
                request,
                StatusCode::NOT_FOUND,
                "config entry is not indexed",
            );
        }

        let result = ConfigResponse {
            schema_version: 1,
            config,
            records,
        };
        match serde_json::to_vec(&result) {
            Ok(json) => cached_response(
                request,
                StatusCode::OK,
                "application/json; charset=utf-8",
                json,
                DATA_CACHE,
            ),
            Err(_) => error_response(
                request,
                StatusCode::INTERNAL_SERVER_ERROR,
                "unable to serialize response",
            ),
        }
    }

    async fn handle_raw<B>(
        &self,
        request: &Request<B>,
        encoded_path: &str,
    ) -> Response<Full<Bytes>> {
        let Ok(decoded) = percent_decode_str(encoded_path).decode_utf8() else {
            return error_response(request, StatusCode::BAD_REQUEST, "invalid config path");
        };
        let relative = Path::new(decoded.as_ref());
        if relative.as_os_str().is_empty()
            || relative.file_name().is_none_or(|name| name != "config")
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return error_response(request, StatusCode::BAD_REQUEST, "invalid config path");
        }

        let requested = self.data_dir.join(relative);
        let canonical = match tokio::fs::canonicalize(&requested).await {
            Ok(path) if path.starts_with(&self.data_dir) => path,
            Ok(_) => return error_response(request, StatusCode::FORBIDDEN, "forbidden"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return error_response(request, StatusCode::NOT_FOUND, "config file not found");
            }
            Err(_) => {
                return error_response(
                    request,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "unable to open config file",
                );
            }
        };
        if !canonical.is_file() {
            return error_response(request, StatusCode::NOT_FOUND, "config file not found");
        }

        match tokio::fs::read(canonical).await {
            Ok(body) => cached_response(
                request,
                StatusCode::OK,
                "text/plain; charset=utf-8",
                body,
                RAW_CACHE,
            ),
            Err(_) => error_response(
                request,
                StatusCode::INTERNAL_SERVER_ERROR,
                "unable to read config file",
            ),
        }
    }

    fn records_for_config(&self, config: &str) -> Vec<ConfigRecord> {
        let mut records = Vec::new();
        for loaded in &self.indexes {
            let Some(occurrences) = loaded.index.entries.get(config) else {
                continue;
            };
            let occurrence_by_kernel = occurrences
                .iter()
                .map(|occurrence| (occurrence.kernel.as_str(), &occurrence.value))
                .collect::<BTreeMap<_, _>>();

            for (kernel_id, kernel) in &loaded.index.kernels {
                let value = occurrence_by_kernel
                    .get(kernel_id.as_str())
                    .map(|value| value.as_display_value().to_string())
                    .unwrap_or_else(|| ConfigValue::Missing.as_display_value().to_string());
                let raw_path = loaded.relative_dir.join(&kernel.config_path);
                records.push(ConfigRecord {
                    distribution: loaded.index.distribution.to_string(),
                    release: kernel.release.clone(),
                    package_name: loaded.index.package_name.clone(),
                    version: kernel.version.clone(),
                    architecture: kernel
                        .stored_architecture
                        .clone()
                        .unwrap_or_else(|| kernel.architecture.to_string()),
                    value,
                    source: kernel.source.clone().filter(|source| {
                        source.starts_with("https://") || source.starts_with("http://")
                    }),
                    config_url: format!(
                        "{API_BASE}/raw/{}",
                        encode_url_path(&raw_path.to_string_lossy())
                    ),
                });
            }
        }

        records.sort_by(|left, right| {
            (
                &left.distribution,
                &left.release,
                &left.package_name,
                &left.version,
                &left.architecture,
            )
                .cmp(&(
                    &right.distribution,
                    &right.release,
                    &right.package_name,
                    &right.version,
                    &right.architecture,
                ))
        });
        records
    }
}

fn is_frontend_route(path: &str) -> bool {
    let Some(config) = path.strip_prefix("/CONFIG_/") else {
        return false;
    };
    let config = config.strip_suffix('/').unwrap_or(config);
    !config.is_empty() && !config.contains('/')
}

fn encode_url_path(path: &str) -> String {
    path.split('/')
        .map(encode_url_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_url_segment(segment: &str) -> String {
    percent_encoding::utf8_percent_encode(segment, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn cached_response<B>(
    request: &Request<B>,
    status: StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
    cache_control: &'static str,
) -> Response<Full<Bytes>> {
    let etag = etag(&body);
    if request
        .headers()
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| etag_matches(value, &etag))
    {
        return response(
            request,
            StatusCode::NOT_MODIFIED,
            content_type,
            Vec::new(),
            cache_control,
            Some(&etag),
        );
    }
    response(
        request,
        status,
        content_type,
        body,
        cache_control,
        Some(&etag),
    )
}

fn response<B>(
    request: &Request<B>,
    status: StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
    cache_control: &'static str,
    etag: Option<&str>,
) -> Response<Full<Bytes>> {
    let content_length = body.len();
    let response_body = if request.method() == Method::HEAD {
        Vec::new()
    } else {
        body
    };
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(CACHE_CONTROL, cache_control)
        .header("CDN-Cache-Control", cache_control)
        .header("Cloudflare-CDN-Cache-Control", cache_control)
        .header("X-Content-Type-Options", "nosniff");
    if status != StatusCode::NOT_MODIFIED {
        builder = builder.header(CONTENT_LENGTH, content_length);
    }
    if let Some(etag) = etag {
        builder = builder.header(ETAG, etag);
    }
    builder
        .body(Full::new(Bytes::from(response_body)))
        .expect("response headers are valid")
}

fn error_response<B>(
    request: &Request<B>,
    status: StatusCode,
    message: &str,
) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .expect("serializing an error response cannot fail");
    response(
        request,
        status,
        "application/json; charset=utf-8",
        body,
        NEGATIVE_CACHE,
        None,
    )
}

fn method_not_allowed<B>(request: &Request<B>) -> Response<Full<Bytes>> {
    let mut response = response(
        request,
        StatusCode::METHOD_NOT_ALLOWED,
        "text/plain; charset=utf-8",
        b"method not allowed\n".to_vec(),
        "no-store",
        None,
    );
    response
        .headers_mut()
        .insert(ALLOW, "GET, HEAD".parse().expect("valid Allow header"));
    response
}

fn etag(body: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    body.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

fn etag_matches(header: &str, etag: &str) -> bool {
    header.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Architecture, Distribution, write_packages_to_data_dir};
    use crate::indexer::KernelConfigPackage;
    use http_body_util::BodyExt;

    fn test_app() -> (tempfile::TempDir, App) {
        let data = tempfile::tempdir().expect("data tempdir");
        write_packages_to_data_dir(
            [KernelConfigPackage {
                distribution: Distribution::Debian,
                release: "trixie".to_string(),
                package_name: "linux-image-amd64".to_string(),
                package_version: "6.1.0-1".to_string(),
                architecture: Architecture::Amd64,
                source: Some("https://example.test/linux.deb".to_string()),
                config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n".to_string(),
            }],
            data.path(),
        )
        .expect("write data");
        let app = App::load(data.path(), "test").expect("load app");
        (data, app)
    }

    #[test]
    fn loads_manifest_and_config_records_from_local_data() {
        let (_data, app) = test_app();
        let manifest: SiteManifest =
            serde_json::from_slice(&app.manifest_json).expect("manifest JSON");
        assert_eq!(manifest.configs, vec!["BPF", "EXT4_FS"]);
        let html = String::from_utf8_lossy(&app.index_html);
        assert!(html.contains("data-api-base=\"&#x2f;api&#x2f;v1\""));

        let records = app.records_for_config("CONFIG_BPF");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].distribution, "debian");
        assert_eq!(records[0].value, "y");
        assert_eq!(
            records[0].config_url,
            "/api/v1/raw/debian/linux%2Dimage%2Damd64/6%2E1%2E0%2D1/amd64/config"
        );
    }

    #[test]
    fn frontend_route_accepts_one_config_segment_only() {
        assert!(is_frontend_route("/CONFIG_/BPF/"));
        assert!(is_frontend_route("/CONFIG_/BPF"));
        assert!(!is_frontend_route("/CONFIG_/"));
        assert!(!is_frontend_route("/CONFIG_/BPF/more"));
    }

    #[test]
    fn weak_and_strong_etags_match() {
        assert!(etag_matches("\"abc\"", "\"abc\""));
        assert!(etag_matches("W/\"abc\"", "\"abc\""));
        assert!(etag_matches("\"no\", W/\"abc\"", "\"abc\""));
        assert!(!etag_matches("\"no\"", "\"abc\""));
    }

    #[tokio::test]
    async fn api_and_raw_routes_are_cacheable_and_conditional() {
        let (_data, app) = test_app();
        let request = Request::builder()
            .uri("/api/v1/configs/BPF")
            .body(())
            .expect("request");
        let response = app.handle(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], DATA_CACHE);
        assert_eq!(response.headers()["CDN-Cache-Control"], DATA_CACHE);
        let etag = response.headers()[ETAG].clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        let result: ConfigResponse = serde_json::from_slice(&body).expect("config response");
        assert_eq!(result.config, "CONFIG_BPF");

        let conditional = Request::builder()
            .uri("/api/v1/configs/BPF")
            .header(IF_NONE_MATCH, etag)
            .body(())
            .expect("conditional request");
        assert_eq!(
            app.handle(conditional).await.status(),
            StatusCode::NOT_MODIFIED
        );

        let raw = Request::builder()
            .uri(&result.records[0].config_url)
            .body(())
            .expect("raw request");
        let raw_response = app.handle(raw).await;
        assert_eq!(raw_response.status(), StatusCode::OK);
        assert_eq!(raw_response.headers()[CACHE_CONTROL], RAW_CACHE);
        let raw_body = raw_response
            .into_body()
            .collect()
            .await
            .expect("raw body")
            .to_bytes();
        assert!(raw_body.starts_with(b"CONFIG_BPF=y"));

        let head = Request::builder()
            .method(Method::HEAD)
            .uri("/api/v1/configs/BPF")
            .body(())
            .expect("HEAD request");
        let head_response = app.handle(head).await;
        assert_eq!(head_response.status(), StatusCode::OK);
        assert_ne!(head_response.headers()[CONTENT_LENGTH], "0");
        assert!(
            head_response
                .into_body()
                .collect()
                .await
                .expect("HEAD body")
                .to_bytes()
                .is_empty()
        );

        let index_file = Request::builder()
            .uri("/api/v1/raw/debian/linux%2Dimage%2Damd64/index%2Ejson")
            .body(())
            .expect("index request");
        assert_eq!(
            app.handle(index_file).await.status(),
            StatusCode::BAD_REQUEST
        );

        let post = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/configs")
            .body(())
            .expect("POST request");
        let post_response = app.handle(post).await;
        assert_eq!(post_response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(post_response.headers()[ALLOW], "GET, HEAD");
        assert_eq!(post_response.headers()[CACHE_CONTROL], "no-store");
    }
}
