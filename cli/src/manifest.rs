//! `daram.toml` manifest parsing and manipulation.
//!
//! The manifest is the single source of truth for a Daram project's
//! metadata, dependencies, build configuration, and tool settings.

use std::{collections::HashMap, fmt, fs, io, path::Path};

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ManifestError {
    Io(io::Error),
    Parse(String),
    MissingField(String),
    InvalidValue { field: String, reason: String },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Io(e) => write!(f, "I/O error: {}", e),
            ManifestError::Parse(e) => write!(f, "parse error: {}", e),
            ManifestError::MissingField(k) => write!(f, "missing required field `{}`", k),
            ManifestError::InvalidValue { field, reason } => {
                write!(f, "invalid value for `{}`: {}", field, reason)
            }
        }
    }
}

impl From<io::Error> for ManifestError {
    fn from(e: io::Error) -> Self {
        ManifestError::Io(e)
    }
}

// ─── Manifest types ───────────────────────────────────────────────────────────

/// `[package]` section.
#[derive(Debug, Clone)]
pub struct PackageMeta {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub repository: Option<String>,
    pub readme: Option<String>,
    pub keywords: Vec<String>,
}

/// `[build]` section.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub target: BuildTarget,
    pub opt_level: u8,
    pub debug_info: bool,
    pub incremental: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            target: BuildTarget::NativeDebug,
            opt_level: 0,
            debug_info: true,
            incremental: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildTarget {
    NativeDebug,
    NativeRelease,
    Js,
    Wasm,
    Custom(String),
}

impl BuildTarget {
    pub fn from_str(s: &str) -> Self {
        match s {
            "native" | "native-debug" => BuildTarget::NativeDebug,
            "native-release" | "release" => BuildTarget::NativeRelease,
            "js" => BuildTarget::Js,
            "wasm" => BuildTarget::Wasm,
            other => BuildTarget::Custom(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            BuildTarget::NativeDebug => "native-debug",
            BuildTarget::NativeRelease => "native-release",
            BuildTarget::Js => "js",
            BuildTarget::Wasm => "wasm",
            BuildTarget::Custom(s) => s,
        }
    }
}

/// A version requirement for a dependency.
#[derive(Debug, Clone)]
pub struct VersionReq(pub String);

/// A single dependency.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub version: VersionReq,
    pub registry: Option<String>,
    pub path: Option<String>,
    pub optional: bool,
}

impl Dependency {
    /// Simple version-only dependency.
    pub fn version_only(ver: &str) -> Self {
        Self {
            version: VersionReq(ver.to_string()),
            registry: None,
            path: None,
            optional: false,
        }
    }

    fn to_toml_value(&self) -> String {
        if self.registry.is_none() && self.path.is_none() && !self.optional {
            return format!("{:?}", self.version.0);
        }

        let mut fields = vec![format!("version = {:?}", self.version.0)];
        if let Some(registry) = &self.registry {
            fields.push(format!("registry = {:?}", registry));
        }
        if let Some(path) = &self.path {
            fields.push(format!("path = {:?}", path));
        }
        if self.optional {
            fields.push("optional = true".to_string());
        }
        format!("{{ {} }}", fields.join(", "))
    }
}

/// `[fmt]` configuration.
#[derive(Debug, Clone)]
pub struct FmtConfig {
    pub indent_size: usize,
    pub max_line_length: usize,
    pub trailing_comma: bool,
}

impl Default for FmtConfig {
    fn default() -> Self {
        Self {
            indent_size: 4,
            max_line_length: 100,
            trailing_comma: true,
        }
    }
}

/// `[lint]` configuration.
#[derive(Debug, Clone, Default)]
pub struct LintConfig {
    /// Additional lints to enable (lint name → level).
    pub enable: HashMap<String, LintLevel>,
    pub disable: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    Allow,
    Warn,
    Deny,
    Forbid,
}

impl LintLevel {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "warn" => Some(Self::Warn),
            "deny" => Some(Self::Deny),
            "forbid" => Some(Self::Forbid),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Warn => "warn",
            Self::Deny => "deny",
            Self::Forbid => "forbid",
        }
    }
}

/// `[doc]` configuration.
#[derive(Debug, Clone)]
pub struct DocConfig {
    pub output_dir: String,
    pub include_private: bool,
}

impl Default for DocConfig {
    fn default() -> Self {
        Self {
            output_dir: "docs".to_string(),
            include_private: false,
        }
    }
}

/// The complete parsed `daram.toml` manifest.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub package: PackageMeta,
    pub build: BuildConfig,
    pub dependencies: HashMap<String, Dependency>,
    pub dev_dependencies: HashMap<String, Dependency>,
    pub features: HashMap<String, Vec<String>>,
    pub fmt: FmtConfig,
    pub lint: LintConfig,
    pub doc: DocConfig,
}

