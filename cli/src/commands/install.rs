//! `dr install` — fetch and install all dependencies.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{dependency_cache, manifest::Dependency, terminal, workspace::find_workspace};

const DEFAULT_REGISTRY_ORIGIN: &str = "https://daram.flyingsquirrel.me";
const DEFAULT_TRUSTED_SIGNING_KEY_ID: &str = "local-dev";
const DEFAULT_TRUSTED_SIGNING_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA6yOVMh5UY+KH9Y5Y/Tu2i93a2Lmdsn8/+odW8qCPs8w=\n-----END PUBLIC KEY-----\n";

#[derive(Debug, Deserialize)]
struct ResolvedPackage {
    name: String,
    version: String,
    sha256: String,
    archive_format: String,
    registry_url: String,
    signature: String,
    signing_key_id: String,
    yanked: bool,
}

struct InstallOptions {
    allow_yanked: bool,
    dev: bool,
    package_specs: Vec<String>,
}

pub fn run(args: &[String]) -> i32 {
    let options = parse_options(args);
    let mut ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    if !options.package_specs.is_empty() {
        if let Err(error) = add_requested_packages(&mut ws, &options.package_specs, options.dev) {
            terminal::error(&error);
            return 1;
        }
    }

    let deps = &ws.manifest.dependencies;
    let dev_deps = &ws.manifest.dev_dependencies;

    if deps.is_empty() && dev_deps.is_empty() {
        terminal::info("no dependencies to install");
        return 0;
    }

    let registry = std::env::var("DRPM_REGISTRY")
        .unwrap_or_else(|_| "https://daram.flyingsquirrel.me".to_string());
    let registry_origin = match canonical_registry_origin(&registry) {
        Ok(origin) => origin,
        Err(e) => {
            terminal::error(&e);
            return 1;
        }
    };

    let lock_path = ws.root.join("dr.lock");
    let mut lockfile = match dependency_cache::load_lockfile(&lock_path) {
        Ok(lock) => lock,
        Err(e) => {
            terminal::error(&format!("failed to read dr.lock: {}", e));
            return 1;
        }
    };

    terminal::step(&format!(
        "installing {} package(s)…",
        deps.len() + dev_deps.len()
    ));

    let pkg_dir = dependency_cache::package_cache_dir(&ws.root);
    if let Err(e) = fs::create_dir_all(&pkg_dir) {
        terminal::error(&format!("failed to create package directory: {}", e));
        return 1;
    }

    let mut resolved_entries = Vec::new();
    let mut resolved_dev_entries = Vec::new();
    let mut failed = 0usize;

    for (scope, dep_set) in [("dependency", deps), ("dev-dependency", dev_deps)] {
        for (name, dep) in dep_set {
            let requested = dep.version.0.trim();
            terminal::step(&format!("  resolving {} {} ({})", scope, name, requested));
            match install_dependency(
                &ws.root,
                &registry,
                &registry_origin,
                name,
                dep,
                &pkg_dir,
                lockfile.package.iter().find(|entry| {
                    entry.name == *name && entry.checksum.is_some() && entry.source.is_some()
                }),
                &options,
            ) {
                Ok(entry) => {
                    terminal::success(&format!("  installed {}@{}", entry.name, entry.version));
                    if scope == "dependency" {
                        resolved_entries.push(entry);
                    } else {
                        resolved_dev_entries.push(entry);
                    }
                }
                Err(e) => {
                    terminal::error(&format!("  failed to install {}: {}", name, e));
                    failed += 1;
                }
            }
        }
    }

    if failed > 0 {
        terminal::error(&format!("{} package(s) failed to install", failed));
        return 1;
    }

    lockfile.package = dependency_cache::build_lock_packages(
        &ws.manifest.package.name,
        &ws.manifest.package.version,
        resolved_entries,
        resolved_dev_entries,
    );
    if let Err(e) = dependency_cache::write_lockfile(&lock_path, &lockfile) {
        terminal::error(&format!("failed to write dr.lock: {}", e));
        return 1;
    }

    terminal::success("all packages installed");
    0
}

fn parse_options(args: &[String]) -> InstallOptions {
    InstallOptions {
        allow_yanked: args.iter().any(|arg| arg == "--allow-yanked"),
        dev: args.iter().any(|arg| arg == "--dev"),
        package_specs: args
            .iter()
            .filter(|arg| !arg.starts_with('-'))
            .cloned()
            .collect(),
    }
}

