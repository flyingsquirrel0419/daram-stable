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
        "socket_addr_parse" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("socket_addr_parse expects one argument"));
            }
            return match string_arg(locals, &args[0])?.parse::<std::net::SocketAddr>() {
                Ok(addr) => result_ok(def_names, socket_addr_value(def_names, addr)?).map(Some),
                Err(error) => result_err(
                    def_names,
                    Value::HeapString(Rc::new(RefCell::new(error.to_string()))),
                )
                .map(Some),
            };
        }
        "tcp_connect" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("tcp_connect expects one argument"));
            }
            let addr = runtime_socket_addr(mir, locals, &args[0])?;
            return match std::net::TcpStream::connect(addr) {
                Ok(stream) => {
                    result_ok(def_names, Value::TcpStream(Rc::new(RefCell::new(stream)))).map(Some)
                }
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "tcp_connect_timeout" => {
            if args.len() != 2 {
                return Err(RuntimeError::new(
                    "tcp_connect_timeout expects two arguments",
                ));
            }
            let addr = runtime_socket_addr(mir, locals, &args[0])?;
            let timeout_ms = scalar_to_usize(&materialize_value(locals, &args[1])?)?;
            return match std::net::TcpStream::connect_timeout(
                &addr,
                Duration::from_millis(timeout_ms as u64),
            ) {
                Ok(stream) => {
                    result_ok(def_names, Value::TcpStream(Rc::new(RefCell::new(stream)))).map(Some)
                }
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "tcp_bind" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("tcp_bind expects one argument"));
            }
            let addr = runtime_socket_addr(mir, locals, &args[0])?;
            return match std::net::TcpListener::bind(addr) {
                Ok(listener) => result_ok(
                    def_names,
                    Value::TcpListener(Rc::new(RefCell::new(listener))),
                )
                .map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "tcp_accept" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("tcp_accept expects one argument"));
            }
            let listener = match materialize_value(locals, &args[0])? {
                Value::TcpListener(l) => l,
                _ => return Err(RuntimeError::new("tcp_accept expects TcpListener")),
            };
            return match listener.borrow_mut().accept() {
                Ok((stream, peer)) => {
                    let pair = Value::Tuple(vec![
                        Value::TcpStream(Rc::new(RefCell::new(stream))),
                        socket_addr_value(def_names, peer)?,
                    ]);
                    result_ok(def_names, pair).map(Some)
                }
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "tcp_read" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("tcp_read expects one argument"));
            }
            let stream = match materialize_value(locals, &args[0])? {
                Value::TcpStream(s) => s,
                _ => return Err(RuntimeError::new("tcp_read expects TcpStream")),
            };
            let mut buf = Vec::new();
            return match stream.borrow_mut().read_to_end(&mut buf) {
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf).into_owned();
                    result_ok(def_names, Value::HeapString(Rc::new(RefCell::new(s)))).map(Some)
                }
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "tcp_write" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("tcp_write expects two arguments"));
            }
            let stream = match materialize_value(locals, &args[0])? {
                Value::TcpStream(s) => s,
                _ => return Err(RuntimeError::new("tcp_write expects TcpStream")),
            };
            let data = string_arg(locals, &args[1])?;
            return match stream.borrow_mut().write_all(data.as_bytes()) {
                Ok(()) => result_ok(def_names, Value::Uint(data.len() as u128)).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "tcp_shutdown" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("tcp_shutdown expects one argument"));
            }
            let stream = match materialize_value(locals, &args[0])? {
                Value::TcpStream(s) => s,
                _ => return Err(RuntimeError::new("tcp_shutdown expects TcpStream")),
            };
            return match stream.borrow_mut().shutdown(std::net::Shutdown::Both) {
                Ok(()) => result_ok(def_names, Value::Unit).map(Some),
                Err(error) => {
                    result_err(def_names, io_error_value(def_names, error.to_string())?).map(Some)
                }
            };
        }
        "udp_bind" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("udp_bind expects one argument"));
            }
            let addr = runtime_socket_addr(mir, locals, &args[0])?;
            return match std::net::UdpSocket::bind(addr) {
                Ok(sock) => {
                    result_ok(def_names, Value::UdpSocket(Rc::new(RefCell::new(sock)))).map(Some)
                }
                Err(e) => {
                    result_err(def_names, io_error_value(def_names, e.to_string())?).map(Some)
                }
            };
        }
        "udp_send_to" => {
            if args.len() != 3 {
                return Err(RuntimeError::new("udp_send_to expects three arguments"));
            }
            let sock = match materialize_value(locals, &args[0])? {
                Value::UdpSocket(s) => s,
                _ => return Err(RuntimeError::new("udp_send_to expects UdpSocket")),
            };
            let data = string_arg(locals, &args[1])?;
            let addr = runtime_socket_addr(mir, locals, &args[2])?;
            return match sock.borrow().send_to(data.as_bytes(), addr) {
                Ok(n) => result_ok(def_names, Value::Uint(n as u128)).map(Some),
                Err(e) => {
                    result_err(def_names, io_error_value(def_names, e.to_string())?).map(Some)
                }
            };
        }
        "udp_recv_from" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("udp_recv_from expects one argument"));
            }
            let sock = match materialize_value(locals, &args[0])? {
                Value::UdpSocket(s) => s,
                _ => return Err(RuntimeError::new("udp_recv_from expects UdpSocket")),
            };
            let mut buf = vec![0u8; 65536];
            return match sock.borrow().recv_from(&mut buf) {
                Ok((n, peer)) => {
                    let data = String::from_utf8_lossy(&buf[..n]).into_owned();
                    result_ok(
                        def_names,
                        Value::Tuple(vec![
                            Value::HeapString(Rc::new(RefCell::new(data))),
                            socket_addr_value(def_names, peer)?,
                        ]),
                    )
                    .map(Some)
                }
                Err(e) => {
                    result_err(def_names, io_error_value(def_names, e.to_string())?).map(Some)
                }
            };
        }
        "udp_connect" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("udp_connect expects two arguments"));
            }
            let sock = match materialize_value(locals, &args[0])? {
                Value::UdpSocket(s) => s,
                _ => return Err(RuntimeError::new("udp_connect expects UdpSocket")),
            };
            let addr = runtime_socket_addr(mir, locals, &args[1])?;
            return match sock.borrow().connect(addr) {
                Ok(()) => result_ok(def_names, Value::Unit).map(Some),
                Err(e) => {
                    result_err(def_names, io_error_value(def_names, e.to_string())?).map(Some)
                }
            };
        }
        "udp_send" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("udp_send expects two arguments"));
            }
            let sock = match materialize_value(locals, &args[0])? {
                Value::UdpSocket(s) => s,
                _ => return Err(RuntimeError::new("udp_send expects UdpSocket")),
            };
            let data = string_arg(locals, &args[1])?;
            return match sock.borrow().send(data.as_bytes()) {
                Ok(n) => result_ok(def_names, Value::Uint(n as u128)).map(Some),
                Err(e) => {
                    result_err(def_names, io_error_value(def_names, e.to_string())?).map(Some)
                }
            };
        }
        "udp_recv" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("udp_recv expects one argument"));
            }
            let sock = match materialize_value(locals, &args[0])? {
                Value::UdpSocket(s) => s,
                _ => return Err(RuntimeError::new("udp_recv expects UdpSocket")),
            };
            let mut buf = vec![0u8; 65536];
            return match sock.borrow().recv(&mut buf) {
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).into_owned();
                    result_ok(def_names, Value::HeapString(Rc::new(RefCell::new(data)))).map(Some)
                }
                Err(e) => {
                    result_err(def_names, io_error_value(def_names, e.to_string())?).map(Some)
                }
            };
        }
        "json_parse" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("json_parse expects one argument"));
            }
            let input = string_arg(locals, &args[0])?;
            return match serde_json::from_str::<serde_json::Value>(&input) {
                Ok(value) => result_ok(def_names, json_runtime_value(def_names, value)?).map(Some),
                Err(error) => result_err(
                    def_names,
                    json_parse_error_value(def_names, error.to_string(), error.column() as u128)?,
                )
                .map(Some),
            };
        }
        "json_stringify" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("json_stringify expects one argument"));
            }
            let json = runtime_to_json(locals, &args[0])?;
            let rendered = serde_json::to_string(&json)
                .map_err(|error| RuntimeError::new(format!("json stringify failed: {error}")))?;
            Value::HeapString(Rc::new(RefCell::new(rendered)))
        }
        "json_get" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("json_get expects two arguments"));
            }
            return match materialize_value(locals, &args[0])? {
                Value::Enum {
                    variant_idx: 5,
                    fields,
                    ..
                } => match fields.as_slice() {
                    [Value::HashMap(entries)] => {
                        let key = HashableValue::String(string_arg(locals, &args[1])?);
                        let value = entries
                            .borrow()
                            .get(&key)
                            .cloned()
                            .map(|value| Value::ConstRef(Box::new(value)));
                        option_value(def_names, value).map(Some)
                    }
                    _ => Err(RuntimeError::new("json object payload is malformed")),
                },
                _ => option_value(def_names, None).map(Some),
            };
        }
        "json_index" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("json_index expects two arguments"));
            }
            let index = scalar_to_usize(&materialize_value(locals, &args[1])?)?;
            return match materialize_value(locals, &args[0])? {
                Value::Enum {
                    variant_idx: 4,
                    fields,
                    ..
                } => match fields.as_slice() {
                    [Value::Vec(values)] => {
                        let value = values
                            .borrow()
                            .get(index)
                            .cloned()
                            .map(|value| Value::ConstRef(Box::new(value)));
                        option_value(def_names, value).map(Some)
                    }
                    _ => Err(RuntimeError::new("json array payload is malformed")),
                },
                _ => option_value(def_names, None).map(Some),
            };
        }
        "http_send" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("http_send expects two arguments"));
            }
            return http_send_builtin(mir, def_names, locals, &args[0], &args[1]).map(Some);
        }
        "http_method_as_str" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("http_method_as_str expects one argument"));
            }
            Value::Str(http_method_string(mir, locals, &args[0])?)
        }
        "http_headers_get" => {
            if args.len() != 2 {
                return Err(RuntimeError::new("http_headers_get expects two arguments"));
            }
            return http_headers_get_builtin(mir, def_names, locals, &args[0], &args[1]).map(Some);
        }
        "http_body_json" => {
            if args.len() != 1 {
                return Err(RuntimeError::new("http_body_json expects one argument"));
            }
            return http_body_json_builtin(mir, def_names, locals, &args[0]).map(Some);
        }
        "task_spawn" => {
            return task_spawn_builtin(mir, def_names, functions, state, &args[0..]).map(Some)
        }
        "task_join" => {
            return task_join_builtin(mir, def_names, functions, state, &args[0..]).map(Some)
        }
        "task_block_on" => {
            return task_block_on_builtin(mir, def_names, functions, state, &args[0..]).map(Some)
        }
        "task_sleep_ms" => return task_sleep_ms_builtin(&args[0..]).map(Some),
        _ => return Ok(None),
    };

    Ok(Some(value))
}
