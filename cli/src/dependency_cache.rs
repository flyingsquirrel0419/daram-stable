use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use tar::Archive;
use zip::ZipArchive;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Lockfile {
    #[serde(default)]
    pub package: Vec<LockPackage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev_dependencies: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedDependency {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedPackageMetadata {
    pub name: String,
    pub version: String,
    pub module_name: String,
    pub main: String,
    pub source_files: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<CachedDependency>,
}

pub fn drpm_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".drpm")
}

pub fn package_cache_dir(workspace_root: &Path) -> PathBuf {
    drpm_root(workspace_root).join("packages")
}

pub fn expanded_package_dir(workspace_root: &Path, name: &str, version: &str) -> PathBuf {
    drpm_root(workspace_root)
        .join("expanded")
        .join(name)
        .join(version)
}

pub fn metadata_path(workspace_root: &Path, name: &str, version: &str) -> PathBuf {
    drpm_root(workspace_root)
        .join("metadata")
        .join(name)
        .join(version)
        .join("manifest.json")
}

pub fn load_lockfile(path: &Path) -> Result<Lockfile, String> {
    if !path.exists() {
        return Ok(Lockfile::default());
    }

    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(Lockfile::default());
    }

    toml::from_str(&text).map_err(|e| e.to_string())
}

pub fn write_lockfile(path: &Path, lockfile: &Lockfile) -> Result<(), String> {
    let rendered = toml::to_string(lockfile).map_err(|e| e.to_string())?;
    fs::write(path, rendered).map_err(|e| e.to_string())
}

pub fn build_lock_packages(
    root_name: &str,
    root_version: &str,
    mut dependencies: Vec<LockPackage>,
    mut dev_dependencies: Vec<LockPackage>,
) -> Vec<LockPackage> {
    dependencies.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.version.cmp(&right.version))
    });
    dev_dependencies.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.version.cmp(&right.version))
    });

    let dependency_refs = dependencies
        .iter()
        .map(lock_package_ref)
        .collect::<Vec<_>>();
    let dev_dependency_refs = dev_dependencies
        .iter()
        .map(lock_package_ref)
        .collect::<Vec<_>>();

    let mut packages = vec![LockPackage {
        name: root_name.to_string(),
        version: root_version.to_string(),
        source: None,
        checksum: None,
        registry_url: None,
        signing_key_id: None,
        dependencies: Some(dependency_refs),
        dev_dependencies: Some(dev_dependency_refs),
    }];
    packages.extend(dependencies);
    packages.extend(dev_dependencies);
    packages
}

pub fn write_cached_metadata(
    workspace_root: &Path,
    metadata: &CachedPackageMetadata,
) -> Result<(), String> {
    let path = metadata_path(workspace_root, &metadata.name, &metadata.version);
    let parent = path
        .parent()
        .ok_or_else(|| "invalid metadata path".to_string())?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let rendered = serde_json::to_string_pretty(metadata).map_err(|e| e.to_string())?;
    fs::write(path, rendered).map_err(|e| e.to_string())
}

pub fn load_cached_metadata(
    workspace_root: &Path,
    name: &str,
    version: &str,
) -> Result<CachedPackageMetadata, String> {
    let path = metadata_path(workspace_root, name, version);
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "failed to read cached package metadata `{}`: {}",
            path.display(),
            e
        )
    })?;
    let mut metadata = serde_json::from_str::<CachedPackageMetadata>(&text).map_err(|e| {
        format!(
            "failed to parse cached package metadata `{}`: {}",
            path.display(),
            e
        )
    })?;
    metadata.module_name = package_module_name(&metadata.name);
    Ok(metadata)
}