fn add_requested_packages(
    ws: &mut crate::workspace::Workspace,
    package_specs: &[String],
    dev: bool,
) -> Result<(), String> {
    for spec in package_specs {
        let (name, version) = parse_package_spec(spec)?;
        let dep = Dependency::version_only(&version);
        if dev {
            ws.manifest.dev_dependencies.insert(name.clone(), dep);
        } else {
            ws.manifest.dependencies.insert(name.clone(), dep);
        }
        let kind = if dev { "dev-dependency" } else { "dependency" };
        terminal::success(&format!("added {} `{}`", kind, spec));
    }

    ws.manifest
        .write_to_dir(&ws.root)
        .map_err(|e| format!("failed to update daram.toml: {}", e))?;
    Ok(())
}

fn parse_package_spec(spec: &str) -> Result<(String, String), String> {
    let (name, version) = match spec.split_once('@') {
        Some((name, version)) => (name.trim().to_string(), version.trim().to_string()),
        None => (spec.trim().to_string(), "*".to_string()),
    };
    if name.is_empty() {
        return Err("package name must not be empty".to_string());
    }
    validate_package_coord(&name)?;
    if version != "*" {
        validate_version_coord(&version)?;
    }
    Ok((name, version))
}

fn install_dependency(
    workspace_root: &Path,
    registry: &str,
    registry_origin: &str,
    name: &str,
    dep: &Dependency,
    pkg_dir: &Path,
    existing_lock: Option<&dependency_cache::LockPackage>,
    options: &InstallOptions,
) -> Result<dependency_cache::LockPackage, String> {
    validate_package_coord(name)?;

    let requested_version = dep.version.0.trim();
    let resolved = resolve_package(
        registry,
        registry_origin,
        name,
        requested_version,
        options.allow_yanked,
    )?;
    verify_against_lock(existing_lock, &resolved)?;
    verify_signature(&resolved)?;
    let archive_path = download_and_verify(registry, &resolved, pkg_dir, options.allow_yanked)?;
    let metadata = cache_installed_package(workspace_root, &resolved, &archive_path)?;

    Ok(dependency_cache::LockPackage {
        name: resolved.name,
        version: resolved.version,
        source: Some(format!("registry+{}", resolved.registry_url)),
        checksum: Some(format!("sha256:{}", resolved.sha256)),
        registry_url: Some(resolved.registry_url),
        signing_key_id: Some(resolved.signing_key_id),
        dependencies: Some(
            metadata
                .dependencies
                .iter()
                .map(|dependency| format!("{} {}", dependency.name, dependency.version))
                .collect(),
        ),
        dev_dependencies: None,
    })
}

fn resolve_package(
    registry: &str,
    registry_origin: &str,
    name: &str,
    version_req: &str,
    allow_yanked: bool,
) -> Result<ResolvedPackage, String> {
    let query_version = normalize_version_query(version_req);
    let mut url = format!(
        "{}/api/v1/packages/resolve?name={}&version={}",
        registry.trim_end_matches('/'),
        percent_encode(name),
        percent_encode(&query_version),
    );
    if allow_yanked {
        url.push_str("&allow_yanked=true");
    }

    let output = Command::new("curl")
        .args(["-fsSL", &url])
        .output()
        .map_err(map_curl_error)?;
    if !output.status.success() {
        return Err(curl_failure_message(&output));
    }

    let pkg = serde_json::from_slice::<ResolvedPackage>(&output.stdout)
        .map_err(|e| format!("invalid resolve response: {}", e))?;
    validate_package_coord(&pkg.name)?;
    validate_version_coord(&pkg.version)?;
    validate_sha256(&pkg.sha256)?;
    if !matches!(pkg.archive_format.as_str(), "tar.gz" | "zip") {
        return Err(format!(
            "unsupported archive format from registry: {}",
            pkg.archive_format
        ));
    }
    if pkg.registry_url != *registry_origin {
        return Err(format!(
            "registry origin mismatch: expected {}, got {}",
            registry_origin, pkg.registry_url,
        ));
    }
    if pkg.yanked && !allow_yanked {
        return Err(format!("package {}@{} is yanked", pkg.name, pkg.version));
    }
    Ok(pkg)
}

