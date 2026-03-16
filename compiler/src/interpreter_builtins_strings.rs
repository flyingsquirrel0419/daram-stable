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
        "string_new" => Value::HeapString(Rc::new(RefCell::new(String::new()))),
        "string_len" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("string_len expects one argument"));
            }
            match &args[0] {
                Value::HeapString(value) => Value::Uint(value.borrow().len() as u128),
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_len expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_push" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("string_push expects two arguments"));
            }
            match (&args[0], &args[1]) {
                (Value::HeapString(value), Value::Char(ch)) => {
                    value.borrow_mut().push(*ch);
                    Value::Unit
                }
                (Value::HeapString(_), other) => {
                    return Err(RuntimeError::new(format!(
                        "string_push expects char argument, got `{}`",
                        other.render()
                    )));
                }
                (other, _) => {
                    return Err(RuntimeError::new(format!(
                        "string_push expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_push_str" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("string_push_str expects two arguments"));
            }
            match &args[0] {
                Value::HeapString(value) => {
                    let suffix = string_arg(locals, &args[1])?;
                    value.borrow_mut().push_str(&suffix);
                    Value::Unit
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_push_str expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_contains" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("string_contains expects two arguments"));
            }
            match &args[0] {
                Value::HeapString(value) => {
                    let needle = string_arg(locals, &args[1])?;
                    Value::Bool(value.borrow().contains(needle.as_str()))
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_contains expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_as_str" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("string_as_str expects one argument"));
            }
            match &args[0] {
                Value::HeapString(value) => Value::Str(value.borrow().clone()),
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_as_str expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_split" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("string_split expects two arguments"));
            }
            match &args[0] {
                Value::HeapString(value) => {
                    let delimiter = string_arg(locals, &args[1])?;
                    let parts = split_string_parts(&value.borrow(), &delimiter);
                    Value::StringSplit {
                        parts: Rc::new(parts),
                        index: Rc::new(RefCell::new(0)),
                    }
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_split expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_split_next" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("string_split_next expects one argument"));
            }
            return match &args[0] {
                Value::StringSplit { parts, index } => {
                    let mut cursor = index.borrow_mut();
                    let value = parts.get(*cursor).cloned();
                    if value.is_some() {
                        *cursor += 1;
                    }
                    option_value(
                        def_names,
                        value.map(|item| Value::HeapString(Rc::new(RefCell::new(item)))),
                    )
                    .map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "string_split_next expects StringSplit receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "string_split_count" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("string_split_count expects one argument"));
            }
            match &args[0] {
                Value::StringSplit { parts, index } => {
                    let cursor = *index.borrow();
                    Value::Uint(parts.len().saturating_sub(cursor) as u128)
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_split_count expects StringSplit receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_trim" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("string_trim expects one argument"));
            }
            match &args[0] {
                Value::HeapString(value) => Value::Str(value.borrow().trim().to_string()),
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_trim expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_repeat" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("string_repeat expects two arguments"));
            }
            match &args[0] {
                Value::HeapString(value) => {
                    let count = scalar_to_usize(&materialize_value(locals, &args[1])?)?;
                    Value::HeapString(Rc::new(RefCell::new(value.borrow().repeat(count))))
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_repeat expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "string_replace" => {
            if args.len() != 3 {
                return Err(RuntimeError::new("string_replace expects three arguments"));
            }
            match &args[0] {
                Value::HeapString(value) => {
                    let from = string_arg(locals, &args[1])?;
                    let to = string_arg(locals, &args[2])?;
                    Value::HeapString(Rc::new(RefCell::new(
                        value.borrow().replace(from.as_str(), to.as_str()),
                    )))
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "string_replace expects String receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "pathbuf_new" => Value::HeapString(Rc::new(RefCell::new(String::new()))),
        "pathbuf_from" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_from expects one argument"));
            }
            Value::HeapString(Rc::new(RefCell::new(string_arg(locals, &args[0])?)))
        }
        "pathbuf_as_str" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_as_str expects one argument"));
            }
            Value::Str(string_arg(locals, &args[0])?)
        }
        "pathbuf_file_name" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_file_name expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            let value = std::path::Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| Value::Str(s.to_string()));
            return option_value(def_names, value).map(Some);
        }
        "pathbuf_extension" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_extension expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            let value = std::path::Path::new(&path)
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| Value::Str(s.to_string()));
            return option_value(def_names, value).map(Some);
        }
        "pathbuf_parent" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_parent expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            let value = std::path::Path::new(&path)
                .parent()
                .and_then(|p| p.to_str())
                .map(|s| Value::HeapString(Rc::new(RefCell::new(s.to_string()))));
            return option_value(def_names, value).map(Some);
        }
        "pathbuf_is_file" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_is_file expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            Value::Bool(std::path::Path::new(&path).is_file())
        }
        "pathbuf_is_dir" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("pathbuf_is_dir expects one argument"));
            }
            let path = string_arg(locals, &args[0])?;
            Value::Bool(std::path::Path::new(&path).is_dir())
        }
        _ => return Ok(None),
    };

    Ok(Some(value))
}
