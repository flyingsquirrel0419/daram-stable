use super::*;

pub(super) fn execute(
    _mir: &MirModule,
    name: &str,
    def_names: &HashMap<DefId, String>,
    _functions: &HashMap<DefId, &crate::mir::MirFn>,
    _state: &mut ExecutionState,
    locals: &[Value],
    args: &[Value],
) -> Result<Option<Value>, RuntimeError> {
    let value = match name {
        "fs_exists" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("fs_exists expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            Value::Bool(std::path::Path::new(&path).exists())
        }
        "fs_read_to_string" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("fs_read_to_string expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            return match std::fs::read_to_string(&path) {
                Ok(contents) => result_ok(
                    def_names,
                    Value::HeapString(Rc::new(RefCell::new(contents))),
                )
                .map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "fs_write_str" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("fs_write_str expects two arguments"));
            }
            let path = string_arg(locals, &args[0])?;
            let contents = string_arg(locals, &args[1])?;
            return match std::fs::write(&path, contents) {
                Ok(()) => result_ok(def_names, Value::Unit).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "fs_create_dir_all" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("fs_create_dir_all expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            return match std::fs::create_dir_all(&path) {
                Ok(()) => result_ok(def_names, Value::Unit).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "fs_remove_file" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("fs_remove_file expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            return match std::fs::remove_file(&path) {
                Ok(()) => result_ok(def_names, Value::Unit).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "fs_remove_dir" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("fs_remove_dir expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            return match std::fs::remove_dir(&path) {
                Ok(()) => result_ok(def_names, Value::Unit).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "fs_copy" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("fs_copy expects two arguments"));
            }
            let src = string_arg(locals, &args[0])?;
            let dst = string_arg(locals, &args[1])?;
            return match std::fs::copy(&src, &dst) {
                Ok(bytes) => result_ok(def_names, Value::Uint(bytes as u128)).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "io_read_line" => {
            if !args.is_empty() {
                return Err(RuntimeError::new("io_read_line expects no arguments"));
            }
            let mut buf = String::new();
            return match std::io::stdin().read_line(&mut buf) {
                Ok(_) => {
                    result_ok(def_names, Value::HeapString(Rc::new(RefCell::new(buf)))).map(Some)
                }
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "io_stdout_write_str" => {
            if args.len() != 1 {
                return Err(RuntimeError::new(
                    "io_stdout_write_str expects one argument",
                ));
            }
            let s = string_arg(locals, &args[0])?;
            let len = s.len();
            std::io::stdout()
                .write_all(s.as_bytes())
                .map_err(|e| RuntimeError::new(e.to_string()))?;
            return result_ok(def_names, Value::Uint(len as u128)).map(Some);
        }
        "io_stderr_write_str" => {
            if args.len() != 1 {
                return Err(RuntimeError::new(
                    "io_stderr_write_str expects one argument",
                ));
            }
            let s = string_arg(locals, &args[0])?;
            let len = s.len();
            std::io::stderr()
                .write_all(s.as_bytes())
                .map_err(|e| RuntimeError::new(e.to_string()))?;
            return result_ok(def_names, Value::Uint(len as u128)).map(Some);
        }
        "io_stdout_flush" => {
            std::io::stdout()
                .flush()
                .map_err(|e| RuntimeError::new(e.to_string()))?;
            return result_ok(def_names, Value::Unit).map(Some);
        }
        "io_stderr_flush" => {
            std::io::stderr()
                .flush()
                .map_err(|e| RuntimeError::new(e.to_string()))?;
            return result_ok(def_names, Value::Unit).map(Some);
        }
        "io_stdin_read_to_string" => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| RuntimeError::new(e.to_string()))?;
            return result_ok(def_names, Value::HeapString(Rc::new(RefCell::new(s)))).map(Some);
        }
        "time_instant_now" => {
            use std::sync::OnceLock;
            static PROCESS_START: OnceLock<std::time::Instant> = OnceLock::new();
            let start = PROCESS_START.get_or_init(std::time::Instant::now);
            let nanos = start.elapsed().as_nanos() as u64;
            let instant_def = find_def(def_names, &["std::time::Instant", "Instant"])
                .ok_or_else(|| RuntimeError::new("cannot find std::time::Instant definition"))?;
            Value::Struct {
                def: instant_def,
                fields: vec![Value::Uint(nanos as u128)],
            }
        }
        "time_system_now" => {
            use std::time::{SystemTime, UNIX_EPOCH};
            let dur = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let system_time_def = find_def(def_names, &["std::time::SystemTime", "SystemTime"])
                .ok_or_else(|| RuntimeError::new("cannot find std::time::SystemTime definition"))?;
            Value::Struct {
                def: system_time_def,
                fields: vec![
                    Value::Int(dur.as_secs() as i128),
                    Value::Uint(dur.subsec_nanos() as u128),
                ],
            }
        }
        "time_format_rfc3339" => {
            if args.len() != 1 {
                return Err(RuntimeError::new(
                    "time_format_rfc3339 expects one argument",
                ));
            }
            let val = materialize_value(locals, &args[0])?;
            let (secs, nanos) = match &val {
                Value::Struct { fields, .. } if fields.len() >= 2 => {
                    let secs = match &fields[0] {
                        Value::Int(n) => *n as i64,
                        _ => 0,
                    };
                    let nanos = match &fields[1] {
                        Value::Uint(n) => *n as u32,
                        _ => 0,
                    };
                    (secs, nanos)
                }
                _ => (0, 0),
            };
            let (y, mo, d, h, mi, s) = unix_to_calendar(secs);
            let formatted = if nanos == 0 {
                format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
            } else {
                format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{nanos:09}Z")
            };
            Value::HeapString(Rc::new(RefCell::new(formatted)))
        }
        _ => return Ok(None),
    };

    Ok(Some(value))
}