fn verify_against_lock(
    existing_lock: Option<&dependency_cache::LockPackage>,
    resolved: &ResolvedPackage,
) -> Result<(), String> {
    let Some(entry) = existing_lock else {
        return Ok(());
    };

    if entry.version != resolved.version {
        return Ok(());
    }

    let locked_checksum = entry.checksum.as_deref().unwrap_or_default();
    let locked_registry = entry.registry_url.as_deref().unwrap_or_default();
    let locked_key = entry.signing_key_id.as_deref().unwrap_or_default();
    let expected_checksum = format!("sha256:{}", resolved.sha256);
    if locked_checksum != expected_checksum {
        return Err(format!(
            "lockfile checksum mismatch for {}@{}",
            resolved.name, resolved.version
        ));
    }
    if !locked_registry.is_empty() && locked_registry != resolved.registry_url {
        return Err(format!(
            "lockfile registry mismatch for {}@{}",
            resolved.name, resolved.version
        ));
    }
    if !locked_key.is_empty() && locked_key != resolved.signing_key_id {
        return Err(format!(
            "lockfile signing key mismatch for {}@{}",
            resolved.name, resolved.version
        ));
    }
    Ok(())
}

fn verify_signature(resolved: &ResolvedPackage) -> Result<(), String> {
    let pinned_key_id = trusted_signing_key_id(resolved);
    if !pinned_key_id.is_empty() && pinned_key_id != resolved.signing_key_id {
        return Err(format!(
            "unexpected signing key: expected {}, got {}",
            pinned_key_id, resolved.signing_key_id
        ));
    }

    let payload = signature_payload(resolved);
    let payload_base64 = BASE64.encode(&payload);
    if let Ok(command) = std::env::var("DRPM_VERIFY_COMMAND") {
        return verify_signature_with_command(&command, resolved, &payload_base64);
    }
    if let Some(public_key_pem) = trusted_signing_public_key_pem(resolved) {
        return verify_signature_with_openssl(&public_key_pem, resolved, &payload);
    }
    Err(
        "signature verification is required; configure DRPM_VERIFY_COMMAND or DRPM_TRUSTED_SIGNING_PUBLIC_KEY_PEM"
            .to_string(),
    )
}

fn trusted_signing_key_id(resolved: &ResolvedPackage) -> String {
    match std::env::var("DRPM_TRUSTED_SIGNING_KEY_ID") {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ if resolved.registry_url == DEFAULT_REGISTRY_ORIGIN => {
            DEFAULT_TRUSTED_SIGNING_KEY_ID.to_string()
        }
        _ => String::new(),
    }
}

fn trusted_signing_public_key_pem(resolved: &ResolvedPackage) -> Option<String> {
    match std::env::var("DRPM_TRUSTED_SIGNING_PUBLIC_KEY_PEM") {
        Ok(value) if !value.trim().is_empty() => Some(normalize_pem_env(&value)),
        _ if resolved.registry_url == DEFAULT_REGISTRY_ORIGIN => {
            Some(DEFAULT_TRUSTED_SIGNING_PUBLIC_KEY_PEM.to_string())
        }
        _ => None,
    }
}

fn normalize_pem_env(value: &str) -> String {
    value.trim().replace("\\n", "\n")
}

fn verify_signature_with_command(
    command: &str,
    resolved: &ResolvedPackage,
    payload_base64: &str,
) -> Result<(), String> {
    let argv = split_command(command);
    if argv.is_empty() {
        return Err("DRPM_VERIFY_COMMAND must not be empty".to_string());
    }

    let input = serde_json::json!({
        "kind": "drpm-resolve-v2",
        "name": resolved.name,
        "version": resolved.version,
        "sha256": resolved.sha256,
        "archive_format": resolved.archive_format,
        "registry_url": resolved.registry_url,
        "signature": resolved.signature,
        "signing_key_id": resolved.signing_key_id,
        "payload_base64": payload_base64,
    })
    .to_string();

    let output = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(input.as_bytes())?;
            }
            child.wait_with_output()
        })
        .map_err(|e| format!("failed to execute verifier: {}", e))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!(
            "signature verification failed with status {}",
            output.status
        ))
    } else {
        Err(format!("signature verification failed: {}", stderr))
    }
}

