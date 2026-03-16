const CORE_SRC: &str = include_str!("../../stdlib/src/core/mod.dr");
pub const SOURCE_BUNDLE_MARKER: &str = "//!__daram_bundle_v1";
const SOURCE_BUNDLE_FILE_MARKER: &str = "//!__daram_file:";
const SOURCE_BUNDLE_ESCAPED_LINE_MARKER: &str = "//!__daram_line:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibStability {
    Stable,
    Unstable,
}

const STABLE_MODULES: &[&str] = &["core", "io", "test", "http", "fmt"];
const UNSTABLE_MODULES: &[&str] = &["collections", "fs", "json", "time", "net", "crypto", "task"];

pub fn stable_stdlib_modules() -> &'static [&'static str] {
    STABLE_MODULES
}

pub fn unstable_stdlib_modules() -> &'static [&'static str] {
    UNSTABLE_MODULES
}

pub fn stdlib_stability_for_path(path: &str) -> Option<StdlibStability> {
    let normalized = path.strip_prefix("std::").unwrap_or(path);
    let module = normalized.split("::").next()?;
    if STABLE_MODULES.contains(&module) {
        Some(StdlibStability::Stable)
    } else if UNSTABLE_MODULES.contains(&module) {
        Some(StdlibStability::Unstable)
    } else {
        None
    }
}

pub fn is_unstable_std_path(path: &str) -> bool {
    matches!(
        stdlib_stability_for_path(path),
        Some(StdlibStability::Unstable)
    )
}

fn extract_between(source: &str, start: &str, end: &str) -> String {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing stdlib section starting with `{start}`"));
    let tail = &source[start_index..];
    let end_index = tail.find(end).unwrap_or(tail.len());
    tail[..end_index].trim().to_string()
}

fn extract_option_source() -> String {
    extract_between(CORE_SRC, "export enum Option<T>", "extend<T> Option<T> {")
}

fn extract_result_source() -> String {
    extract_between(
        CORE_SRC,
        "export enum Result<T, E>",
        "extend<T, E> Result<T, E> {",
    )
}

fn extract_display_source() -> String {
    extract_between(
        CORE_SRC,
        "export ability Clone {",
        "// ─── Conversion abilities",
    )
}

fn extract_panic_source() -> String {
    let _ = CORE_SRC;
    r#"export fun panic(msg: &str): ! {
    __builtin_panic(msg)
}"#
    .to_string()
}

fn iterator_subset_source() -> &'static str {
    r#"
export ability Iterator {
    type Item;

    fun next(mut self): Option<Self::Item>;
}

export struct Map<T>;

extend<T> Map<T> {
    export fun next(mut self): Option<T> {
        __builtin_iter_map_next(self)
    }
}

extend<T> Map<T> implements Copy {}

export struct Filter<T>;

extend<T> Filter<T> {
    export fun next(mut self): Option<T> {
        __builtin_iter_filter_next(self)
    }
}

extend<T> Filter<T> implements Copy {}
"#
}

fn string_subset_source() -> &'static str {
    r#"
export struct String;

export struct StringSplit;

extend std::core::String {
    export fun new(): std::core::String {
        __builtin_string_new()
    }

    export fun len(self): usize {
        __builtin_string_len(self)
    }

    export fun is_empty(self): bool {
        __builtin_string_len(self) == 0
    }

    export fun as_str(self): &str {
        __builtin_string_as_str(self)
    }

    export fun push(mut self, c: char) {
        __builtin_string_push(self, c)
    }

    export fun push_str(mut self, s: &str) {
        __builtin_string_push_str(self, s)
    }

    export fun contains(self, needle: &str): bool {
        __builtin_string_contains(self, needle)
    }

    export fun split(self, delimiter: &str): std::core::StringSplit {
        __builtin_string_split(self, delimiter)
    }

    export fun trim(self): &str {
        __builtin_string_trim(self)
    }

    export fun repeat(self, times: i32): std::core::String {
        __builtin_string_repeat(self, times)
    }

    export fun replace(self, needle: &str, replacement: &str): std::core::String {
        __builtin_string_replace(self, needle, replacement)
    }
}

extend std::core::String implements Copy {}

extend std::core::StringSplit implements Copy {}

extend std::core::StringSplit {
    export fun next(mut self): Option<std::core::String> {
        __builtin_string_split_next(self)
    }

    export fun count(self): usize {
        __builtin_string_split_count(self)
    }
}
"#
}

