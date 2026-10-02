use std::collections::hash_map::DefaultHasher;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use http::header::{ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use http::{Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::disk::DiskLookup;
use kconfigwtf::index::normalize_config_name;
use kconfigwtf::site::render_server_page;

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

struct App {
    data_dir: PathBuf,
    indexes: Arc<DiskLookup>,
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

        let indexes = Arc::new(DiskLookup::load(&data_dir)?);
        let manifest_json = serde_json::to_vec(&indexes.manifest)?;

        Ok(Self {
            data_dir,
            indexes,
            manifest_json,
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
                include_bytes!("../../../../../../../src/templates/app.js").to_vec(),
                ASSET_CACHE,
            ),
            "/styles.css" => cached_response(
                &request,
                StatusCode::OK,
                "text/css; charset=utf-8",
                include_bytes!("../../../../../../../src/templates/styles.css").to_vec(),
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
                    .await
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

    async fn handle_config<B>(
        &self,
        request: &Request<B>,
        encoded_name: &str,
    ) -> Response<Full<Bytes>> {
        let Ok(decoded) = percent_decode_str(encoded_name).decode_utf8() else {
            return error_response(request, StatusCode::BAD_REQUEST, "invalid config name");
        };
        if decoded.is_empty() || decoded.contains('/') || decoded.contains('\\') {
            return error_response(request, StatusCode::BAD_REQUEST, "invalid config name");
        }

        let config = normalize_config_name(&decoded);
        let indexes = Arc::clone(&self.indexes);
        let query = config.clone();
        let records =
            match tokio::task::spawn_blocking(move || indexes.records_for_config(&query)).await {
                Ok(Ok(records)) => records,
                _ => {
                    return error_response(
                        request,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "unable to read index",
                    );
                }
            };
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
}

fn is_frontend_route(path: &str) -> bool {
    let Some(config) = path.strip_prefix("/CONFIG_/") else {
        return false;
    };
    let config = config.strip_suffix('/').unwrap_or(config);
    !config.is_empty() && !config.contains('/')
}

#[allow(dead_code)]
fn encode_url_path(path: &str) -> String {
    path.split('/')
        .map(encode_url_segment)
        .collect::<Vec<_>>()
        .join("/")
}

#[allow(dead_code)]
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