fn verify_signature_with_openssl(
    public_key_pem: &str,
    resolved: &ResolvedPackage,
    payload: &[u8],
) -> Result<(), String> {
    let signature = BASE64
        .decode(resolved.signature.as_bytes())
        .map_err(|e| format!("invalid base64 signature: {}", e))?;
    let temp_dir = temp_dir_path(&format!("verify-{}-{}", resolved.name, resolved.version));
    fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

    let key_path = temp_dir.join("key.pem");
    let payload_path = temp_dir.join("payload.bin");
    let signature_path = temp_dir.join("signature.bin");
    fs::write(&key_path, public_key_pem).map_err(|e| e.to_string())?;
    fs::write(&payload_path, payload).map_err(|e| e.to_string())?;
    fs::write(&signature_path, signature).map_err(|e| e.to_string())?;

    let output = Command::new("openssl")
        .args([
            "pkeyutl",
            "-verify",
            "-pubin",
            "-inkey",
            key_path
                .to_str()
                .ok_or_else(|| "invalid key path".to_string())?,
            "-rawin",
            "-in",
            payload_path
                .to_str()
                .ok_or_else(|| "invalid payload path".to_string())?,
            "-sigfile",
            signature_path
                .to_str()
                .ok_or_else(|| "invalid signature path".to_string())?,
        ])
        .output()
        .map_err(|e| format!("failed to execute openssl: {}", e))?;

    let _ = fs::remove_dir_all(&temp_dir);

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!(
            "signature verification failed for {}@{}",
            resolved.name, resolved.version
        ))
    } else {
        Err(format!("signature verification failed: {}", stderr))
    }
}

fn download_and_verify(
    registry: &str,
    resolved: &ResolvedPackage,
    pkg_dir: &Path,
    allow_yanked: bool,
) -> Result<PathBuf, String> {
    let extension = archive_extension(&resolved.archive_format)?;
    let dest = pkg_dir.join(format!(
        "{}-{}.{}",
        resolved.name, resolved.version, extension
    ));
    if dest.exists() {
        let existing_hash = sha256_file(&dest)?;
        if existing_hash == resolved.sha256 {
            return Ok(dest);
        }
        fs::remove_file(&dest).map_err(|e| e.to_string())?;
    }

    let temp_path = pkg_dir.join(format!(
        ".tmp-{}-{}-{}.{}",
        resolved.name,
        resolved.version,
        std::process::id(),
        extension
    ));

    let mut url = format!(
        "{}/api/v1/packages/download?name={}&version={}",
        registry.trim_end_matches('/'),
        percent_encode(&resolved.name),
        percent_encode(&resolved.version),
    );
    if allow_yanked {
        url.push_str("&allow_yanked=true");
    }

    let status = Command::new("curl")
        .args([
            "-fsSL",
            "--output",
            temp_path
                .to_str()
                .ok_or_else(|| "invalid package cache path".to_string())?,
            &url,
        ])
        .status()
        .map_err(map_curl_error)?;
    if !status.success() {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("curl exited with status {}", status));
    }

    let digest = sha256_file(&temp_path)?;
    if digest != resolved.sha256 {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "downloaded archive checksum mismatch for {}@{}",
            resolved.name, resolved.version
        ));
    }

    fs::rename(&temp_path, &dest).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        e.to_string()
    })?;
    Ok(dest)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let digest = Sha256::digest(bytes);
    Ok(format!("{:x}", digest))
}

fn signature_payload(resolved: &ResolvedPackage) -> Vec<u8> {
    [
        "drpm-resolve-v2",
        resolved.name.as_str(),
        resolved.version.as_str(),
        resolved.sha256.as_str(),
        resolved.archive_format.as_str(),
        resolved.registry_url.as_str(),
        "",
    ]
    .join("\n")
    .into_bytes()
}

fn canonical_registry_origin(registry: &str) -> Result<String, String> {
    let trimmed = registry.trim();
    let parsed = url::Url::parse(trimmed).map_err(|e| format!("invalid DRPM_REGISTRY: {}", e))?;
    match parsed.scheme() {
        "https" => {}
        "http" if matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")) => {}
        _ => {
            return Err(
                "DRPM_REGISTRY must use https (or http for localhost development)".to_string(),
            )
        }
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "DRPM_REGISTRY must include a host".to_string())?;
    let mut origin = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        origin.push_str(&format!(":{}", port));
    }
    Ok(origin)
}