fn collections_subset_source() -> &'static str {
    r#"
    import { Option, Copy, Iterator, Map, Filter } from "std/core";

    /// unstable: runtime coverage is intentionally partial in v1.0
    export struct Vec<T>;

    export struct VecIter<T>;

    extend<T> Vec<T> {
        export fun new(): Vec<T> {
            __builtin_vec_new()
        }

        export fun with_capacity(_cap: usize): Vec<T> {
            __builtin_vec_new()
        }

        export fun len(self): usize {
            __builtin_vec_len(self)
        }

        export fun is_empty(self): bool {
            __builtin_vec_len(self) == 0
        }

        export fun push(mut self, value: T) {
            __builtin_vec_push(self, value)
        }

        export fun pop(mut self): Option<T> {
            __builtin_vec_pop(self)
        }

        export fun get(self, index: usize): Option<&T> {
            __builtin_vec_get(self, index)
        }

        export fun iter(self): std::collections::VecIter<T> {
            __builtin_vec_iter(self)
        }
    }

    extend<T> Vec<T> implements Copy {}

    extend<T> VecIter<T> implements Copy {}

    extend<T> VecIter<T> {
        export fun next(mut self): Option<&T> {
            __builtin_vec_iter_next(self)
        }

        export fun count(self): usize {
            __builtin_vec_iter_count(self)
        }

        export fun map<B>(self, f: fun(&T): B): std::core::Map<B> {
            __builtin_iter_map(self, f)
        }

        export fun filter(self, pred: fun(&T): bool): std::core::Filter<&T> {
            __builtin_iter_filter(self, pred)
        }

        export fun collect_vec(mut self): std::collections::Vec<&T> {
            __builtin_iter_collect_vec(self)
        }
    }

    extend<T> Map<T> {
        export fun collect_vec(mut self): std::collections::Vec<T> {
            __builtin_iter_collect_vec(self)
        }
    }

    extend<T> Filter<T> {
        export fun map<B>(self, f: fun(T): B): std::core::Map<B> {
            __builtin_iter_map(self, f)
        }

        export fun collect_vec(mut self): std::collections::Vec<T> {
            __builtin_iter_collect_vec(self)
        }
    }

    /// unstable: runtime coverage is intentionally partial in v1.0
    export struct HashMap<K, V>;

    export struct HashMapIter<K, V>;

    extend<K, V> HashMap<K, V> {
        export fun new(): HashMap<K, V> {
            __builtin_hashmap_new()
        }

        export fun with_capacity(_cap: usize): HashMap<K, V> {
            __builtin_hashmap_new()
        }

        export fun len(self): usize {
            __builtin_hashmap_len(self)
        }

        export fun is_empty(self): bool {
            __builtin_hashmap_len(self) == 0
        }

        export fun insert(mut self, key: K, value: V): Option<V> {
            __builtin_hashmap_insert(self, key, value)
        }

        export fun get(self, key: K): Option<&V> {
            __builtin_hashmap_get(self, key)
        }

        export fun remove(mut self, key: K): Option<V> {
            __builtin_hashmap_remove(self, key)
        }

        export fun contains_key(self, key: K): bool {
            __builtin_hashmap_contains(self, key)
        }

        export fun iter(self): std::collections::HashMapIter<K, V> {
            __builtin_hashmap_iter(self)
        }
    }

    extend<K, V> HashMap<K, V> implements Copy {}

    extend<K, V> HashMapIter<K, V> implements Copy {}

    extend<K, V> HashMapIter<K, V> {
        export fun next(mut self): Option<(&K, &V)> {
            __builtin_hashmap_iter_next(self)
        }

        export fun count(self): usize {
            __builtin_hashmap_iter_count(self)
        }

        export fun map<B>(self, f: fun((&K, &V)): B): std::core::Map<B> {
            __builtin_iter_map(self, f)
        }

        export fun filter(self, pred: fun((&K, &V)): bool): std::core::Filter<(&K, &V)> {
            __builtin_iter_filter(self, pred)
        }

        export fun collect_vec(mut self): std::collections::Vec<(&K, &V)> {
            __builtin_iter_collect_vec(self)
        }
    }

    /// unstable: runtime coverage is intentionally partial in v1.0
    export struct HashSet<T>;
"#
}