pub fn extract_archive_to_dir(
    archive_path: &Path,
    archive_format: &str,
    dest_dir: &Path,
) -> Result<(), String> {
    if dest_dir.exists() {
        fs::remove_dir_all(dest_dir).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;

    match archive_format {
        "tar.gz" => extract_tar_gz_archive(archive_path, dest_dir),
        "zip" => extract_zip_archive(archive_path, dest_dir),
        other => Err(format!("unsupported archive format: {}", other)),
    }
}

pub fn inspect_extracted_package(
    package_name: &str,
    version: &str,
    extract_dir: &Path,
) -> Result<CachedPackageMetadata, String> {
    let source_files = collect_relative_dr_files(extract_dir)?;
    if source_files.is_empty() {
        return Err(format!(
            "package {}@{} does not contain any .dr source files",
            package_name, version
        ));
    }

    let manifest_path = extract_dir.join("drpkg.toml");
    let (manifest_name, main, dependencies) = if manifest_path.is_file() {
        parse_package_manifest(package_name, &manifest_path, &source_files)?
    } else {
        let fallback = if source_files
            .iter()
            .any(|file| file == &format!("{package_name}.dr"))
        {
            format!("{package_name}.dr")
        } else if source_files.iter().any(|file| file == "main.dr") {
            "main.dr".to_string()
        } else {
            source_files[0].clone()
        };
        (package_name.to_string(), fallback, Vec::new())
    };

    Ok(CachedPackageMetadata {
        name: manifest_name,
        version: version.to_string(),
        module_name: package_module_name(package_name),
        main,
        source_files,
        dependencies,
    })
}

pub fn compose_workspace_source(
    workspace_root: &Path,
    current_file: Option<&Path>,
    source: &str,
    include_dev_dependencies: bool,
) -> Result<String, String> {
    let bundled_dependencies = dependency_source_bundle(workspace_root, include_dev_dependencies)?;
    let workspace_source = bundle_workspace_sources(workspace_root, current_file, source)?;
    let merged = if bundled_dependencies.trim().is_empty() {
        workspace_source
    } else {
        daram_compiler::stdlib_bundle::merge_source_bundles(&[
            bundled_dependencies,
            workspace_source,
        ])
    };
    Ok(daram_compiler::stdlib_bundle::with_bundled_prelude(&merged))
}

pub fn dependency_source_bundle(
    workspace_root: &Path,
    include_dev_dependencies: bool,
) -> Result<String, String> {
    let lockfile = load_lockfile(&workspace_root.join("dr.lock"))?;
    if lockfile.package.is_empty() {
        return Ok(String::new());
    }

    let root = lockfile
        .package
        .iter()
        .find(|entry| entry.source.is_none())
        .ok_or_else(|| "dr.lock is missing the root package entry".to_string())?;

    let package_map = lockfile
        .package
        .iter()
        .filter(|entry| entry.source.is_some())
        .map(|entry| ((entry.name.clone(), entry.version.clone()), entry))
        .collect::<HashMap<_, _>>();

    let mut requested = root.dependencies.clone().unwrap_or_default();
    if include_dev_dependencies {
        requested.extend(root.dev_dependencies.clone().unwrap_or_default());
    }

    let mut ordered = Vec::new();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();

    for spec in requested {
        let key = parse_lock_reference(&spec)?;
        visit_dependency(
            &package_map,
            &key,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }

    let mut versions_by_name = BTreeMap::<String, BTreeSet<String>>::new();
    for (name, version) in &ordered {
        versions_by_name
            .entry(name.clone())
            .or_default()
            .insert(version.clone());
    }
    if let Some((name, versions)) = versions_by_name
        .into_iter()
        .find(|(_, versions)| versions.len() > 1)
    {
        return Err(format!(
            "multiple installed versions for `{}` are not supported: {}",
            name,
            versions.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    let mut bundled_files = Vec::new();
    for (name, version) in ordered {
        let metadata = load_cached_metadata(workspace_root, &name, &version)?;
        let package_root = expanded_package_dir(workspace_root, &name, &version);
        bundled_files.extend(render_cached_package(&package_root, &metadata)?);
    }
    Ok(daram_compiler::stdlib_bundle::encode_source_bundle(
        &bundled_files,
    ))
}

pub fn lock_package_ref(package: &LockPackage) -> String {
    format!("{} {}", package.name, package.version)
}

pub fn strip_outer_attributes(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim().starts_with("#["))
        .collect::<Vec<_>>()
        .join("\n")
}

fn visit_dependency<'a>(
    package_map: &HashMap<(String, String), &'a LockPackage>,
    key: &(String, String),
    visiting: &mut HashSet<(String, String)>,
    visited: &mut HashSet<(String, String)>,
    ordered: &mut Vec<(String, String)>,
) -> Result<(), String> {
    if visited.contains(key) {
        return Ok(());
    }
    if !visiting.insert(key.clone()) {
        return Err(format!("dependency cycle detected at {} {}", key.0, key.1));
    }

    let package = package_map
        .get(key)
        .ok_or_else(|| format!("dr.lock references missing package {} {}", key.0, key.1))?;
    for child in package.dependencies.clone().unwrap_or_default() {
        let child_key = parse_lock_reference(&child)?;
        visit_dependency(package_map, &child_key, visiting, visited, ordered)?;
    }

    visiting.remove(key);
    visited.insert(key.clone());
    ordered.push(key.clone());
    Ok(())
}

fn render_cached_package(
    package_root: &Path,
    metadata: &CachedPackageMetadata,
) -> Result<Vec<(String, String)>, String> {
    let mut files = Vec::new();
    for relative_path in &metadata.source_files {
        let source_path = package_root.join(relative_path);
        let source = fs::read_to_string(&source_path).map_err(|e| {
            format!(
                "failed to read cached source `{}`: {}",
                source_path.display(),
                e
            )
        })?;
        let rewritten = rewrite_package_source(
            &strip_outer_attributes(&source),
            relative_path,
            &metadata.main,
            &metadata.module_name,
        );
        let bundled_path =
            package_bundled_path(relative_path, &metadata.main, &metadata.module_name);
        files.push((bundled_path, rewritten));
    }
    Ok(files)
}

fn rewrite_dependency_source(source: &str, package_module_name: &str) -> String {
    source
        .replace("crate::std", "std")
        .replace("crate::", &format!("{package_module_name}::"))
}

fn bundle_workspace_sources(
    workspace_root: &Path,
    current_file: Option<&Path>,
    current_source: &str,
) -> Result<String, String> {
    let src_root = workspace_root.join("src");
    if !src_root.is_dir() {
        return Ok(daram_compiler::stdlib_bundle::encode_source_bundle(&[(
            "main.dr".to_string(),
            strip_outer_attributes(current_source),
        )]));
    }

    let Some(current_file) = current_file else {
        return Ok(daram_compiler::stdlib_bundle::encode_source_bundle(&[(
            "main.dr".to_string(),
            strip_outer_attributes(current_source),
        )]));
    };
    let current_relative = current_file
        .strip_prefix(&src_root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"));
    if current_relative.is_none() {
        return Ok(daram_compiler::stdlib_bundle::encode_source_bundle(&[(
            "main.dr".to_string(),
            strip_outer_attributes(current_source),
        )]));
    }
    let relative_files = collect_relative_dr_files(&src_root)?;

    let mut files = Vec::new();
    for relative_path in relative_files {
        let source = if current_relative.as_deref() == Some(relative_path.as_str()) {
            current_source.to_string()
        } else {
            fs::read_to_string(src_root.join(&relative_path))
                .map_err(|e| format!("failed to read `{}`: {}", relative_path, e))?
        };
        let rewritten = rewrite_workspace_source(&strip_outer_attributes(&source), &relative_path);
        files.push((relative_path, rewritten));
    }

    Ok(daram_compiler::stdlib_bundle::encode_source_bundle(&files))
}

fn rewrite_workspace_source(source: &str, relative_path: &str) -> String {
    rewrite_import_sources(
        source,
        &module_dir_segments_from_relative(relative_path, "main.dr"),
    )
}

fn rewrite_package_source(
    source: &str,
    relative_path: &str,
    main_file: &str,
    package_module_name: &str,
) -> String {
    let rewritten = rewrite_import_sources(
        source,
        &module_dir_segments_from_relative(relative_path, main_file),
    );
    rewrite_dependency_source(&rewritten, package_module_name)
}

fn package_bundled_path(
    relative_path: &str,
    root_entry: &str,
    package_module_name: &str,
) -> String {
    let normalized = relative_path.replace('\\', "/");
    if normalized == root_entry || normalized == "main.dr" || normalized == "lib.dr" {
        return format!("{package_module_name}/main.dr");
    }

    let mut segments = normalized
        .split('/')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if segments.last().is_some_and(|segment| segment == "mod.dr") {
        let _ = segments.pop();
        return format!("{package_module_name}/{}", segments.join("/")) + "/mod.dr";
    } else if let Some(last) = segments.last_mut() {
        if let Some(stripped) = last.strip_suffix(".dr") {
            *last = format!("{stripped}.dr");
        }
    }
    format!("{package_module_name}/{}", segments.join("/"))
}

fn module_dir_segments_from_relative(relative_path: &str, root_entry: &str) -> Vec<String> {
    let normalized = relative_path.replace('\\', "/");
    if normalized == root_entry || normalized == "main.dr" || normalized == "lib.dr" {
        return Vec::new();
    }

    let mut parts = normalized
        .split('/')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.last().is_some_and(|segment| segment == "mod.dr") {
        parts.pop();
    } else {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|segment| sanitize_module_segment(&segment))
        .collect()
}

fn rewrite_import_sources(source: &str, current_dir: &[String]) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;
    let mut expect_import_source = false;
    let mut block_depth = 0usize;

    while i < bytes.len() {
        if block_depth > 0 {
            if bytes[i..].starts_with(b"/*") {
                block_depth += 1;
                out.push_str("/*");
                i += 2;
            } else if bytes[i..].starts_with(b"*/") {
                block_depth -= 1;
                out.push_str("*/");
                i += 2;
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }

        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() {
                let ch = bytes[i] as char;
                out.push(ch);
                i += 1;
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }

        if bytes[i..].starts_with(b"/*") {
            block_depth = 1;
            out.push_str("/*");
            i += 2;
            continue;
        }

        if bytes[i] == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let literal = &source[start + 1..i.saturating_sub(1)];
            if expect_import_source {
                let rewritten = canonicalize_import_source(literal, current_dir);
                out.push('"');
                out.push_str(&escape_string_literal(&rewritten));
                out.push('"');
                expect_import_source = false;
            } else {
                out.push_str(&source[start..i]);
            }
            continue;
        }

        let ch = bytes[i] as char;
        if is_ident_start(ch) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i] as char) {
                i += 1;
            }
            let ident = &source[start..i];
            out.push_str(ident);
            if ident == "from" {
                expect_import_source = true;
            } else if expect_import_source {
                expect_import_source = false;
            }
            continue;
        }

        if expect_import_source && !ch.is_whitespace() {
            expect_import_source = false;
        }
        out.push(ch);
        i += 1;
    }

    out
}