fn normalize_version_query(version_req: &str) -> String {
    let trimmed = version_req.trim();
    if trimmed.is_empty()
        || trimmed == "*"
        || trimmed.contains('^')
        || trimmed.contains('~')
        || trimmed.contains('>')
        || trimmed.contains('<')
        || trimmed.contains('=')
        || trimmed.contains(',')
    {
        return "latest".to_string();
    }
    trimmed.to_string()
}

fn archive_extension(format: &str) -> Result<&'static str, String> {
    match format {
        "tar.gz" => Ok("tar.gz"),
        "zip" => Ok("zip"),
        other => Err(format!("unsupported archive format: {}", other)),
    }
}

fn split_command(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

fn temp_dir_path(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("dr-{}-{}", prefix, std::process::id()));
    path
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

fn curl_failure_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("curl exited with status {}", output.status)
    } else {
        stderr
    }
}

fn map_curl_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "curl not found; install curl to use dr install".to_string()
    } else {
        error.to_string()
    }
}

fn validate_package_coord(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("invalid package name".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "package name contains invalid characters: `{}`",
            name
        ));
    }
    if name.contains("..") || name.starts_with('.') {
        return Err(format!("package name is not allowed: `{}`", name));
    }
    Ok(())
}

fn validate_version_coord(version: &str) -> Result<(), String> {
    if version.is_empty() || version.len() > 32 {
        return Err("invalid version string".to_string());
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' || c == '*')
    {
        return Err(format!(
            "version contains invalid characters: `{}`",
            version
        ));
    }
    Ok(())
}

fn validate_sha256(sha256: &str) -> Result<(), String> {
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("registry returned invalid sha256".to_string());
    }
    Ok(())
}

fn cache_installed_package(
    workspace_root: &Path,
    resolved: &ResolvedPackage,
    archive_path: &Path,
) -> Result<dependency_cache::CachedPackageMetadata, String> {
    let extract_dir =
        dependency_cache::expanded_package_dir(workspace_root, &resolved.name, &resolved.version);
    dependency_cache::extract_archive_to_dir(archive_path, &resolved.archive_format, &extract_dir)?;
    let metadata = dependency_cache::inspect_extracted_package(
        &resolved.name,
        &resolved.version,
        &extract_dir,
    )?;
    dependency_cache::write_cached_metadata(workspace_root, &metadata)?;
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_pem_env, parse_package_spec, trusted_signing_key_id,
        trusted_signing_public_key_pem, ResolvedPackage, DEFAULT_REGISTRY_ORIGIN,
        DEFAULT_TRUSTED_SIGNING_KEY_ID, DEFAULT_TRUSTED_SIGNING_PUBLIC_KEY_PEM,
    };

    fn sample_resolved(registry_url: &str) -> ResolvedPackage {
        ResolvedPackage {
            name: "dotenv".to_string(),
            version: "1.0.0".to_string(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            archive_format: "zip".to_string(),
            registry_url: registry_url.to_string(),
            signature: "sig".to_string(),
            signing_key_id: DEFAULT_TRUSTED_SIGNING_KEY_ID.to_string(),
            yanked: false,
        }
    }

    #[test]
    fn parses_install_package_specs() {
        assert_eq!(
            parse_package_spec("dotenv@1.0.0").unwrap(),
            ("dotenv".to_string(), "1.0.0".to_string())
        );
        assert_eq!(
            parse_package_spec("dotenv").unwrap(),
            ("dotenv".to_string(), "*".to_string())
        );
    }

    #[test]
    fn normalizes_pem_env_newlines() {
        assert_eq!(normalize_pem_env("A\\nB\\nC"), "A\nB\nC".to_string());
    }

    #[test]
    fn uses_default_registry_public_key_when_env_is_unset() {
        unsafe {
            std::env::remove_var("DRPM_TRUSTED_SIGNING_KEY_ID");
            std::env::remove_var("DRPM_TRUSTED_SIGNING_PUBLIC_KEY_PEM");
        }

        let resolved = sample_resolved(DEFAULT_REGISTRY_ORIGIN);
        assert_eq!(
            trusted_signing_key_id(&resolved),
            DEFAULT_TRUSTED_SIGNING_KEY_ID
        );
        assert_eq!(
            trusted_signing_public_key_pem(&resolved).unwrap(),
            DEFAULT_TRUSTED_SIGNING_PUBLIC_KEY_PEM.to_string()
        );
    }
}