fn fmt_subset_source() -> &'static str {
    r#"
    import { Result, String } from "std/core";

    export struct Error {}

    export struct Arguments;

    export struct Formatter(export std::core::String);

    extend Formatter {
        export fun new(): Formatter {
            Formatter(std::core::String::new())
        }

        export fun write_str(mut self, s: &str): Result<(), Error> {
            self.0.push_str(s);
            std::core::Result::Ok(())
        }

        export fun finish(self): std::core::String {
            self.0
        }
    }

    export fun format(fmt: &str): std::core::String;
"#
}

fn fs_subset_source() -> &'static str {
    r#"
    import { Result, String } from "std/core";
    import { Error } from "std/io";

    /// unstable: explicit capability token required for read operations
    export struct FsReadCap {}

    /// unstable: explicit capability token required for write operations
    export struct FsWriteCap {}

    /// unstable: runtime coverage is intentionally partial in v1.0
    export struct PathBuf;

    extend PathBuf {
        export fun new(): PathBuf {
            __builtin_pathbuf_new()
        }

        export fun from(s: &str): PathBuf {
            __builtin_pathbuf_from(s)
        }

        export fun as_str(self): &str {
            __builtin_pathbuf_as_str(self)
        }

        export fun exists(self): bool {
            __builtin_fs_exists(self)
        }

        export fun file_name(self): std::core::Option<&str> {
            __builtin_pathbuf_file_name(self)
        }

        export fun extension(self): std::core::Option<&str> {
            __builtin_pathbuf_extension(self)
        }

        export fun parent(self): std::core::Option<PathBuf> {
            __builtin_pathbuf_parent(self)
        }

        export fun is_file(self): bool {
            __builtin_pathbuf_is_file(self)
        }

        export fun is_dir(self): bool {
            __builtin_pathbuf_is_dir(self)
        }
    }

    export fun read_to_string(path: &PathBuf): Result<String, Error> {
        __builtin_fs_read_to_string(path)
    }

    export fun write_str(path: &PathBuf, content: &str): Result<(), Error> {
        __builtin_fs_write_str(path, content)
    }

    export fun create_dir_all(path: &PathBuf): Result<(), Error> {
        __builtin_fs_create_dir_all(path)
    }

    export fun remove_file(path: &PathBuf): Result<(), Error> {
        __builtin_fs_remove_file(path)
    }

    export fun remove_dir(path: &PathBuf): Result<(), Error> {
        __builtin_fs_remove_dir(path)
    }

    export fun copy(src: &PathBuf, dst: &PathBuf): Result<u64, Error> {
        __builtin_fs_copy(src, dst)
    }
"#
}

fn json_subset_source() -> &'static str {
    r#"
    import { Result, Option, String } from "std/core";
    import { Vec, HashMap } from "std/collections";

    export struct ParseError {
        export message: String,
        export offset: usize,
    }

    /// unstable: runtime coverage is intentionally partial in v1.0
    export enum Value {
        Null,
        Bool(bool),
        Number(f64),
        Str(String),
        Array(Vec<Value>),
        Object(HashMap<String, Value>),
    }

    extend Value {
        export fun is_null(self): bool {
            match self {
                Value::Null => true,
                _ => false,
            }
        }

        export fun is_bool(self): bool {
            match self {
                Value::Bool(_) => true,
                _ => false,
            }
        }

        export fun is_number(self): bool {
            match self {
                Value::Number(_) => true,
                _ => false,
            }
        }

        export fun is_string(self): bool {
            match self {
                Value::Str(_) => true,
                _ => false,
            }
        }

        export fun is_array(self): bool {
            match self {
                Value::Array(_) => true,
                _ => false,
            }
        }

        export fun is_object(self): bool {
            match self {
                Value::Object(_) => true,
                _ => false,
            }
        }

        export fun get(self, key: &str): Option<&Value> {
            __builtin_json_get(self, key)
        }

        export fun index(self, i: usize): Option<&Value> {
            __builtin_json_index(self, i)
        }
    }

    export fun parse(input: &str): Result<Value, ParseError> {
        __builtin_json_parse(input)
    }

    export fun to_string(value: &Value): String {
        __builtin_json_stringify(value)
    }

    export fun null(): Value {
        Value::Null
    }

    export fun bool_val(value: bool): Value {
        Value::Bool(value)
    }

    export fun number(value: f64): Value {
        Value::Number(value)
    }