fn canonicalize_import_source(raw: &str, current_dir: &[String]) -> String {
    if raw.starts_with("./") || raw.starts_with("../") {
        let mut segments = current_dir.to_vec();
        for part in raw.replace('\\', "/").split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                value => segments.push(sanitize_module_segment(value.trim_end_matches(".dr"))),
            }
        }
        if segments.last().is_some_and(|segment| segment == "mod") {
            segments.pop();
        }
        return segments.join("/");
    }

    let mut segments = raw
        .replace("::", "/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .enumerate()
        .map(|(index, segment)| {
            let segment = segment.trim_end_matches(".dr");
            if index == 0 {
                package_module_name(segment)
            } else {
                sanitize_module_segment(segment)
            }
        })
        .collect::<Vec<_>>();
    if segments.last().is_some_and(|segment| segment == "mod") {
        segments.pop();
    }
    segments.join("/")
}

fn escape_string_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn parse_package_manifest(
    expected_name: &str,
    manifest_path: &Path,
    source_files: &[String],
) -> Result<(String, String, Vec<CachedDependency>), String> {
    let text = fs::read_to_string(manifest_path)
        .map_err(|e| format!("failed to read `{}`: {}", manifest_path.display(), e))?;
    let value = text
        .parse::<toml::Value>()
        .map_err(|e| format!("invalid drpkg.toml: {}", e))?;
    let package = value
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "drpkg.toml must contain a [package] table".to_string())?;

    let manifest_name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .unwrap_or(expected_name)
        .trim()
        .to_string();
    let main = package
        .get("main")
        .and_then(toml::Value::as_str)
        .unwrap_or("main.dr")
        .trim()
        .to_string();
    if !source_files.iter().any(|path| path == &main) {
        return Err(format!(
            "package {} manifest points to missing main source `{}`",
            manifest_name, main
        ));
    }

    let dependencies = value
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .map(|(name, raw)| {
                    let spec = raw
                        .as_table()
                        .ok_or_else(|| format!("dependency {} must be a table", name))?;
                    let version = spec
                        .get("version")
                        .and_then(toml::Value::as_str)
                        .ok_or_else(|| format!("dependency {} is missing version", name))?
                        .trim()
                        .to_string();
                    let source = spec
                        .get("source")
                        .and_then(toml::Value::as_str)
                        .map(|value| value.trim().to_string());
                    Ok(CachedDependency {
                        name: name.to_string(),
                        version,
                        source,
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok((manifest_name, main, dependencies))
}

fn collect_relative_dr_files(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    collect_relative_dr_files_recursive(root, root, &mut files).map_err(|e| e.to_string())?;
    files.sort();
    Ok(files)
}

fn collect_relative_dr_files_recursive(
    root: &Path,
    dir: &Path,
    files: &mut Vec<String>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_relative_dr_files_recursive(root, &path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dr") {
            let relative = path
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }

    Ok(())
}

fn extract_tar_gz_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|e| e.to_string())?;

    for entry in entries {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err("archive contains an unsupported symbolic link".to_string());
        }
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err("archive contains an unsupported entry type".to_string());
        }

        let relative = normalized_relative_path(&entry.path().map_err(|e| e.to_string())?)?;
        let target = dest_dir.join(relative);
        if entry_type.is_dir() {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry.unpack(&target).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn extract_zip_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
        let relative = normalized_relative_path(Path::new(file.name()))?;
        let target = dest_dir.join(relative);

        let is_symlink = file
            .unix_mode()
            .map(|mode| (mode & 0o170000) == 0o120000)
            .unwrap_or(false);
        if is_symlink {
            return Err("archive contains an unsupported symbolic link".to_string());
        }

        if file.name().ends_with('/') {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut output = fs::File::create(&target).map_err(|e| e.to_string())?;
        io::copy(&mut file, &mut output).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn normalized_relative_path(path: &Path) -> Result<PathBuf, String> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err("archive contains an absolute path".to_string())
            }
            Component::ParentDir => {
                return Err("archive contains a path traversal segment".to_string())
            }
        }
    }

    if relative.as_os_str().is_empty() {
        return Err("archive contains an empty path".to_string());
    }
    Ok(relative)
}

