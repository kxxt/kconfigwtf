use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn looks_like_kernel_config(text: &str) -> bool {
    if looks_like_html(text) || text.trim().is_empty() {
        return false;
    }

    text.lines().take(200).any(|line| {
        let line = line.trim();
        line.starts_with("CONFIG_")
            || line.starts_with("# CONFIG_")
            || line.contains("Kernel Configuration")
    })
}

pub fn extract_ikconfig_from_image(image: &[u8]) -> Result<String> {
    let script = locate_extract_ikconfig_script()?;
    let temp_path = std::env::temp_dir().join(format!(
        "kconfigwtf-boot-{}-{}.img",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&temp_path, image)
        .with_context(|| format!("writing temporary boot image {}", temp_path.display()))?;

    let output = Command::new("sh")
        .arg(&script)
        .arg(&temp_path)
        .output()
        .with_context(|| format!("running {}", script.display()))?;
    let _ = std::fs::remove_file(&temp_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "extract-ikconfig failed for {}: {stderr}",
            temp_path.display()
        );
    }

    let config = String::from_utf8(output.stdout).context("decoding extract-ikconfig stdout")?;
    if !looks_like_kernel_config(&config) {
        bail!("extract-ikconfig output did not look like a kernel config");
    }
    Ok(config)
}

fn locate_extract_ikconfig_script() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("EXTRACT_IKCONFIG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "EXTRACT_IKCONFIG is set but {} does not exist",
            path.display()
        );
    }

    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/extract-ikconfig");
    if bundled.is_file() {
        return Ok(bundled);
    }

    if let Ok(path) = which_extract_ikconfig() {
        return Ok(path);
    }

    bail!("extract-ikconfig was not found; set EXTRACT_IKCONFIG or install the kernel script");
}

fn which_extract_ikconfig() -> Result<PathBuf> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("command -v extract-ikconfig")
        .output()
        .context("locating extract-ikconfig in PATH")?;
    if !output.status.success() {
        bail!("extract-ikconfig is not available in PATH");
    }
    let path = String::from_utf8(output.stdout)
        .context("decoding extract-ikconfig path")?
        .trim()
        .to_string();
    if path.is_empty() {
        bail!("extract-ikconfig is not available in PATH");
    }
    Ok(PathBuf::from(path))
}

pub(crate) fn looks_like_html(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.contains("<artifact-page")
}