"#
}

fn time_subset_source() -> &'static str {
    r#"
    /// unstable: runtime coverage is intentionally partial in v1.0
    export struct Duration {
        _secs: u64,
        _nanos: u32,
    }

    export fun duration_from_secs(secs: u64): Duration {
        Duration { _secs: secs, _nanos: 0 as u32 }
    }

    export fun duration_zero(): Duration {
        Duration { _secs: 0 as u64, _nanos: 0 as u32 }
    }

    export fun duration_is_zero(value: Duration): bool {
        value._secs == 0 as u64 && value._nanos == 0 as u32
    }

    /// unstable: runtime-backed clocks are not bundled in v1.0
    export struct Instant {
        _nanos: u64,
    }

    extend Instant {
        export fun now(): Instant {
            __builtin_time_instant_now()
        }
    }

    /// unstable: wall-clock integration is not bundled in v1.0
    export struct SystemTime {
        _secs: i64,
        _nanos: u32,
    }

    extend SystemTime {
        export fun now(): SystemTime {
            __builtin_time_system_now()
        }
    }

    export const UNIX_EPOCH: SystemTime = SystemTime { _secs: 0 as i64, _nanos: 0 as u32 };

    export fun format_rfc3339(t: SystemTime): std::core::String {
        __builtin_time_format_rfc3339(t)
    }
"#
}

fn net_subset_source() -> &'static str {
    r#"
    /// unstable: capability-gated networking surface only
    import { Result, String } from "std/core";
    import { IoResult } from "std/io";

    export struct NetCap {
        _private: (),
    }

    extend NetCap implements std::core::Copy {}

    /// unstable: runtime socket integration is intentionally partial in v1.0
    export enum IpAddr {
        V4(u8, u8, u8, u8),
        V6,
    }

    /// unstable: runtime socket integration is intentionally partial in v1.0
    export struct SocketAddr {
        export addr: IpAddr,
        export port: u16,
    }

    extend SocketAddr {
        export fun from_str(s: &str): Result<SocketAddr, String> {
            __builtin_socket_addr_parse(s)
        }
    }

    export fun socket_addr_v4(a: u8, b: u8, c: u8, d: u8, port: u16): SocketAddr {
        SocketAddr {
            addr: IpAddr::V4(a, b, c, d),
            port: port,
        }
    }

    export struct TcpStream;
    export struct TcpListener;
    export struct UdpSocket;

    extend TcpStream {
        export fun connect(addr: SocketAddr, _cap: NetCap): IoResult<TcpStream> {
            __builtin_tcp_connect(addr)
        }

        export fun connect_timeout(addr: SocketAddr, timeout_ms: u64, _cap: NetCap): IoResult<TcpStream> {
            __builtin_tcp_connect_timeout(addr, timeout_ms)
        }

        export fun shutdown(self): IoResult<()> {
            __builtin_tcp_shutdown(self)
        }
    }

    extend TcpListener {
        export fun bind(addr: SocketAddr, _cap: NetCap): IoResult<TcpListener> {
            __builtin_tcp_bind(addr)
        }

        export fun accept(mut self): IoResult<(TcpStream, SocketAddr)> {
            __builtin_tcp_accept(self)
        }
    }

    extend UdpSocket {
        export fun bind(addr: SocketAddr, _cap: NetCap): IoResult<UdpSocket> {
            __builtin_udp_bind(addr)
        }

        export fun send_to(self, buf: &str, addr: SocketAddr): IoResult<usize> {
            __builtin_udp_send_to(self, buf, addr)
        }

        export fun recv_from(self, buf: &str): IoResult<(usize, SocketAddr)> {
            __builtin_udp_recv_from(self, buf)
        }

        export fun connect(self, addr: SocketAddr): IoResult<()> {
            __builtin_udp_connect(self, addr)
        }

        export fun send(self, buf: &str): IoResult<usize> {
            __builtin_udp_send(self, buf)
        }

        export fun recv(self, buf: &str): IoResult<usize> {
            __builtin_udp_recv(self, buf)
        }
    }
"#
}

