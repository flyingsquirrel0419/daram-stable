use super::*;

pub(super) fn execute(
    mir: &MirModule,
    name: &str,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    locals: &[Value],
    args: &[Value],
) -> Result<Option<Value>, RuntimeError> {
    let value = match name {
        "vec_new" => Value::Vec(Rc::new(RefCell::new(Vec::new()))),
        "vec_push" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("vec_push expects two arguments"));
            }
            match &args[0] {
                Value::Vec(values) => {
                    values.borrow_mut().push(args[1].clone());
                    Value::Unit
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "vec_push expects Vec receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "vec_pop" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("vec_pop expects one argument"));
            }
            return match &args[0] {
                Value::Vec(values) => option_value(def_names, values.borrow_mut().pop()).map(Some),
                other => Err(RuntimeError::new(format!(
                    "vec_pop expects Vec receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "vec_len" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("vec_len expects one argument"));
            }
            match &args[0] {
                Value::Vec(values) => Value::Uint(values.borrow().len() as u128),
                other => {
                    return Err(RuntimeError::new(format!(
                        "vec_len expects Vec receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "vec_get" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("vec_get expects two arguments"));
            }
            let index = scalar_to_usize(&materialize_value(locals, &args[1])?)?;
            return match &args[0] {
                Value::Vec(values) => {
                    let value = values
                        .borrow()
                        .get(index)
                        .cloned()
                        .map(|value| Value::ConstRef(Box::new(value)));
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "vec_get expects Vec receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "vec_iter" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("vec_iter expects one argument"));
            }
            match &args[0] {
                Value::Vec(values) => Value::VecIter {
                    values: Rc::clone(values),
                    index: Rc::new(RefCell::new(0)),
                },
                other => {
                    return Err(RuntimeError::new(format!(
                        "vec_iter expects Vec receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "vec_iter_next" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("vec_iter_next expects one argument"));
            }
            return match &args[0] {
                Value::VecIter { values, index } => {
                    let current = *index.borrow();
                    *index.borrow_mut() = current.saturating_add(1);
                    let value = values
                        .borrow()
                        .get(current)
                        .cloned()
                        .map(|value| Value::ConstRef(Box::new(value)));
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "vec_iter_next expects VecIter receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "vec_iter_count" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("vec_iter_count expects one argument"));
            }
            match &args[0] {
                Value::VecIter { values, index } => {
                    let current = *index.borrow();
                    let len = values.borrow().len();
                    Value::Uint(len.saturating_sub(current) as u128)
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "vec_iter_count expects VecIter receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "iter_map" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("iter_map expects two arguments"));
            }
            Value::MapIter {
                iter: Rc::new(RefCell::new(args[0].clone())),
                mapper: Box::new(capture_call_arg(locals, args[1].clone())?),
            }
        }
        "iter_map_next" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("iter_map_next expects one argument"));
            }
            return match args[0].clone() {
                mut iter @ Value::MapIter { .. } => {
                    let value = runtime_iter_next(mir, def_names, functions, state, &mut iter)?;
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "iter_map_next expects Map receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "iter_filter" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("iter_filter expects two arguments"));
            }
            Value::FilterIter {
                iter: Rc::new(RefCell::new(args[0].clone())),
                predicate: Box::new(capture_call_arg(locals, args[1].clone())?),
            }
        }
        "iter_filter_next" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("iter_filter_next expects one argument"));
            }
            return match args[0].clone() {
                mut iter @ Value::FilterIter { .. } => {
                    let value = runtime_iter_next(mir, def_names, functions, state, &mut iter)?;
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "iter_filter_next expects Filter receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "iter_collect_vec" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("iter_collect_vec expects one argument"));
            }
            let mut iter = args[0].clone();
            let mut values = Vec::new();
            loop {
                let next = runtime_iter_next(mir, def_names, functions, state, &mut iter)?;
                let Some(item) = next else {
                    break;
                };
                values.push(item);
            }
            Value::Vec(Rc::new(RefCell::new(values)))
        }
        "hashmap_new" => Value::HashMap(Rc::new(RefCell::new(std::collections::HashMap::new()))),
        "hashmap_insert" => {
            if args.len() != 3 {
                return Err(RuntimeError::new("hashmap_insert expects three arguments"));
            }
            return match &args[0] {
                Value::HashMap(values) => {
                    let key = hashable_arg(locals, &args[1])?;
                    let prev = values
                        .borrow_mut()
                        .insert(key, materialize_value(locals, &args[2])?);
                    option_value(def_names, prev).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "hashmap_insert expects HashMap receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "hashmap_get" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("hashmap_get expects two arguments"));
            }
            return match &args[0] {
                Value::HashMap(values) => {
                    let key = hashable_arg(locals, &args[1])?;
                    let value = values
                        .borrow()
                        .get(&key)
                        .cloned()
                        .map(|value| Value::ConstRef(Box::new(value)));
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "hashmap_get expects HashMap receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "hashmap_remove" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("hashmap_remove expects two arguments"));
            }
            return match &args[0] {
                Value::HashMap(values) => {
                    let key = hashable_arg(locals, &args[1])?;
                    let removed = values.borrow_mut().remove(&key);
                    option_value(def_names, removed).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "hashmap_remove expects HashMap receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "hashmap_contains" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("hashmap_contains expects two arguments"));
            }
            match &args[0] {
                Value::HashMap(values) => {
                    let key = hashable_arg(locals, &args[1])?;
                    Value::Bool(values.borrow().contains_key(&key))
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "hashmap_contains expects HashMap receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "hashmap_len" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("hashmap_len expects one argument"));
            }
            match &args[0] {
                Value::HashMap(values) => Value::Uint(values.borrow().len() as u128),
                other => {
                    return Err(RuntimeError::new(format!(
                        "hashmap_len expects HashMap receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "hashmap_iter_get" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("hashmap_iter_get expects two arguments"));
            }
            let index = scalar_to_usize(&materialize_value(locals, &args[1])?)?;
            return match &args[0] {
                Value::HashMap(values) => {
                    let value = values.borrow().iter().nth(index).map(|(key, value)| {
                        Value::Tuple(vec![
                            Value::ConstRef(Box::new(key.to_value())),
                            Value::ConstRef(Box::new(value.clone())),
                        ])
                    });
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "hashmap_iter_get expects HashMap receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "hashmap_iter" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("hashmap_iter expects one argument"));
            }
            match &args[0] {
                Value::HashMap(entries) => Value::HashMapIter {
                    entries: Rc::clone(entries),
                    index: Rc::new(RefCell::new(0)),
                },
                other => {
                    return Err(RuntimeError::new(format!(
                        "hashmap_iter expects HashMap receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        "hashmap_iter_next" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("hashmap_iter_next expects one argument"));
            }
            return match &args[0] {
                Value::HashMapIter { entries, index } => {
                    let current = *index.borrow();
                    *index.borrow_mut() = current.saturating_add(1);
                    let value = entries.borrow().iter().nth(current).map(|(key, value)| {
                        Value::Tuple(vec![
                            Value::ConstRef(Box::new(key.to_value())),
                            Value::ConstRef(Box::new(value.clone())),
                        ])
                    });
                    option_value(def_names, value).map(Some)
                }
                other => Err(RuntimeError::new(format!(
                    "hashmap_iter_next expects HashMapIter receiver, got `{}`",
                    other.render()
                ))),
            };
        }
        "hashmap_iter_count" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("hashmap_iter_count expects one argument"));
            }
            match &args[0] {
                Value::HashMapIter { entries, index } => {
                    let current = *index.borrow();
                    let len = entries.borrow().len();
                    Value::Uint(len.saturating_sub(current) as u128)
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "hashmap_iter_count expects HashMapIter receiver, got `{}`",
                        other.render()
                    )));
                }
            }
        }
        _ => return Ok(None),
    };

    Ok(Some(value))
}