fn parse_lock_reference(spec: &str) -> Result<(String, String), String> {
    let mut parts = spec.split_whitespace();
    let name = parts
        .next()
        .ok_or_else(|| format!("invalid package reference `{}`", spec))?;
    let version = parts
        .next()
        .ok_or_else(|| format!("invalid package reference `{}`", spec))?;
    if parts.next().is_some() {
        return Err(format!("invalid package reference `{}`", spec));
    }
    Ok((name.to_string(), version.to_string()))
}

fn sanitize_module_segment(module_name: &str) -> String {
    module_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn package_module_name(package_name: &str) -> String {
    if package_name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        package_name.to_string()
    } else {
        let mut encoded = String::from("_pkg_");
        for byte in package_name.bytes() {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "{byte:02x}");
        }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daram_compiler::compile;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::FileOptions;

    #[test]
    fn extracted_package_metadata_reads_manifest_and_sources() {
        let root = temp_test_dir("extract-metadata");
        let archive_path = root.join("dep.zip");
        write_test_zip(
            &archive_path,
            &[
                (
                    "drpkg.toml",
                    "[package]\nname = \"greeter\"\nversion = \"1.0.0\"\nmain = \"src/main.dr\"\n",
                ),
                ("src/main.dr", "pub fn answer() -> i32 { 42 }\n"),
            ],
        );

        let extract_dir = root.join("expanded");
        extract_archive_to_dir(&archive_path, "zip", &extract_dir).unwrap();
        let metadata = inspect_extracted_package("greeter", "1.0.0", &extract_dir).unwrap();

        assert_eq!(metadata.name, "greeter");
        assert_eq!(metadata.main, "src/main.dr");
        assert_eq!(metadata.source_files, vec!["src/main.dr"]);
    }

    #[test]
    fn bundled_dependency_source_compiles_with_user_import() {
        let root = temp_test_dir("bundle-compile");
        fs::create_dir_all(expanded_package_dir(&root, "greeter", "1.0.0")).unwrap();
        fs::write(
            expanded_package_dir(&root, "greeter", "1.0.0").join("main.dr"),
            "pub fn answer() -> i32 { 42 }\n",
        )
        .unwrap();
        write_cached_metadata(
            &root,
            &CachedPackageMetadata {
                name: "greeter".to_string(),
                version: "1.0.0".to_string(),
                module_name: "greeter".to_string(),
                main: "main.dr".to_string(),
                source_files: vec!["main.dr".to_string()],
                dependencies: Vec::new(),
            },
        )
        .unwrap();
        write_lockfile(
            &root.join("dr.lock"),
            &Lockfile {
                package: vec![
                    LockPackage {
                        name: "app".to_string(),
                        version: "1.0.0".to_string(),
                        source: None,
                        checksum: None,
                        registry_url: None,
                        signing_key_id: None,
                        dependencies: Some(vec!["greeter 1.0.0".to_string()]),
                        dev_dependencies: Some(Vec::new()),
                    },
                    LockPackage {
                        name: "greeter".to_string(),
                        version: "1.0.0".to_string(),
                        source: Some("registry+https://registry.example.test".to_string()),
                        checksum: Some("sha256:deadbeef".to_string()),
                        registry_url: Some("https://registry.example.test".to_string()),
                        signing_key_id: Some("test".to_string()),
                        dependencies: Some(Vec::new()),
                        dev_dependencies: None,
                    },
                ],
            },
        )
        .unwrap();

        let bundled = compose_workspace_source(
            &root,
            None,
            "fn main() -> i32 { greeter::answer() }\n",
            false,
        )
        .unwrap();
        let result = compile(&bundled, "main.dr");

        assert!(!result.has_errors());
    }

    #[test]
    fn workspace_bundle_resolves_relative_file_imports() {
        let root = temp_test_dir("workspace-relative-import");
        fs::create_dir_all(root.join("src/db")).unwrap();
        fs::write(
            root.join("src/main.dr"),
            "import { answer } from \"./db/query\";\nfun main(): i32 { answer() }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/db/query.dr"),
            "export fun answer(): i32 { 42 }\n",
        )
        .unwrap();

        let main_path = root.join("src/main.dr");
        let source = fs::read_to_string(&main_path).unwrap();
        let bundled = compose_workspace_source(&root, Some(&main_path), &source, false).unwrap();
        let result = compile(&bundled, "main.dr");

        assert!(
            !result.has_errors(),
            "diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn workspace_bundle_uses_explicit_source_file_outside_src_root() {
        let root = temp_test_dir("workspace-external-file");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/main.dr"),
            "fun main() {\n    println(\"Hello from app\");\n}\n",
        )
        .unwrap();

        let external = root.join("hello.dr");
        let source = "fun main() {\n    println(\"Hello user\");\n}\n";
        fs::write(&external, source).unwrap();

        let bundled = compose_workspace_source(&root, Some(&external), source, false).unwrap();

        assert!(bundled.contains("Hello user"));
        assert!(!bundled.contains("Hello from app"));
    }

    #[test]
    fn dependency_bundle_resolves_relative_imports_inside_package() {
        let root = temp_test_dir("dependency-relative-import");
        fs::create_dir_all(expanded_package_dir(&root, "greeter", "1.0.0")).unwrap();
        fs::write(
            expanded_package_dir(&root, "greeter", "1.0.0").join("main.dr"),
            "import { answer_impl } from \"./util\";\nexport fun answer(): i32 { answer_impl() }\n",
        )
        .unwrap();
        fs::write(
            expanded_package_dir(&root, "greeter", "1.0.0").join("util.dr"),
            "export fun answer_impl(): i32 { 42 }\n",
        )
        .unwrap();
        write_cached_metadata(
            &root,
            &CachedPackageMetadata {
                name: "greeter".to_string(),
                version: "1.0.0".to_string(),
                module_name: "greeter".to_string(),
                main: "main.dr".to_string(),
                source_files: vec!["main.dr".to_string(), "util.dr".to_string()],
                dependencies: Vec::new(),
            },
        )
        .unwrap();
        write_lockfile(
            &root.join("dr.lock"),
            &Lockfile {
                package: vec![
                    LockPackage {
                        name: "app".to_string(),
                        version: "1.0.0".to_string(),
                        source: None,
                        checksum: None,
                        registry_url: None,
                        signing_key_id: None,
                        dependencies: Some(vec!["greeter 1.0.0".to_string()]),
                        dev_dependencies: Some(Vec::new()),
                    },
                    LockPackage {
                        name: "greeter".to_string(),
                        version: "1.0.0".to_string(),
                        source: Some("registry+https://registry.example.test".to_string()),
                        checksum: Some("sha256:deadbeef".to_string()),
                        registry_url: Some("https://registry.example.test".to_string()),
                        signing_key_id: Some("test".to_string()),
                        dependencies: Some(Vec::new()),
                        dev_dependencies: None,
                    },
                ],
            },
        )
        .unwrap();

        let bundled = compose_workspace_source(
            &root,
            None,
            "import { answer } from greeter;\nfun main(): i32 { answer() }\n",
            false,
        )
        .unwrap();
        let result = compile(&bundled, "main.dr");

        assert!(
            !result.has_errors(),
            "diagnostics: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn dependency_bundle_rejects_multiple_versions_for_same_package_name() {
        let root = temp_test_dir("bundle-conflict");
        write_lockfile(
            &root.join("dr.lock"),
            &Lockfile {
                package: vec![
                    LockPackage {
                        name: "app".to_string(),
                        version: "1.0.0".to_string(),
                        source: None,
                        checksum: None,
                        registry_url: None,
                        signing_key_id: None,
                        dependencies: Some(vec!["dep 1.0.0".to_string(), "dep 2.0.0".to_string()]),
                        dev_dependencies: Some(Vec::new()),
                    },
                    LockPackage {
                        name: "dep".to_string(),
                        version: "1.0.0".to_string(),
                        source: Some("registry+https://registry.example.test".to_string()),
                        checksum: Some("sha256:one".to_string()),
                        registry_url: Some("https://registry.example.test".to_string()),
                        signing_key_id: Some("test".to_string()),
                        dependencies: Some(Vec::new()),
                        dev_dependencies: None,
                    },
                    LockPackage {
                        name: "dep".to_string(),
                        version: "2.0.0".to_string(),
                        source: Some("registry+https://registry.example.test".to_string()),
                        checksum: Some("sha256:two".to_string()),
                        registry_url: Some("https://registry.example.test".to_string()),
                        signing_key_id: Some("test".to_string()),
                        dependencies: Some(Vec::new()),
                        dev_dependencies: None,
                    },
                ],
            },
        )
        .unwrap();

        let error = dependency_source_bundle(&root, false).unwrap_err();
        assert!(error.contains("multiple installed versions"));
    }

    #[test]
    fn bundled_dependencies_support_package_names_with_distinct_normalized_shapes() {
        let root = temp_test_dir("bundle-package-name-collision");

        for (name, body) in [
            ("foo-bar", "export fun dash_answer(): i32 { 40 }\n"),
            ("foo_bar", "export fun underscore_answer(): i32 { 2 }\n"),
        ] {
            fs::create_dir_all(expanded_package_dir(&root, name, "1.0.0")).unwrap();
            fs::write(
                expanded_package_dir(&root, name, "1.0.0").join("main.dr"),
                body,
            )
            .unwrap();
            write_cached_metadata(
                &root,
                &CachedPackageMetadata {
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    module_name: sanitize_module_segment(name),
                    main: "main.dr".to_string(),
                    source_files: vec!["main.dr".to_string()],
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        }

        write_lockfile(
            &root.join("dr.lock"),
            &Lockfile {
                package: vec![
                    LockPackage {
                        name: "app".to_string(),
                        version: "1.0.0".to_string(),
                        source: None,
                        checksum: None,
                        registry_url: None,
                        signing_key_id: None,
                        dependencies: Some(vec![
                            "foo-bar 1.0.0".to_string(),
                            "foo_bar 1.0.0".to_string(),
                        ]),
                        dev_dependencies: Some(Vec::new()),
                    },
                    LockPackage {
                        name: "foo-bar".to_string(),
                        version: "1.0.0".to_string(),
                        source: Some("registry+https://registry.example.test".to_string()),
                        checksum: Some("sha256:dash".to_string()),
                        registry_url: Some("https://registry.example.test".to_string()),
                        signing_key_id: Some("test".to_string()),
                        dependencies: Some(Vec::new()),
                        dev_dependencies: None,
                    },
                    LockPackage {
                        name: "foo_bar".to_string(),
                        version: "1.0.0".to_string(),
                        source: Some("registry+https://registry.example.test".to_string()),
                        checksum: Some("sha256:underscore".to_string()),
                        registry_url: Some("https://registry.example.test".to_string()),
                        signing_key_id: Some("test".to_string()),
                        dependencies: Some(Vec::new()),
                        dev_dependencies: None,
                    },
                ],
            },
        )
        .unwrap();

        let src_root = root.join("src");
        fs::create_dir_all(&src_root).unwrap();
        let main_path = src_root.join("main.dr");
        fs::write(
            &main_path,
            "import { dash_answer } from \"foo-bar\";\nimport { underscore_answer } from foo_bar;\nfun main(): i32 { dash_answer() + underscore_answer() }\n",
        )
        .unwrap();

        let source = fs::read_to_string(&main_path).unwrap();
        let bundled = compose_workspace_source(&root, Some(&main_path), &source, false).unwrap();
        let result = compile(&bundled, "main.dr");

        assert!(
            !result.has_errors(),
            "diagnostics: {:?}",
            result.diagnostics
        );
    }

    fn write_test_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::<()>::default();
        for (name, contents) in entries {
            zip.start_file(name, options).unwrap();
            use std::io::Write as _;
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dr-cli-{prefix}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