fn http_subset_source() -> &'static str {
    r#"
    import { Result, Option, String } from "std/core";
    import { HashMap } from "std/collections";
    import { NetCap } from "std/net";

    export enum Method {
        Get,
        Post,
        Put,
        Patch,
        Delete,
        Head,
        Options,
        Trace,
        Connect,
        Custom(String),
    }

    extend Method {
        export fun as_str(self): &str {
            __builtin_http_method_as_str(self)
        }
    }

    fun string_from_str(s: &str): std::core::String {
        const text = std::core::String::new();
        text.push_str(s);
        text
    }

    export struct StatusCode(export u16);

    extend StatusCode {
        export fun is_success(self): bool { self.0 >= 200 && self.0 < 300 }
        export fun is_redirect(self): bool { self.0 >= 300 && self.0 < 400 }
        export fun is_client_error(self): bool { self.0 >= 400 && self.0 < 500 }
        export fun is_server_error(self): bool { self.0 >= 500 && self.0 < 600 }
        export fun as_u16(self): u16 { self.0 }
    }

    export const OK: StatusCode = StatusCode(200);
    export const CREATED: StatusCode = StatusCode(201);
    export const NO_CONTENT: StatusCode = StatusCode(204);
    export const BAD_REQUEST: StatusCode = StatusCode(400);
    export const NOT_FOUND: StatusCode = StatusCode(404);
    export const INTERNAL_SERVER_ERROR: StatusCode = StatusCode(500);

    export struct Headers {
        _map: HashMap<String, String>,
    }

    extend Headers {
        export fun new(): Headers {
            Headers { _map: std::collections::HashMap::new() }
        }

        export fun get(self, name: &str): Option<String> {
            __builtin_http_headers_get(self, name)
        }

        export fun set(mut self, name: &str, value: &str) {
            self._map.insert(string_from_str(name), string_from_str(value));
        }
    }

    export struct Request {
        export method: Method,
        export url: String,
        export headers: Headers,
        export body: String,
    }

    extend Request {
        export fun new(method: Method, url: &str): Request {
            Request {
                method,
                url: string_from_str(url),
                headers: Headers::new(),
                body: std::core::String::new(),
            }
        }

        export fun get(url: &str): Request { Request::new(Method::Get, url) }
        export fun post(url: &str): Request { Request::new(Method::Post, url) }

        export fun header(mut self, name: &str, value: &str): Request {
            self.headers.set(name, value);
            self
        }

        export fun body_str(mut self, s: &str): Request {
            self.body = string_from_str(s);
            self
        }

        export fun json_body(mut self, json: &str): Request {
            self.headers.set("content-type", "application/json");
            self.body = string_from_str(json);
            self
        }
    }

    export struct Response {
        export status: StatusCode,
        export headers: Headers,
        export body: String,
    }

    extend Response {
        export fun body_str(self): Result<String, String> {
            std::core::Result::Ok(self.body)
        }

        export fun body_json(self): Result<std::json::Value, String> {
            __builtin_http_body_json(self)
        }
    }

    export struct Client {
        _timeout_ms: u64,
        _follow_redirects: bool,
    }

    extend Client {
        export fun new(_cap: NetCap): Client {
            Client { _timeout_ms: 30_000 as u64, _follow_redirects: true }
        }

        export fun timeout_ms(mut self, ms: u64): Client {
            self._timeout_ms = ms;
            self
        }

        export fun follow_redirects(mut self, yes: bool): Client {
            self._follow_redirects = yes;
            self
        }

        export fun send(self, req: Request): Result<Response, String> {
            __builtin_http_send(self, req)
        }

        export fun get(self, url: &str): Result<Response, String> {
            self.send(Request::get(url))
        }

        export fun post(self, url: &str, body: &str): Result<Response, String> {
            self.send(Request::post(url).body_str(body))
        }
    }
"#
}

fn task_subset_source() -> &'static str {
    r#"
    export struct JoinHandle<T>;

    extend<T> JoinHandle<T> {
        export fun join(self): T {
            __builtin_task_join(self)
        }
    }

    export fun spawn<T>(job: fun(): T): std::task::JoinHandle<T> {
        __builtin_task_spawn(job)
    }

    export fun block_on<T>(job: fun(): T): T {
        __builtin_task_block_on(job)
    }

    export fun sleep_ms(ms: u64) {
        __builtin_task_sleep_ms(ms)
    }
"#
}