// ─── Parsing ──────────────────────────────────────────────────────────────────

impl Manifest {
    /// Load and parse `daram.toml` from `path`.
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = fs::read_to_string(path)?;
        Self::from_str(&content)
    }

    /// Parse a TOML string directly.
    pub fn from_str(toml_src: &str) -> Result<Self, ManifestError> {
        // Minimal hand-rolled TOML subset parser.
        // For a production compiler we would use the `toml` crate.
        let table = parse_toml_basic(toml_src)?;

        let pkg_table = table
            .get("package")
            .and_then(|v| {
                if let TomlValue::Table(t) = v {
                    Some(t)
                } else {
                    None
                }
            })
            .ok_or_else(|| ManifestError::MissingField("package".into()))?;

        let name = string_field(pkg_table, "name")?;
        let version = string_field(pkg_table, "version")?;
        let edition = pkg_table
            .get("edition")
            .and_then(TomlValue::as_string)
            .unwrap_or("2026")
            .to_string();

        validate_package_name(&name)?;
        validate_semver(&version)?;

        let package = PackageMeta {
            name,
            version,
            edition,
            description: pkg_table
                .get("description")
                .and_then(TomlValue::as_string)
                .map(str::to_string),
            authors: string_array(pkg_table.get("authors")).unwrap_or_default(),
            license: pkg_table
                .get("license")
                .and_then(TomlValue::as_string)
                .map(str::to_string),
            repository: pkg_table
                .get("repository")
                .and_then(TomlValue::as_string)
                .map(str::to_string),
            readme: pkg_table
                .get("readme")
                .and_then(TomlValue::as_string)
                .map(str::to_string),
            keywords: string_array(pkg_table.get("keywords")).unwrap_or_default(),
        };

        let build = if let Some(TomlValue::Table(bt)) = table.get("build") {
            let target = bt
                .get("target")
                .and_then(TomlValue::as_string)
                .map(BuildTarget::from_str)
                .unwrap_or(BuildTarget::NativeDebug);
            let opt_level = bt
                .get("opt-level")
                .and_then(TomlValue::as_int)
                .unwrap_or(0)
                .clamp(0, 3) as u8;
            let debug_info = bt
                .get("debug-info")
                .and_then(TomlValue::as_bool)
                .unwrap_or(true);
            let incremental = bt
                .get("incremental")
                .and_then(TomlValue::as_bool)
                .unwrap_or(true);
            BuildConfig {
                target,
                opt_level,
                debug_info,
                incremental,
            }
        } else {
            BuildConfig::default()
        };

        let dependencies = parse_deps(table.get("dependencies"))?;
        let dev_dependencies = parse_deps(table.get("dev-dependencies"))?;
        let features = parse_features(table.get("features")).unwrap_or_default();

        let fmt = if let Some(TomlValue::Table(ft)) = table.get("fmt") {
            FmtConfig {
                indent_size: ft
                    .get("indent-size")
                    .and_then(TomlValue::as_int)
                    .unwrap_or(4) as usize,
                max_line_length: ft
                    .get("max-line-length")
                    .and_then(TomlValue::as_int)
                    .unwrap_or(100) as usize,
                trailing_comma: ft
                    .get("trailing-comma")
                    .and_then(TomlValue::as_bool)
                    .unwrap_or(true),
            }
        } else {
            FmtConfig::default()
        };

        let lint = if let Some(TomlValue::Table(lt)) = table.get("lint") {
            let mut enable = HashMap::new();
            let disable = string_array(lt.get("disable")).unwrap_or_default();
            for (name, value) in lt {
                if name == "disable" {
                    continue;
                }
                if let Some(level) = value.as_string().and_then(LintLevel::from_str) {
                    enable.insert(name.clone(), level);
                }
            }
            LintConfig { enable, disable }
        } else {
            LintConfig::default()
        };

        let doc = if let Some(TomlValue::Table(dt)) = table.get("doc") {
            DocConfig {
                output_dir: dt
                    .get("output-dir")
                    .and_then(TomlValue::as_string)
                    .unwrap_or("docs")
                    .to_string(),
                include_private: dt
                    .get("include-private")
                    .and_then(TomlValue::as_bool)
                    .unwrap_or(false),
            }
        } else {
            DocConfig::default()
        };

        Ok(Manifest {
            package,
            build,
            dependencies,
            dev_dependencies,
            features,
            fmt,
            lint,
            doc,
        })
    }

    /// Serialise to a TOML string.
    pub fn to_toml_string(&self) -> String {
        let mut out = String::new();
        out.push_str("[package]\n");
        out.push_str(&format!("name = {:?}\n", self.package.name));
        out.push_str(&format!("version = {:?}\n", self.package.version));
        out.push_str(&format!("edition = {:?}\n", self.package.edition));
        if let Some(desc) = &self.package.description {
            out.push_str(&format!("description = {:?}\n", desc));
        }
        if !self.package.authors.is_empty() {
            out.push_str(&format!(
                "authors = [{}]\n",
                self.package
                    .authors
                    .iter()
                    .map(|a| format!("{:?}", a))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(lic) = &self.package.license {
            out.push_str(&format!("license = {:?}\n", lic));
        }
        if let Some(repository) = &self.package.repository {
            out.push_str(&format!("repository = {:?}\n", repository));
        }
        if let Some(readme) = &self.package.readme {
            out.push_str(&format!("readme = {:?}\n", readme));
        }
        if !self.package.keywords.is_empty() {
            out.push_str(&format!(
                "keywords = [{}]\n",
                self.package
                    .keywords
                    .iter()
                    .map(|keyword| format!("{:?}", keyword))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str("\n[build]\n");
        out.push_str(&format!("target = {:?}\n", self.build.target.as_str()));
        out.push_str(&format!("opt-level = {}\n", self.build.opt_level));
        out.push_str(&format!("debug-info = {}\n", self.build.debug_info));
        out.push_str(&format!("incremental = {}\n", self.build.incremental));
        out.push_str("\n[dependencies]\n");
        for (name, dep) in &self.dependencies {
            out.push_str(&format!("{} = {}\n", name, dep.to_toml_value()));
        }
        out.push_str("\n[dev-dependencies]\n");
        for (name, dep) in &self.dev_dependencies {
            out.push_str(&format!("{} = {}\n", name, dep.to_toml_value()));
        }
        out.push_str("\n[features]\n");
        for (name, members) in &self.features {
            out.push_str(&format!(
                "{} = [{}]\n",
                name,
                members
                    .iter()
                    .map(|member| format!("{:?}", member))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str("\n[lint]\n");
        if !self.lint.disable.is_empty() {
            out.push_str(&format!(
                "disable = [{}]\n",
                self.lint
                    .disable
                    .iter()
                    .map(|lint| format!("{:?}", lint))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for (name, level) in &self.lint.enable {
            out.push_str(&format!("{} = {:?}\n", name, level.as_str()));
        }
        out.push_str("\n[fmt]\n");
        out.push_str(&format!("indent-size = {}\n", self.fmt.indent_size));
        out.push_str(&format!("max-line-length = {}\n", self.fmt.max_line_length));
        out.push_str(&format!("trailing-comma = {}\n", self.fmt.trailing_comma));
        out.push_str("\n[doc]\n");
        out.push_str(&format!("output-dir = {:?}\n", self.doc.output_dir));
        out.push_str(&format!("include-private = {}\n", self.doc.include_private));
        out
    }

    /// Write to `daram.toml` in the given directory.
    pub fn write_to_dir(&self, dir: &Path) -> Result<(), ManifestError> {
        let path = dir.join("daram.toml");
        fs::write(path, self.to_toml_string()).map_err(ManifestError::Io)
    }
}

// ─── Validation ───────────────────────────────────────────────────────────────

/// SECURITY: reject package names that could be used for path traversal or
/// injection attacks. Only allow `[a-z0-9_-]` with a leading letter or digit.
pub fn validate_package_name(name: &str) -> Result<(), ManifestError> {
    if name.is_empty() || name.len() > 64 {
        return Err(ManifestError::InvalidValue {
            field: "name".into(),
            reason: "package name must be 1–64 characters".into(),
        });
    }
    if !name
        .chars()
        .next()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        return Err(ManifestError::InvalidValue {
            field: "name".into(),
            reason: "package name must start with a letter or digit".into(),
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ManifestError::InvalidValue {
            field: "name".into(),
            reason: "package name may only contain [a-z0-9_-]".into(),
        });
    }
    // Disallow names that look like path components.
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(ManifestError::InvalidValue {
            field: "name".into(),
            reason: "package name must not be a path component".into(),
        });
    }
    Ok(())
}

fn validate_semver(version: &str) -> Result<(), ManifestError> {
    // Minimal check: three dot-separated integers.
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.parse::<u64>().is_err()) {
        return Err(ManifestError::InvalidValue {
            field: "version".into(),
            reason: "version must be a semver string like \"1.0.0\"".into(),
        });
    }
    Ok(())
}

// ─── Minimal TOML parser ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum TomlValue {
    String(String),
    Int(i64),
    Bool(bool),
    Array(Vec<TomlValue>),
    Table(HashMap<String, TomlValue>),
}

impl TomlValue {
    fn as_string(&self) -> Option<&str> {
        if let TomlValue::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
    fn as_int(&self) -> Option<i64> {
        if let TomlValue::Int(n) = self {
            Some(*n)
        } else {
            None
        }
    }
    fn as_bool(&self) -> Option<bool> {
        if let TomlValue::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
}

fn string_field(table: &HashMap<String, TomlValue>, key: &str) -> Result<String, ManifestError> {
    table
        .get(key)
        .and_then(TomlValue::as_string)
        .map(str::to_string)
        .ok_or_else(|| ManifestError::MissingField(key.into()))
}

fn string_array(val: Option<&TomlValue>) -> Option<Vec<String>> {
    if let Some(TomlValue::Array(arr)) = val {
        Some(
            arr.iter()
                .filter_map(TomlValue::as_string)
                .map(str::to_string)
                .collect(),
        )
    } else {
        None
    }
}

fn parse_deps(val: Option<&TomlValue>) -> Result<HashMap<String, Dependency>, ManifestError> {
    let mut out = HashMap::new();
    if let Some(TomlValue::Table(table)) = val {
        for (name, v) in table {
            let dep = match v {
                TomlValue::String(ver) => Dependency::version_only(ver),
                TomlValue::Table(dt) => {
                    let ver = dt
                        .get("version")
                        .and_then(TomlValue::as_string)
                        .unwrap_or("*")
                        .to_string();
                    let path = dt
                        .get("path")
                        .and_then(TomlValue::as_string)
                        .map(str::to_string);
                    let registry = dt
                        .get("registry")
                        .and_then(TomlValue::as_string)
                        .map(str::to_string);
                    let optional = dt
                        .get("optional")
                        .and_then(TomlValue::as_bool)
                        .unwrap_or(false);
                    Dependency {
                        version: VersionReq(ver),
                        registry,
                        path,
                        optional,
                    }
                }
                _ => {
                    return Err(ManifestError::Parse(format!(
                        "invalid dependency entry for `{}`",
                        name
                    )))
                }
            };
            out.insert(name.clone(), dep);
        }
    }
    Ok(out)
}

fn parse_features(val: Option<&TomlValue>) -> Option<HashMap<String, Vec<String>>> {
    if let Some(TomlValue::Table(t)) = val {
        let mut out = HashMap::new();
        for (k, v) in t {
            if let Some(arr) = string_array(Some(v)) {
                out.insert(k.clone(), arr);
            }
        }
        Some(out)
    } else {
        None
    }
}

/// A very small TOML parser that handles the subset used by `daram.toml`.
/// Supports: string, integer, boolean, inline arrays of strings, and tables/subtables.
fn parse_toml_basic(src: &str) -> Result<HashMap<String, TomlValue>, ManifestError> {
    let mut root: HashMap<String, TomlValue> = HashMap::new();
    let mut current_table: Option<String> = None;

    for (line_no, line) in src.lines().enumerate() {
        let line = line.trim();
        // Skip blanks and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Table header `[name]`
        if line.starts_with('[') && !line.starts_with("[[") {
            let inner = line.trim_start_matches('[').trim_end_matches(']').trim();
            current_table = Some(inner.to_string());
            root.entry(inner.to_string())
                .or_insert_with(|| TomlValue::Table(HashMap::new()));
            continue;
        }
        // Key-value pair
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let val_src = line[eq_pos + 1..].trim();
            let val = parse_toml_value(val_src)
                .map_err(|e| ManifestError::Parse(format!("line {}: {}", line_no + 1, e)))?;
            if let Some(table_name) = &current_table {
                if let Some(TomlValue::Table(t)) = root.get_mut(table_name.as_str()) {
                    t.insert(key, val);
                }
            } else {
                root.insert(key, val);
            }
        }
    }
    Ok(root)
}

fn parse_toml_value(s: &str) -> Result<TomlValue, String> {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        return Ok(TomlValue::String(unescape_toml_string(inner)));
    }
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let mut items = Vec::new();
        for item in split_toml_array(inner) {
            items.push(parse_toml_value(item.trim())?);
        }
        return Ok(TomlValue::Array(items));
    }
    if s == "true" {
        return Ok(TomlValue::Bool(true));
    }
    if s == "false" {
        return Ok(TomlValue::Bool(false));
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(TomlValue::Int(n));
    }
    Err(format!("unrecognised TOML value: {:?}", s))
}

fn unescape_toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn split_toml_array(s: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                items.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start <= s.len() {
        let tail = s[start..].trim();
        if !tail.is_empty() {
            items.push(tail);
        }
    }
    items
}