fn crypto_subset_source() -> &'static str {
    r#"
    /// unstable: capability-gated crypto surface only
    export struct CryptoCap {}

    /// unstable: bundled subset provides deterministic placeholder semantics only
    export fun constant_time_eq(left: u8, right: u8): bool {
        left == right
    }

    /// unstable: runtime CSPRNG is not bundled in v1.0
    export fun random_u64(_cap: CryptoCap): u64 {
        4 as u64
    }

    export struct Sha256;

    extend Sha256 {
        export fun new(): Sha256 {
            Sha256
        }
    }
"#
}

pub fn encode_source_bundle(files: &[(String, String)]) -> String {
    let mut rendered = String::from(SOURCE_BUNDLE_MARKER);
    for (path, source) in files {
        rendered.push('\n');
        rendered.push_str(SOURCE_BUNDLE_FILE_MARKER);
        rendered.push_str(path);
        rendered.push('\n');
        for line in source.trim_end_matches('\n').lines() {
            if let Some(escaped) = encode_bundle_source_line(line) {
                rendered.push_str(&escaped);
            } else {
                rendered.push_str(line);
            }
            rendered.push('\n');
        }
    }
    rendered
}

pub fn decode_source_bundle(source: &str) -> Option<Vec<(String, String)>> {
    let mut lines = source.lines();
    if lines.next()? != SOURCE_BUNDLE_MARKER {
        return None;
    }

    let mut files = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_source = String::new();

    for line in lines {
        if let Some(line) = decode_bundle_source_line(line) {
            current_source.push_str(&line);
            current_source.push('\n');
        } else if let Some(path) = line.strip_prefix(SOURCE_BUNDLE_FILE_MARKER) {
            if let Some(path) = current_path.replace(path.to_string()) {
                files.push((path, current_source.trim_end_matches('\n').to_string()));
                current_source.clear();
            }
        } else {
            current_source.push_str(line);
            current_source.push('\n');
        }
    }

    if let Some(path) = current_path {
        files.push((path, current_source.trim_end_matches('\n').to_string()));
    }

    Some(files)
}

pub fn merge_source_bundles(parts: &[String]) -> String {
    let mut files = Vec::new();
    for part in parts {
        if let Some(mut decoded) = decode_source_bundle(part) {
            files.append(&mut decoded);
        } else {
            files.push(("main.dr".to_string(), part.clone()));
        }
    }
    encode_source_bundle(&files)
}

fn encode_bundle_source_line(line: &str) -> Option<String> {
    if line.starts_with(SOURCE_BUNDLE_FILE_MARKER)
        || line.starts_with(SOURCE_BUNDLE_ESCAPED_LINE_MARKER)
    {
        Some(format!("{SOURCE_BUNDLE_ESCAPED_LINE_MARKER}{line}"))
    } else {
        None
    }
}

fn decode_bundle_source_line(line: &str) -> Option<String> {
    line.strip_prefix(SOURCE_BUNDLE_ESCAPED_LINE_MARKER)
        .map(str::to_string)
}

fn bundled_stdlib_files() -> Vec<(String, String)> {
    vec![
        (
            "std/core/mod.dr".to_string(),
            format!(
                r#"
{}

{}

{}

{}

{}

export fun todo(): ! {{
    panic("not yet implemented")
}}

{}

export ability Copy {{}}
"#,
                extract_option_source(),
                extract_result_source(),
                extract_panic_source(),
                iterator_subset_source(),
                string_subset_source(),
                extract_display_source(),
            ),
        ),
        (
            "std/io.dr".to_string(),
            r#"
import { Result, String } from "std/core";

export enum ErrorKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    AddrInUse,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    UnexpectedEof,
    Interrupted,
    Other,
}

export struct Error {
    export kind: ErrorKind,
    export message: String,
}

export type IoResult<T> = Result<T, Error>;

export interface Write {
    fun write(mut self, buf: &str): IoResult<usize>;
    fun flush(mut self): IoResult<()>;
}

export struct Stdout { _handle: i32 }
export struct Stderr { _handle: i32 }
export struct Stdin  { _handle: i32 }

export fun stdout(): Stdout { Stdout { _handle: 1 } }
export fun stderr(): Stderr { Stderr { _handle: 2 } }
export fun stdin(): Stdin  { Stdin  { _handle: 0 } }

extend Stdout implements Write {
    fun write(mut self, buf: &str): IoResult<usize> {
        __builtin_io_stdout_write_str(buf)
    }
    fun flush(mut self): IoResult<()> {
        __builtin_io_stdout_flush()
    }
}

extend Stderr implements Write {
    fun write(mut self, buf: &str): IoResult<usize> {
        __builtin_io_stderr_write_str(buf)
    }
    fun flush(mut self): IoResult<()> {
        __builtin_io_stderr_flush()
    }
}

export fun print(value: &str) {
    __builtin_print(value)
}

export fun println(value: &str) {
    __builtin_println(value)
}

export fun eprint(value: &str) {
    __builtin_eprint(value)
}

export fun eprintln(value: &str) {
    __builtin_eprintln(value)
}
"#
            .to_string(),
        ),
        ("std/fmt.dr".to_string(), fmt_subset_source().to_string()),
        (
            "std/test.dr".to_string(),
            r#"
export fun assert(cond: bool) {
    __builtin_assert(cond)
}

export fun assert_eq(left: i32, right: i32) {
    __builtin_assert_eq(left, right)
}

export fun panic_with_fmt(msg: &str, left: i32, right: i32) {
    __builtin_panic_with_fmt(msg, left, right)
}
"#
            .to_string(),
        ),
        (
            "std/collections.dr".to_string(),
            collections_subset_source().to_string(),
        ),
        ("std/fs.dr".to_string(), fs_subset_source().to_string()),
        ("std/json.dr".to_string(), json_subset_source().to_string()),
        ("std/time.dr".to_string(), time_subset_source().to_string()),
        ("std/net.dr".to_string(), net_subset_source().to_string()),
        ("std/task.dr".to_string(), task_subset_source().to_string()),
        (
            "std/crypto.dr".to_string(),
            crypto_subset_source().to_string(),
        ),
        (
            "std/http/mod.dr".to_string(),
            http_subset_source().to_string(),
        ),
    ]
}

pub fn bundled_stdlib_source() -> String {
    encode_source_bundle(&bundled_stdlib_files())
}

pub fn with_bundled_prelude(source: &str) -> String {
    merge_source_bundles(&[bundled_stdlib_source(), source.to_string()])
}

#[cfg(test)]
mod tests {
    use super::{
        bundled_stdlib_source, decode_source_bundle, encode_source_bundle, is_unstable_std_path,
        stable_stdlib_modules, stdlib_stability_for_path, unstable_stdlib_modules, StdlibStability,
    };

    #[test]
    fn classifies_stdlib_paths_by_stability() {
        assert_eq!(
            stdlib_stability_for_path("std::core::Option"),
            Some(StdlibStability::Stable)
        );
        assert_eq!(
            stdlib_stability_for_path("std::collections::Vec"),
            Some(StdlibStability::Unstable)
        );
        assert_eq!(stdlib_stability_for_path("std::unknown::Thing"), None);
        assert!(is_unstable_std_path("std::fs::read_to_string"));
        assert!(!is_unstable_std_path("std::io::println"));
    }

    #[test]
    fn bundled_stdlib_marks_unstable_modules_in_source() {
        let bundled = bundled_stdlib_source();
        let files = decode_source_bundle(&bundled).expect("expected bundled stdlib files");
        assert!(files.iter().any(|(path, _)| path == "std/collections.dr"));
        assert!(files.iter().any(|(path, _)| path == "std/crypto.dr"));
        assert!(files.iter().any(|(path, _)| path == "std/fmt.dr"));
        assert!(files
            .iter()
            .any(|(_, source)| source.contains("export struct Vec<T>;")));
        assert!(files
            .iter()
            .any(|(_, source)| source.contains("export struct CryptoCap {}")));
        assert_eq!(
            stable_stdlib_modules(),
            &["core", "io", "test", "http", "fmt"]
        );
        assert_eq!(
            unstable_stdlib_modules(),
            &["collections", "fs", "json", "time", "net", "crypto", "task"]
        );
    }

    #[test]
    fn bundle_round_trips_embedded_file_markers_in_source() {
        let bundled = encode_source_bundle(&[(
            "main.dr".to_string(),
            "fun main(): i32 {\n    injected::answer()\n}\n\n//!__daram_file:injected.dr\n//!__daram_line://!__daram_file:still-comment\nexport fun answer(): i32 {\n    42\n}\n"
                .to_string(),
        )]);

        let files = decode_source_bundle(&bundled).expect("expected source bundle");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "main.dr");
        assert!(files[0].1.contains("//!__daram_file:injected.dr"));
        assert!(files[0]
            .1
            .contains("//!__daram_line://!__daram_file:still-comment"));
    }
}
