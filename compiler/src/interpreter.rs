use crate::{
    builtin_catalog,
    hir::{DefId, Ty},
    mir::{
        AggregateKind, MirBinOp, MirConst, MirModule, MirUnaryOp, Operand, Place, Projection,
        Rvalue, StatementKind, TerminatorKind,
    },
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::rc::Rc;
use std::thread;
use std::time::Duration;

#[path = "interpreter_builtins_collections.rs"]
mod builtins_collections;
#[path = "interpreter_builtins_network.rs"]
mod builtins_network;
#[path = "interpreter_builtins_strings.rs"]
mod builtins_strings;
#[path = "interpreter_builtins_system.rs"]
mod builtins_system;

#[derive(Debug, Clone)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i128),
    Uint(u128),
    Float(f64),
    Char(char),
    Str(String),
    HeapString(Rc<RefCell<String>>),
    StringSplit {
        parts: Rc<Vec<String>>,
        index: Rc<RefCell<usize>>,
    },
    Vec(Rc<RefCell<Vec<Value>>>),
    VecIter {
        values: Rc<RefCell<Vec<Value>>>,
        index: Rc<RefCell<usize>>,
    },
    MapIter {
        iter: Rc<RefCell<Value>>,
        mapper: Box<Value>,
    },
    FilterIter {
        iter: Rc<RefCell<Value>>,
        predicate: Box<Value>,
    },
    HashMap(Rc<RefCell<std::collections::HashMap<HashableValue, Value>>>),
    HashMapIter {
        entries: Rc<RefCell<std::collections::HashMap<HashableValue, Value>>>,
        index: Rc<RefCell<usize>>,
    },
    JoinHandle(Rc<RefCell<JoinHandleState>>),
    TcpStream(Rc<RefCell<std::net::TcpStream>>),
    TcpListener(Rc<RefCell<std::net::TcpListener>>),
    UdpSocket(Rc<RefCell<std::net::UdpSocket>>),
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    Struct {
        def: DefId,
        fields: Vec<Value>,
    },
    Enum {
        def: DefId,
        variant_idx: usize,
        fields: Vec<Value>,
    },
    Ref(Place),
    ConstRef(Box<Value>),
    Closure {
        def: DefId,
        captures: Vec<Value>,
    },
    Def(DefId),
    Undef,
}

#[derive(Debug, Clone)]
pub enum JoinHandleState {
    Pending(Box<Value>),
    Ready(Box<Value>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HashableValue {
    Unit,
    Bool(bool),
    Int(i128),
    Uint(u128),
    Char(char),
    String(String),
    Tuple(Vec<HashableValue>),
    Array(Vec<HashableValue>),
    Struct {
        def: DefId,
        fields: Vec<HashableValue>,
    },
    Enum {
        def: DefId,
        variant_idx: usize,
        fields: Vec<HashableValue>,
    },
}

impl Value {
    pub fn render(&self) -> String {
        match self {
            Value::Unit => "()".into(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Uint(value) => value.to_string(),
            Value::Float(value) => value.to_string(),
            Value::Char(value) => value.to_string(),
            Value::Str(value) => value.clone(),
            Value::HeapString(value) => value.borrow().clone(),
            Value::StringSplit { .. } => "<string-split>".into(),
            Value::Vec(values) => {
                let inner = values
                    .borrow()
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            Value::VecIter { .. } => "<vec-iter>".into(),
            Value::MapIter { .. } => "<map-iter>".into(),
            Value::FilterIter { .. } => "<filter-iter>".into(),
            Value::HashMap(entries) => {
                let inner = entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.render(), value.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{inner}}}")
            }
            Value::HashMapIter { .. } => "<hashmap-iter>".into(),
            Value::JoinHandle(_) => "<join-handle>".into(),
            Value::TcpStream(_) => "<tcp-stream>".into(),
            Value::TcpListener(_) => "<tcp-listener>".into(),
            Value::UdpSocket(_) => "<udp-socket>".into(),
            Value::Tuple(values) => {
                let inner = values
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({inner})")
            }
            Value::Array(values) => {
                let inner = values
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            Value::Struct { fields, .. } => {
                let inner = fields
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{inner}}}")
            }
            Value::Enum {
                variant_idx,
                fields,
                ..
            } => {
                if fields.is_empty() {
                    format!("<enum:{variant_idx}>")
                } else {
                    let inner = fields
                        .iter()
                        .map(Value::render)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("<enum:{variant_idx}({inner})>")
                }
            }
            Value::Ref(_) => "&<ref>".into(),
            Value::ConstRef(value) => format!("&{}", value.render()),
            Value::Closure { def, .. } => format!("<closure {}>", def.index),
            Value::Def(def) => format!("<def {}>", def.index),
            Value::Undef => "<undef>".into(),
        }
    }
}

impl HashableValue {
    fn render(&self) -> String {
        match self {
            HashableValue::Unit => "()".into(),
            HashableValue::Bool(value) => value.to_string(),
            HashableValue::Int(value) => value.to_string(),
            HashableValue::Uint(value) => value.to_string(),
            HashableValue::Char(value) => value.to_string(),
            HashableValue::String(value) => value.clone(),
            HashableValue::Tuple(values) => format!(
                "({})",
                values
                    .iter()
                    .map(HashableValue::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            HashableValue::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(HashableValue::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            HashableValue::Struct { def, fields } => format!(
                "<struct:{}:{}>",
                def.index,
                fields
                    .iter()
                    .map(HashableValue::render)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            HashableValue::Enum {
                def,
                variant_idx,
                fields,
            } => format!(
                "<enum:{}:{}:{}>",
                def.index,
                variant_idx,
                fields
                    .iter()
                    .map(HashableValue::render)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            HashableValue::Unit => Value::Unit,
            HashableValue::Bool(value) => Value::Bool(*value),
            HashableValue::Int(value) => Value::Int(*value),
            HashableValue::Uint(value) => Value::Uint(*value),
            HashableValue::Char(value) => Value::Char(*value),
            HashableValue::String(value) => Value::Str(value.clone()),
            HashableValue::Tuple(values) => {
                Value::Tuple(values.iter().map(HashableValue::to_value).collect())
            }
            HashableValue::Array(values) => {
                Value::Array(values.iter().map(HashableValue::to_value).collect())
            }
            HashableValue::Struct { def, fields } => Value::Struct {
                def: *def,
                fields: fields.iter().map(HashableValue::to_value).collect(),
            },
            HashableValue::Enum {
                def,
                variant_idx,
                fields,
            } => Value::Enum {
                def: *def,
                variant_idx: *variant_idx,
                fields: fields.iter().map(HashableValue::to_value).collect(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    MissingDefinition,
    InvalidArguments,
    TypeMismatch,
    Bounds,
    Io,
    Network,
    Json,
    AssertionFailed,
    Panic,
    LimitExceeded,
    Unsupported,
    Other,
}

#[derive(Debug, Clone)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
}

impl RuntimeError {
    fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            kind: Self::infer_kind(&message),
            message,
        }
    }

    fn with_kind(kind: RuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn missing_definition(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::MissingDefinition, message)
    }

    fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::InvalidArguments, message)
    }

    fn assertion_failed(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::AssertionFailed, message)
    }

    fn panic(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::Panic, message)
    }

    fn limit_exceeded(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::LimitExceeded, message)
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self::with_kind(RuntimeErrorKind::Unsupported, message)
    }

    fn infer_kind(message: &str) -> RuntimeErrorKind {
        if message.contains("step limit exceeded") || message.contains("call depth limit exceeded")
        {
            RuntimeErrorKind::LimitExceeded
        } else if message.contains("assertion failed") || message.contains("assert_eq failed") {
            RuntimeErrorKind::AssertionFailed
        } else if message.starts_with("panic")
            || message.contains("left=`") && message.contains("right=`")
        {
            RuntimeErrorKind::Panic
        } else if message.contains("cannot find") || message.contains("no MIR function") {
            RuntimeErrorKind::MissingDefinition
        } else if message.contains("expects")
            || message.contains("expected ")
            || message.contains("missing target")
        {
            RuntimeErrorKind::InvalidArguments
        } else if message.contains("out of bounds")
            || message.contains("invalid local")
            || message.contains("invalid block")
            || message.contains("invalid field index")
            || message.contains("index `")
        {
            RuntimeErrorKind::Bounds
        } else if message.contains("http")
            || message.contains("tcp")
            || message.contains("udp")
            || message.contains("socket")
        {
            RuntimeErrorKind::Network
        } else if message.contains("json") {
            RuntimeErrorKind::Json
        } else if message.contains("fs_")
            || message.contains("No such file")
            || message.contains("Permission denied")
            || message.contains("stdin")
            || message.contains("stdout")
            || message.contains("stderr")
        {
            RuntimeErrorKind::Io
        } else if message.contains("cannot cast")
            || message.contains("cannot call non-function")
            || message.contains("malformed")
            || message.contains("cannot get length")
            || message.contains("unknown http method")
        {
            RuntimeErrorKind::TypeMismatch
        } else if message.contains("unknown builtin") {
            RuntimeErrorKind::Unsupported
        } else {
            RuntimeErrorKind::Other
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionLimits {
    pub max_steps: u64,
    pub max_call_depth: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            max_call_depth: 1024,
        }
    }
}

struct ExecutionState {
    steps_remaining: u64,
    max_call_depth: usize,
    call_depth: usize,
}

impl ExecutionState {
    fn new(limits: ExecutionLimits) -> Self {
        Self {
            steps_remaining: limits.max_steps,
            max_call_depth: limits.max_call_depth,
            call_depth: 0,
        }
    }

    fn step(&mut self, what: &str) -> Result<(), RuntimeError> {
        if self.steps_remaining == 0 {
            return Err(RuntimeError::limit_exceeded(format!(
                "execution step limit exceeded while processing {what}"
            )));
        }
        self.steps_remaining -= 1;
        Ok(())
    }
}

pub fn execute_function(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    entry_name: &str,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    execute_function_with_limits(mir, def_names, entry_name, args, ExecutionLimits::default())
}

pub fn execute_function_with_limits(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    entry_name: &str,
    args: &[Value],
    limits: ExecutionLimits,
) -> Result<Value, RuntimeError> {
    let functions = mir
        .functions
        .iter()
        .map(|function| (function.def, function))
        .collect::<HashMap<_, _>>();

    let entry = def_names
        .iter()
        .find_map(|(def, name)| (name == entry_name).then_some(*def))
        .ok_or_else(|| {
            RuntimeError::missing_definition(format!("cannot find entry function `{entry_name}`"))
        })?;

    let mut state = ExecutionState::new(limits);
    execute_def(mir, def_names, &functions, &mut state, entry, args)
}

fn execute_def(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    def: DefId,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    if state.call_depth >= state.max_call_depth {
        return Err(RuntimeError::limit_exceeded(format!(
            "call depth limit exceeded while entering definition {}",
            def.index
        )));
    }
    state.call_depth += 1;

    let result = execute_def_inner(mir, def_names, functions, state, def, args);
    state.call_depth -= 1;
    result
}

fn execute_def_inner(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    def: DefId,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    if let Some(name) = def_names.get(&def) {
        if is_builtin(name) {
            return execute_builtin(mir, name, def_names, functions, state, &[], args);
        }
    }

    // If the function has no MIR body (abstract ability method), try dynamic dispatch:
    // look up the concrete type of the first argument and find the matching impl method.
    if !functions.contains_key(&def) {
        if let Some(abstract_name) = def_names.get(&def) {
            let method_name = abstract_name.rsplit("::").next().unwrap_or(abstract_name);
            if let Some(receiver) = args.first() {
                let concrete_def = match receiver {
                    Value::Struct { def, .. } | Value::Enum { def, .. } => Some(*def),
                    _ => None,
                };
                if let Some(cdef) = concrete_def {
                    if let Some(type_name) = def_names.get(&cdef) {
                        let short_name = type_name.rsplit("::").next().unwrap_or(type_name);
                        let expected_suffix = format!("{}::{}", short_name, method_name);
                        for (candidate_def, candidate_name) in def_names {
                            if functions.contains_key(candidate_def)
                                && (candidate_name == &expected_suffix
                                    || candidate_name.ends_with(&format!("::{}", expected_suffix)))
                            {
                                return execute_def(
                                    mir,
                                    def_names,
                                    functions,
                                    state,
                                    *candidate_def,
                                    args,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    let function = functions.get(&def).copied().ok_or_else(|| {
        RuntimeError::missing_definition(format!("no MIR function for definition {}", def.index))
    })?;

    if args.len() != function.argc {
        return Err(RuntimeError::invalid_arguments(format!(
            "function expected {} args, got {}",
            function.argc,
            args.len()
        )));
    }

    let mut locals = vec![Value::Undef; function.locals.len()];
    for (offset, arg) in args.iter().enumerate() {
        let local_index = offset + 1;
        if local_index < locals.len() {
            locals[local_index] = arg.clone();
        }
    }

    let mut block = crate::mir::BlockId(0);
    let mut pending_error: Option<RuntimeError> = None;
    loop {
        let basic_block = function
            .basic_blocks
            .get(block.0 as usize)
            .ok_or_else(|| RuntimeError::new(format!("invalid block {}", block.0)))?;

        for statement in &basic_block.statements {
            state.step("statement")?;
            match &statement.kind {
                StatementKind::Assign(place, rvalue) => {
                    let value = eval_rvalue(mir, def_names, functions, &locals, rvalue)?;
                    write_place(&mut locals, place, value)?;
                }
                StatementKind::StorageLive(_)
                | StatementKind::StorageDead(_)
                | StatementKind::DeferStart(_)
                | StatementKind::ErrdeferStart(_)
                | StatementKind::Nop => {}
            }
        }

        let terminator = basic_block
            .terminator
            .as_ref()
            .ok_or_else(|| RuntimeError::new(format!("block {} missing terminator", block.0)))?;
        state.step("terminator")?;

        match &terminator.kind {
            TerminatorKind::Return => {
                return Ok(locals.first().cloned().unwrap_or(Value::Unit));
            }
            TerminatorKind::Goto(target) => {
                block = *target;
            }
            TerminatorKind::SwitchInt {
                discriminant,
                targets,
                otherwise,
            } => {
                let value = eval_operand(mir, functions, &locals, discriminant)?;
                let key = scalar_to_u128(&value)?;
                block = targets
                    .iter()
                    .find_map(|(candidate, block)| (*candidate == key).then_some(*block))
                    .unwrap_or(*otherwise);
            }
            TerminatorKind::Call {
                callee,
                args,
                destination,
                target,
                unwind,
            } => {
                let callee = eval_operand(mir, functions, &locals, callee)?;
                let call_args = args
                    .iter()
                    .map(|arg| eval_operand(mir, functions, &locals, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let call_args = call_args
                    .into_iter()
                    .map(|arg| capture_call_arg(&locals, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let (def, full_args) = match callee {
                    Value::Def(def) => (def, call_args),
                    Value::Closure { def, captures } => {
                        let mut full_args = captures;
                        full_args.extend(call_args);
                        (def, full_args)
                    }
                    other => {
                        return Err(RuntimeError::new(format!(
                            "cannot call non-function value `{}`",
                            other.render()
                        )));
                    }
                };
                let result = if let Some(name) = def_names.get(&def) {
                    if is_builtin(name) {
                        execute_builtin(mir, name, def_names, functions, state, &locals, &full_args)
                    } else {
                        execute_def(mir, def_names, functions, state, def, &full_args)
                    }
                } else {
                    execute_def(mir, def_names, functions, state, def, &full_args)
                };
                match result {
                    Ok(result) => {
                        write_place(&mut locals, destination, result)?;
                        block = target
                            .ok_or_else(|| RuntimeError::new("call terminator missing target"))?;
                    }
                    Err(error) => {
                        if let Some(unwind) = unwind {
                            pending_error = Some(error);
                            block = *unwind;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
            TerminatorKind::Assert {
                cond,
                expected,
                msg,
                target,
            } => {
                let cond = eval_operand(mir, functions, &locals, cond)?;
                let actual = scalar_to_bool(&cond)?;
                if actual == *expected {
                    block = *target;
                } else {
                    return Err(RuntimeError::new(*msg));
                }
            }
            TerminatorKind::Drop { place, target } => {
                let _ = place;
                // TODO(#43): keep the edge explicit, but real destructor/release lowering is not
                // implemented yet. The interpreter now matches native backends and treats `Drop`
                // as control-flow only.
                block = *target;
            }
            TerminatorKind::Unreachable => {
                return Err(RuntimeError::new("entered unreachable MIR block"));
            }
            TerminatorKind::ErrdeferUnwind(target) => {
                if let Some(error) = pending_error.take() {
                    return Err(error);
                }
                block = *target;
            }
        }
    }
}

fn eval_rvalue(
    mir: &MirModule,
    _def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    locals: &[Value],
    rvalue: &Rvalue,
) -> Result<Value, RuntimeError> {
    match rvalue {
        Rvalue::Use(operand) => eval_operand(mir, functions, locals, operand),
        Rvalue::Read(place) => read_place(locals, place),
        Rvalue::BinaryOp { op, lhs, rhs } => {
            let lhs = eval_operand(mir, functions, locals, lhs)?;
            let rhs = eval_operand(mir, functions, locals, rhs)?;
            eval_bin_op(*op, lhs, rhs)
        }
        Rvalue::UnaryOp { op, operand } => {
            let operand = eval_operand(mir, functions, locals, operand)?;
            eval_unary_op(*op, operand)
        }
        Rvalue::Ref { place, .. } | Rvalue::AddressOf { place, .. } => {
            Ok(Value::Ref(place.clone()))
        }
        Rvalue::Cast {
            operand, target_ty, ..
        } => {
            let value = eval_operand(mir, functions, locals, operand)?;
            cast_value(value, target_ty)
        }
        Rvalue::Aggregate(kind, operands) => {
            let values = operands
                .iter()
                .map(|operand| eval_operand(mir, functions, locals, operand))
                .collect::<Result<Vec<_>, _>>()?;
            match kind {
                AggregateKind::Tuple => Ok(Value::Tuple(values)),
                AggregateKind::Array(_) => Ok(Value::Array(values)),
                AggregateKind::Struct(def) => Ok(Value::Struct {
                    def: *def,
                    fields: values,
                }),
                AggregateKind::Closure(def) => Ok(Value::Closure {
                    def: *def,
                    captures: values,
                }),
                AggregateKind::Enum { def, variant_idx } => Ok(Value::Enum {
                    def: *def,
                    variant_idx: *variant_idx,
                    fields: values,
                }),
            }
        }
        Rvalue::Discriminant(place) => {
            let value = read_place(locals, place)?;
            match value {
                Value::Enum { variant_idx, .. } => Ok(Value::Uint(variant_idx as u128)),
                other => Ok(Value::Uint(scalar_to_u128(&other)?)),
            }
        }
        Rvalue::Len(place) => match read_place(locals, place)? {
            Value::Array(values) => Ok(Value::Uint(values.len() as u128)),
            other => Err(RuntimeError::new(format!(
                "cannot get length of `{}`",
                other.render()
            ))),
        },
    }
}

fn eval_operand(
    mir: &MirModule,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    locals: &[Value],
    operand: &Operand,
) -> Result<Value, RuntimeError> {
    match operand {
        Operand::Copy(local) | Operand::Move(local) => locals
            .get(local.0 as usize)
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!("invalid local {}", local.0))),
        Operand::Def(def) => {
            if functions.contains_key(def) {
                Ok(Value::Def(*def))
            } else if let Some(value) = mir.consts.iter().find(|item| item.def == *def) {
                Ok(const_to_value(&value.value))
            } else {
                Ok(Value::Def(*def))
            }
        }
        Operand::Const(value) => Ok(const_to_value(value)),
    }
}

fn const_to_value(value: &MirConst) -> Value {
    match value {
        MirConst::Bool(value) => Value::Bool(*value),
        MirConst::Int(value) => Value::Int(*value),
        MirConst::Uint(value) => Value::Uint(*value),
        MirConst::Float(value) => Value::Float(*value),
        MirConst::Char(value) => Value::Char(*value),
        MirConst::Str(value) => Value::Str(value.clone()),
        MirConst::Tuple(values) => Value::Tuple(values.iter().map(const_to_value).collect()),
        MirConst::Array(values) => Value::Array(values.iter().map(const_to_value).collect()),
        MirConst::Struct { def, fields } => Value::Struct {
            def: *def,
            fields: fields.iter().map(const_to_value).collect(),
        },
        MirConst::Ref(value) => Value::ConstRef(Box::new(const_to_value(value))),
        MirConst::Unit => Value::Unit,
        MirConst::Undef => Value::Undef,
    }
}

fn read_place(locals: &[Value], place: &Place) -> Result<Value, RuntimeError> {
    let mut value = locals
        .get(place.local.0 as usize)
        .cloned()
        .ok_or_else(|| RuntimeError::new(format!("invalid local {}", place.local.0)))?;

    for projection in &place.projections {
        value = match projection {
            Projection::Field(index) => match value {
                Value::Tuple(values) | Value::Array(values) => values
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))?,
                Value::Struct { fields, .. } | Value::Enum { fields, .. } => fields
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))?,
                other => {
                    return Err(RuntimeError::new(format!(
                        "cannot project field from `{}`",
                        other.render()
                    )))
                }
            },
            Projection::VariantField {
                variant_idx,
                field_idx,
            } => match value {
                Value::Enum {
                    variant_idx: actual_variant,
                    fields,
                    ..
                } if actual_variant == *variant_idx => fields
                    .get(*field_idx)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {field_idx}")))?,
                Value::Enum {
                    variant_idx: actual,
                    ..
                } => {
                    return Err(RuntimeError::new(format!(
                        "variant payload access expected variant {}, found {}",
                        variant_idx, actual
                    )))
                }
                other => {
                    return Err(RuntimeError::new(format!(
                        "cannot project enum payload from `{}`",
                        other.render()
                    )))
                }
            },
            Projection::Index(local) => {
                let index = locals
                    .get(local.0 as usize)
                    .ok_or_else(|| RuntimeError::new(format!("invalid local {}", local.0)))?;
                let index = scalar_to_usize(index)?;
                match value {
                    Value::Array(values) => values.get(index).cloned().ok_or_else(|| {
                        RuntimeError::new(format!("index {} out of bounds", index))
                    })?,
                    other => {
                        return Err(RuntimeError::new(format!(
                            "cannot index into `{}`",
                            other.render()
                        )))
                    }
                }
            }
            Projection::Deref => match value {
                Value::Ref(target) => read_place(locals, &target)?,
                Value::ConstRef(target) => *target,
                other => {
                    return Err(RuntimeError::new(format!(
                        "cannot dereference `{}`",
                        other.render()
                    )))
                }
            },
        };
    }

    Ok(value)
}

fn write_place(locals: &mut [Value], place: &Place, value: Value) -> Result<(), RuntimeError> {
    let local_index = place.local.0 as usize;
    let current = locals
        .get(local_index)
        .cloned()
        .ok_or_else(|| RuntimeError::new(format!("invalid local {}", place.local.0)))?;
    let updated = write_value(current, &place.projections, locals, value)?;
    locals[local_index] = updated;
    Ok(())
}

fn write_value(
    current: Value,
    projections: &[Projection],
    locals: &mut [Value],
    value: Value,
) -> Result<Value, RuntimeError> {
    if projections.is_empty() {
        return Ok(value);
    }

    match &projections[0] {
        Projection::Field(index) => match current {
            Value::Tuple(mut values) => {
                let slot = values
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))?;
                values[*index] = write_value(slot, &projections[1..], locals, value)?;
                Ok(Value::Tuple(values))
            }
            Value::Array(mut values) => {
                let slot = values
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))?;
                values[*index] = write_value(slot, &projections[1..], locals, value)?;
                Ok(Value::Array(values))
            }
            Value::Struct { def, mut fields } => {
                let slot = fields
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))?;
                fields[*index] = write_value(slot, &projections[1..], locals, value)?;
                Ok(Value::Struct { def, fields })
            }
            Value::Enum {
                def,
                variant_idx,
                mut fields,
            } => {
                let slot = fields
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))?;
                fields[*index] = write_value(slot, &projections[1..], locals, value)?;
                Ok(Value::Enum {
                    def,
                    variant_idx,
                    fields,
                })
            }
            other => Err(RuntimeError::new(format!(
                "cannot assign through `{}`",
                other.render()
            ))),
        },
        Projection::VariantField {
            variant_idx,
            field_idx,
        } => match current {
            Value::Enum {
                def,
                variant_idx: actual_variant,
                mut fields,
            } if actual_variant == *variant_idx => {
                let slot = fields
                    .get(*field_idx)
                    .cloned()
                    .ok_or_else(|| RuntimeError::new(format!("invalid field index {field_idx}")))?;
                fields[*field_idx] = write_value(slot, &projections[1..], locals, value)?;
                Ok(Value::Enum {
                    def,
                    variant_idx: actual_variant,
                    fields,
                })
            }
            Value::Enum {
                variant_idx: actual,
                ..
            } => Err(RuntimeError::new(format!(
                "variant payload write expected variant {}, found {}",
                variant_idx, actual
            ))),
            other => Err(RuntimeError::new(format!(
                "cannot assign through `{}`",
                other.render()
            ))),
        },
        Projection::Index(local) => {
            let index_value = locals
                .get(local.0 as usize)
                .cloned()
                .ok_or_else(|| RuntimeError::new(format!("invalid local {}", local.0)))?;
            let index = scalar_to_usize(&index_value)?;
            match current {
                Value::Array(mut values) => {
                    let slot = values.get(index).cloned().ok_or_else(|| {
                        RuntimeError::new(format!("index {} out of bounds", index))
                    })?;
                    values[index] = write_value(slot, &projections[1..], locals, value)?;
                    Ok(Value::Array(values))
                }
                other => Err(RuntimeError::new(format!(
                    "cannot assign through `{}`",
                    other.render()
                ))),
            }
        }
        Projection::Deref => {
            let target = match current {
                Value::Ref(target) => target,
                other => {
                    return Err(RuntimeError::new(format!(
                        "cannot dereference `{}`",
                        other.render()
                    )))
                }
            };
            let nested = Place {
                local: target.local,
                projections: target
                    .projections
                    .clone()
                    .into_iter()
                    .chain(projections[1..].iter().cloned())
                    .collect(),
            };
            write_place(locals, &nested, value)?;
            Ok(Value::Ref(target))
        }
    }
}

fn eval_bin_op(op: MirBinOp, lhs: Value, rhs: Value) -> Result<Value, RuntimeError> {
    match (op, lhs, rhs) {
        (MirBinOp::Add, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs + rhs)),
        (MirBinOp::Add, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs + rhs)),
        (MirBinOp::Sub, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs - rhs)),
        (MirBinOp::Sub, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs - rhs)),
        (MirBinOp::Mul, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs * rhs)),
        (MirBinOp::Mul, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs * rhs)),
        (MirBinOp::Div, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs / rhs)),
        (MirBinOp::Div, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs / rhs)),
        (MirBinOp::Rem, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs % rhs)),
        (MirBinOp::Rem, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs % rhs)),
        (MirBinOp::Eq, lhs, rhs) => Ok(Value::Bool(values_equal(&lhs, &rhs))),
        (MirBinOp::Ne, lhs, rhs) => Ok(Value::Bool(!values_equal(&lhs, &rhs))),
        (MirBinOp::Lt, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs < rhs)),
        (MirBinOp::Lt, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Bool(lhs < rhs)),
        (MirBinOp::Le, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs <= rhs)),
        (MirBinOp::Le, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Bool(lhs <= rhs)),
        (MirBinOp::Gt, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs > rhs)),
        (MirBinOp::Gt, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Bool(lhs > rhs)),
        (MirBinOp::Ge, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs >= rhs)),
        (MirBinOp::Ge, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Bool(lhs >= rhs)),
        (MirBinOp::BitAnd, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs && rhs)),
        (MirBinOp::BitAnd, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs & rhs)),
        (MirBinOp::BitAnd, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs & rhs)),
        (MirBinOp::BitOr, Value::Bool(lhs), Value::Bool(rhs)) => Ok(Value::Bool(lhs || rhs)),
        (MirBinOp::BitOr, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs | rhs)),
        (MirBinOp::BitOr, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs | rhs)),
        (MirBinOp::BitXor, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs ^ rhs)),
        (MirBinOp::BitXor, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs ^ rhs)),
        (MirBinOp::Shl, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs << rhs)),
        (MirBinOp::Shl, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs << rhs)),
        (MirBinOp::Shr, Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs >> rhs)),
        (MirBinOp::Shr, Value::Uint(lhs), Value::Uint(rhs)) => Ok(Value::Uint(lhs >> rhs)),
        (op, lhs, rhs) => Err(RuntimeError::new(format!(
            "unsupported binary op {:?} for `{}` and `{}`",
            op,
            lhs.render(),
            rhs.render()
        ))),
    }
}

fn eval_unary_op(op: MirUnaryOp, operand: Value) -> Result<Value, RuntimeError> {
    match (op, operand) {
        (MirUnaryOp::Neg, Value::Int(value)) => Ok(Value::Int(-value)),
        (MirUnaryOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (MirUnaryOp::Not, Value::Int(value)) => Ok(Value::Int(!value)),
        (op, value) => Err(RuntimeError::new(format!(
            "unsupported unary op {:?} for `{}`",
            op,
            value.render()
        ))),
    }
}

fn cast_value(value: Value, target_ty: &Ty) -> Result<Value, RuntimeError> {
    match (value, target_ty) {
        (Value::Bool(value), Ty::Bool) => Ok(Value::Bool(value)),
        (Value::Bool(value), Ty::Int(_)) => Ok(Value::Int(i128::from(value))),
        (Value::Bool(value), Ty::Uint(_)) => Ok(Value::Uint(u128::from(value))),
        (Value::Bool(value), Ty::Float(_)) => Ok(Value::Float(f64::from(u8::from(value)))),
        (Value::Int(value), Ty::Int(_)) => Ok(Value::Int(value)),
        (Value::Int(value), Ty::Uint(_)) => u128::try_from(value)
            .map(Value::Uint)
            .map_err(|_| RuntimeError::new(format!("cannot cast `{value}` to unsigned integer"))),
        (Value::Int(value), Ty::Float(_)) => Ok(Value::Float(value as f64)),
        (Value::Int(value), Ty::Char) => u32::try_from(value)
            .ok()
            .and_then(char::from_u32)
            .map(Value::Char)
            .ok_or_else(|| RuntimeError::new(format!("cannot cast `{value}` to `char`"))),
        (Value::Uint(value), Ty::Uint(_)) => Ok(Value::Uint(value)),
        (Value::Uint(value), Ty::Int(_)) => i128::try_from(value)
            .map(Value::Int)
            .map_err(|_| RuntimeError::new(format!("cannot cast `{value}` to signed integer"))),
        (Value::Uint(value), Ty::Float(_)) => Ok(Value::Float(value as f64)),
        (Value::Uint(value), Ty::Char) => u32::try_from(value)
            .ok()
            .and_then(char::from_u32)
            .map(Value::Char)
            .ok_or_else(|| RuntimeError::new(format!("cannot cast `{value}` to `char`"))),
        (Value::Float(value), Ty::Float(_)) => Ok(Value::Float(value)),
        (Value::Float(value), Ty::Int(_)) if value.is_finite() => {
            Ok(Value::Int(value.trunc() as i128))
        }
        (Value::Float(value), Ty::Uint(_)) if value.is_finite() && value >= 0.0 => {
            Ok(Value::Uint(value.trunc() as u128))
        }
        (Value::Char(value), Ty::Char) => Ok(Value::Char(value)),
        (Value::Char(value), Ty::Int(_)) => Ok(Value::Int(value as i128)),
        (Value::Char(value), Ty::Uint(_)) => Ok(Value::Uint(value as u32 as u128)),
        (Value::Unit, Ty::Unit) => Ok(Value::Unit),
        (value, _) => Ok(value),
    }
}

fn scalar_to_u128(value: &Value) -> Result<u128, RuntimeError> {
    match value {
        Value::Bool(value) => Ok(u128::from(*value)),
        Value::Int(value) => Ok(*value as u128),
        Value::Uint(value) => Ok(*value),
        Value::Unit => Ok(0),
        other => Err(RuntimeError::new(format!(
            "cannot use `{}` as switch discriminant",
            other.render()
        ))),
    }
}

fn scalar_to_bool(value: &Value) -> Result<bool, RuntimeError> {
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Int(value) => Ok(*value != 0),
        Value::Uint(value) => Ok(*value != 0),
        Value::Unit => Ok(false),
        other => Err(RuntimeError::new(format!(
            "cannot use `{}` as boolean condition",
            other.render()
        ))),
    }
}

fn scalar_to_usize(value: &Value) -> Result<usize, RuntimeError> {
    let scalar = scalar_to_u128(value)?;
    usize::try_from(scalar).map_err(|_| RuntimeError::new(format!("index `{scalar}` out of range")))
}

fn values_equal(lhs: &Value, rhs: &Value) -> bool {
    match (lhs, rhs) {
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(lhs), Value::Bool(rhs)) => lhs == rhs,
        (Value::Int(lhs), Value::Int(rhs)) => lhs == rhs,
        (Value::Uint(lhs), Value::Uint(rhs)) => lhs == rhs,
        (Value::Float(lhs), Value::Float(rhs)) => lhs == rhs,
        (Value::Char(lhs), Value::Char(rhs)) => lhs == rhs,
        (Value::Str(lhs), Value::Str(rhs)) => lhs == rhs,
        (Value::Str(lhs), Value::HeapString(rhs)) | (Value::HeapString(rhs), Value::Str(lhs)) => {
            lhs == &*rhs.borrow()
        }
        (Value::HeapString(lhs), Value::HeapString(rhs)) => {
            lhs.borrow().as_str() == rhs.borrow().as_str()
        }
        (Value::Vec(lhs), Value::Vec(rhs)) => {
            let lhs = lhs.borrow();
            let rhs = rhs.borrow();
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs.iter())
                    .all(|(lhs, rhs)| values_equal(lhs, rhs))
        }
        (Value::HashMap(lhs), Value::HashMap(rhs)) => {
            let lhs = lhs.borrow();
            let rhs = rhs.borrow();
            lhs.len() == rhs.len()
                && lhs.iter().all(|(key, lhs_value)| {
                    rhs.get(key)
                        .is_some_and(|rhs_value| values_equal(lhs_value, rhs_value))
                })
        }
        (Value::Tuple(lhs), Value::Tuple(rhs)) | (Value::Array(lhs), Value::Array(rhs)) => {
            lhs.len() == rhs.len() && lhs.iter().zip(rhs).all(|(lhs, rhs)| values_equal(lhs, rhs))
        }
        (Value::ConstRef(lhs), rhs) => values_equal(lhs, rhs),
        (lhs, Value::ConstRef(rhs)) => values_equal(lhs, rhs),
        (
            Value::Struct {
                def: lhs_def,
                fields: lhs,
            },
            Value::Struct {
                def: rhs_def,
                fields: rhs,
            },
        ) => {
            lhs_def == rhs_def
                && lhs.len() == rhs.len()
                && lhs.iter().zip(rhs).all(|(lhs, rhs)| values_equal(lhs, rhs))
        }
        (
            Value::Enum {
                def: lhs_def,
                variant_idx: lhs_variant,
                fields: lhs,
            },
            Value::Enum {
                def: rhs_def,
                variant_idx: rhs_variant,
                fields: rhs,
            },
        ) => {
            lhs_def == rhs_def
                && lhs_variant == rhs_variant
                && lhs.len() == rhs.len()
                && lhs.iter().zip(rhs).all(|(lhs, rhs)| values_equal(lhs, rhs))
        }
        _ => false,
    }
}

fn is_builtin(name: &str) -> bool {
    builtin_catalog::lookup(name).is_some()
}

fn execute_builtin(
    mir: &MirModule,
    name: &str,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    locals: &[Value],
    args: &[Value],
) -> Result<Value, RuntimeError> {
    let builtin = builtin_name(name).unwrap_or(name);
    if let Some(value) =
        builtins_collections::execute(mir, builtin, def_names, functions, state, locals, args)?
    {
        return Ok(value);
    }
    if let Some(value) =
        builtins_strings::execute(mir, builtin, def_names, functions, state, locals, args)?
    {
        return Ok(value);
    }
    if let Some(value) =
        builtins_system::execute(mir, builtin, def_names, functions, state, locals, args)?
    {
        return Ok(value);
    }
    if let Some(value) =
        builtins_network::execute(mir, builtin, def_names, functions, state, locals, args)?
    {
        return Ok(value);
    }

    match builtin {
        "print" => {
            if !args.is_empty() {
                print!(
                    "{}",
                    render_args(mir, def_names, functions, state, locals, args)?
                );
            }
            Ok(Value::Unit)
        }
        "println" => {
            if !args.is_empty() {
                println!(
                    "{}",
                    render_args(mir, def_names, functions, state, locals, args)?
                );
            } else {
                println!();
            }
            Ok(Value::Unit)
        }
        "eprint" => {
            if !args.is_empty() {
                eprint!(
                    "{}",
                    render_args(mir, def_names, functions, state, locals, args)?
                );
            }
            Ok(Value::Unit)
        }
        "eprintln" => {
            if !args.is_empty() {
                eprintln!(
                    "{}",
                    render_args(mir, def_names, functions, state, locals, args)?
                );
            } else {
                eprintln!();
            }
            Ok(Value::Unit)
        }
        "assert" => match args.first() {
            Some(Value::Bool(true)) => Ok(Value::Unit),
            Some(Value::Bool(false)) => Err(RuntimeError::assertion_failed("assertion failed")),
            Some(other) => Err(RuntimeError::invalid_arguments(format!(
                "assert expects bool, got `{}`",
                other.render()
            ))),
            None => Err(RuntimeError::invalid_arguments(
                "assert expects one argument",
            )),
        },
        "assert_eq" => {
            if args.len() != 2 {
                return Err(RuntimeError::invalid_arguments(
                    "assert_eq expects two arguments",
                ));
            }
            if values_equal(&args[0], &args[1]) {
                Ok(Value::Unit)
            } else {
                Err(RuntimeError::assertion_failed(format!(
                    "assert_eq failed: left=`{}`, right=`{}`",
                    render_value(mir, def_names, functions, state, locals, &args[0])?,
                    render_value(mir, def_names, functions, state, locals, &args[1])?
                )))
            }
        }
        "panic_with_fmt" => {
            if args.len() < 3 {
                return Err(RuntimeError::invalid_arguments(
                    "panic_with_fmt expects three arguments",
                ));
            }
            Err(RuntimeError::panic(format!(
                "{}: left=`{}`, right=`{}`",
                render_value(mir, def_names, functions, state, locals, &args[0])?,
                render_value(mir, def_names, functions, state, locals, &args[1])?,
                render_value(mir, def_names, functions, state, locals, &args[2])?
            )))
        }
        "panic" => {
            let message = match args.first() {
                Some(value) => render_value(mir, def_names, functions, state, locals, value)?,
                None => "panic".to_string(),
            };
            Err(RuntimeError::panic(message))
        }
        "format" => Ok(Value::HeapString(Rc::new(RefCell::new(format_builtin(
            mir, def_names, functions, state, locals, args,
        )?)))),
        _ => Err(RuntimeError::unsupported(format!(
            "unknown builtin `{name}`"
        ))),
    }
}

fn builtin_name(name: &str) -> Option<&str> {
    builtin_catalog::canonical_name(name)
}

fn render_args(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    locals: &[Value],
    args: &[Value],
) -> Result<String, RuntimeError> {
    format_builtin(mir, def_names, functions, state, locals, args)
}

fn option_value(
    def_names: &HashMap<DefId, String>,
    value: Option<Value>,
) -> Result<Value, RuntimeError> {
    let option_def = find_def(def_names, &["std::core::Option", "Option"])
        .ok_or_else(|| RuntimeError::new("cannot find std::core::Option definition"))?;
    Ok(match value {
        Some(value) => Value::Enum {
            def: option_def,
            variant_idx: 0,
            fields: vec![value],
        },
        None => Value::Enum {
            def: option_def,
            variant_idx: 1,
            fields: Vec::new(),
        },
    })
}

fn result_ok(def_names: &HashMap<DefId, String>, value: Value) -> Result<Value, RuntimeError> {
    let result_def = find_def(def_names, &["std::core::Result", "Result"])
        .ok_or_else(|| RuntimeError::new("cannot find std::core::Result definition"))?;
    Ok(Value::Enum {
        def: result_def,
        variant_idx: 0,
        fields: vec![value],
    })
}

fn result_err(def_names: &HashMap<DefId, String>, error: Value) -> Result<Value, RuntimeError> {
    let result_def = find_def(def_names, &["std::core::Result", "Result"])
        .ok_or_else(|| RuntimeError::new("cannot find std::core::Result definition"))?;
    Ok(Value::Enum {
        def: result_def,
        variant_idx: 1,
        fields: vec![error],
    })
}

/// Convert a Unix timestamp (seconds since epoch) to (year, month, day, hour, min, sec) UTC.
/// Uses the algorithm from http://howardhinnant.github.io/date_algorithms.html
fn unix_to_calendar(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let (days, day_secs) = if secs >= 0 {
        (secs / 86400, (secs % 86400) as u32)
    } else {
        let d = (secs - 86399) / 86400;
        let s = (secs - d * 86400) as u32;
        (d, s)
    };
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32, hour, min, sec)
}

fn find_def(def_names: &HashMap<DefId, String>, candidates: &[&str]) -> Option<DefId> {
    def_names.iter().find_map(|(def, name)| {
        candidates
            .iter()
            .any(|candidate| name == candidate)
            .then_some(*def)
    })
}

fn materialize_value(locals: &[Value], value: &Value) -> Result<Value, RuntimeError> {
    match value {
        Value::Ref(place) => read_place(locals, place),
        Value::ConstRef(value) => materialize_value(locals, value),
        other => Ok(other.clone()),
    }
}

fn capture_call_arg(locals: &[Value], value: Value) -> Result<Value, RuntimeError> {
    match value {
        Value::Ref(place) => Ok(Value::ConstRef(Box::new(read_place(locals, &place)?))),
        Value::ConstRef(value) => Ok(Value::ConstRef(Box::new(materialize_value(
            locals, &value,
        )?))),
        other => Ok(other),
    }
}

fn invoke_callable(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    callable: &Value,
    args: Vec<Value>,
) -> Result<Value, RuntimeError> {
    let call_args = args
        .into_iter()
        .map(|arg| capture_call_arg(&[], arg))
        .collect::<Result<Vec<_>, _>>()?;
    match callable {
        Value::Def(def) => execute_def(mir, def_names, functions, state, *def, &call_args),
        Value::Closure { def, captures } => {
            let mut full_args = captures.clone();
            full_args.extend(call_args);
            execute_def(mir, def_names, functions, state, *def, &full_args)
        }
        other => Err(RuntimeError::new(format!(
            "cannot call non-function value `{}`",
            other.render()
        ))),
    }
}

fn runtime_iter_next(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    iter: &mut Value,
) -> Result<Option<Value>, RuntimeError> {
    match iter {
        Value::VecIter { values, index } => {
            let current = *index.borrow();
            *index.borrow_mut() = current.saturating_add(1);
            Ok(values
                .borrow()
                .get(current)
                .cloned()
                .map(|value| Value::ConstRef(Box::new(value))))
        }
        Value::HashMapIter { entries, index } => {
            let current = *index.borrow();
            *index.borrow_mut() = current.saturating_add(1);
            Ok(entries.borrow().iter().nth(current).map(|(key, value)| {
                Value::Tuple(vec![
                    Value::ConstRef(Box::new(key.to_value())),
                    Value::ConstRef(Box::new(value.clone())),
                ])
            }))
        }
        Value::MapIter { iter, mapper } => {
            let next = {
                let mut inner = iter.borrow_mut();
                runtime_iter_next(mir, def_names, functions, state, &mut inner)?
            };
            match next {
                Some(item) => {
                    invoke_callable(mir, def_names, functions, state, mapper, vec![item]).map(Some)
                }
                None => Ok(None),
            }
        }
        Value::FilterIter { iter, predicate } => loop {
            let next = {
                let mut inner = iter.borrow_mut();
                runtime_iter_next(mir, def_names, functions, state, &mut inner)?
            };
            let Some(item) = next else {
                return Ok(None);
            };
            let passed = invoke_callable(
                mir,
                def_names,
                functions,
                state,
                predicate,
                vec![item.clone()],
            )?;
            match passed {
                Value::Bool(true) => return Ok(Some(item)),
                Value::Bool(false) => {}
                other => {
                    return Err(RuntimeError::new(format!(
                        "iterator filter predicate must return bool, got `{}`",
                        other.render()
                    )))
                }
            }
        },
        other => Err(RuntimeError::new(format!(
            "value `{}` is not a runtime iterator",
            other.render()
        ))),
    }
}

fn string_arg(locals: &[Value], value: &Value) -> Result<String, RuntimeError> {
    match materialize_value(locals, value)? {
        Value::Str(value) => Ok(value),
        Value::HeapString(value) => Ok(value.borrow().clone()),
        other => Err(RuntimeError::new(format!(
            "expected string argument, got `{}`",
            other.render()
        ))),
    }
}

fn struct_field_index(mir: &MirModule, def: DefId, name: &str) -> Option<usize> {
    mir.struct_field_names
        .get(&def)
        .and_then(|fields| fields.iter().position(|field| field == name))
}

fn struct_field_value(
    mir: &MirModule,
    value: &Value,
    field_name: &str,
) -> Result<Value, RuntimeError> {
    let Value::Struct { def, fields } = value else {
        return Err(RuntimeError::new(format!(
            "expected struct value, got `{}`",
            value.render()
        )));
    };
    let Some(index) = struct_field_index(mir, *def, field_name) else {
        return Err(RuntimeError::new(format!(
            "missing struct field `{field_name}` on definition {}",
            def.index
        )));
    };
    fields
        .get(index)
        .cloned()
        .ok_or_else(|| RuntimeError::new(format!("invalid field index {index}")))
}

fn ip_addr_value(
    def_names: &HashMap<DefId, String>,
    addr: std::net::IpAddr,
) -> Result<Value, RuntimeError> {
    let ip_def = find_def(def_names, &["std::net::IpAddr", "IpAddr"])
        .ok_or_else(|| RuntimeError::new("cannot find std::net::IpAddr definition"))?;
    Ok(match addr {
        std::net::IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            Value::Enum {
                def: ip_def,
                variant_idx: 0,
                fields: vec![
                    Value::Uint(a as u128),
                    Value::Uint(b as u128),
                    Value::Uint(c as u128),
                    Value::Uint(d as u128),
                ],
            }
        }
        std::net::IpAddr::V6(v6) => Value::Enum {
            def: ip_def,
            variant_idx: 1,
            fields: vec![Value::Array(
                v6.octets()
                    .into_iter()
                    .map(|byte| Value::Uint(byte as u128))
                    .collect(),
            )],
        },
    })
}

fn socket_addr_value(
    def_names: &HashMap<DefId, String>,
    addr: std::net::SocketAddr,
) -> Result<Value, RuntimeError> {
    let socket_addr_def = find_def(def_names, &["std::net::SocketAddr", "SocketAddr"])
        .ok_or_else(|| RuntimeError::new("cannot find std::net::SocketAddr definition"))?;
    Ok(Value::Struct {
        def: socket_addr_def,
        fields: vec![
            ip_addr_value(def_names, addr.ip())?,
            Value::Uint(addr.port() as u128),
        ],
    })
}

fn runtime_socket_addr(
    mir: &MirModule,
    locals: &[Value],
    value: &Value,
) -> Result<std::net::SocketAddr, RuntimeError> {
    let value = materialize_value(locals, value)?;
    let addr = struct_field_value(mir, &value, "addr")?;
    let port = scalar_to_usize(&struct_field_value(mir, &value, "port")?)?;
    let ip = match materialize_value(locals, &addr)? {
        Value::Enum {
            variant_idx: 0,
            fields,
            ..
        } => match fields.as_slice() {
            [Value::Uint(a), Value::Uint(b), Value::Uint(c), Value::Uint(d)] => {
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    *a as u8, *b as u8, *c as u8, *d as u8,
                ))
            }
            _ => return Err(RuntimeError::new("malformed IpAddr::V4 payload")),
        },
        Value::Enum {
            variant_idx: 1,
            fields,
            ..
        } => match fields.as_slice() {
            [Value::Array(bytes)] if bytes.len() == 16 => {
                let mut octets = [0u8; 16];
                for (index, value) in bytes.iter().enumerate() {
                    let Value::Uint(byte) = value else {
                        return Err(RuntimeError::new("malformed IpAddr::V6 payload"));
                    };
                    octets[index] = *byte as u8;
                }
                std::net::IpAddr::V6(std::net::Ipv6Addr::from(octets))
            }
            _ => return Err(RuntimeError::new("malformed IpAddr::V6 payload")),
        },
        other => {
            return Err(RuntimeError::new(format!(
                "expected SocketAddr.ip enum, got `{}`",
                other.render()
            )))
        }
    };
    Ok(std::net::SocketAddr::new(ip, port as u16))
}

fn http_send_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    locals: &[Value],
    client: &Value,
    request: &Value,
) -> Result<Value, RuntimeError> {
    let client = materialize_value(locals, client)?;
    let request = materialize_value(locals, request)?;
    let timeout_ms = scalar_to_usize(&struct_field_value(mir, &client, "_timeout_ms")?)?;
    let method = http_method_string(mir, locals, &struct_field_value(mir, &request, "method")?)?;
    let url = string_arg(locals, &struct_field_value(mir, &request, "url")?)?;
    let body = string_arg(locals, &struct_field_value(mir, &request, "body")?)?;
    let headers = runtime_headers_map(mir, locals, &struct_field_value(mir, &request, "headers")?)?;

    let (host, port, path) = parse_http_url(&url)?;
    let mut stream = std::net::TcpStream::connect((host.as_str(), port))
        .map_err(|error| RuntimeError::new(format!("http connect failed: {error}")))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout_ms as u64)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(timeout_ms as u64)));

    let mut request_text =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    let has_content_type = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    for (name, value) in &headers {
        request_text.push_str(name);
        request_text.push_str(": ");
        request_text.push_str(value);
        request_text.push_str("\r\n");
    }
    if !body.is_empty() && !has_content_type {
        request_text.push_str("Content-Type: text/plain\r\n");
    }
    if !body.is_empty() {
        request_text.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request_text.push_str("\r\n");
    request_text.push_str(&body);

    stream
        .write_all(request_text.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|error| RuntimeError::new(format!("http write failed: {error}")))?;

    let mut response_text = String::new();
    stream
        .read_to_string(&mut response_text)
        .map_err(|error| RuntimeError::new(format!("http read failed: {error}")))?;
    let response = parse_http_response(def_names, &response_text)?;
    result_ok(def_names, response)
}

fn execute_callable_value(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    callee: &Value,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    let (def, full_args) = match callee {
        Value::Def(def) => (*def, args.to_vec()),
        Value::Closure { def, captures } => {
            let mut full_args = captures.clone();
            full_args.extend_from_slice(args);
            (*def, full_args)
        }
        other => {
            return Err(RuntimeError::new(format!(
                "cannot call non-function value `{}`",
                other.render()
            )))
        }
    };
    if let Some(name) = def_names.get(&def) {
        if is_builtin(name) {
            execute_builtin(mir, name, def_names, functions, state, &[], &full_args)
        } else {
            execute_def(mir, def_names, functions, state, def, &full_args)
        }
    } else {
        execute_def(mir, def_names, functions, state, def, &full_args)
    }
}

fn task_spawn_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    if args.len() != 1 {
        return Err(RuntimeError::new("task_spawn expects one argument"));
    }
    let _ = (mir, def_names, functions, state);
    Ok(Value::JoinHandle(Rc::new(RefCell::new(
        JoinHandleState::Pending(Box::new(args[0].clone())),
    ))))
}

fn task_join_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    if args.len() != 1 {
        return Err(RuntimeError::new("task_join expects one argument"));
    }
    match &args[0] {
        Value::JoinHandle(result) => {
            let pending = {
                let state = result.borrow();
                match &*state {
                    JoinHandleState::Pending(job) => Some((**job).clone()),
                    JoinHandleState::Ready(value) => return Ok((**value).clone()),
                }
            };
            let Some(job) = pending else {
                return Err(RuntimeError::new("cannot join an empty task handle"));
            };
            let value = execute_callable_value(mir, def_names, functions, state, &job, &[])?;
            *result.borrow_mut() = JoinHandleState::Ready(Box::new(value.clone()));
            Ok(value)
        }
        other => Err(RuntimeError::new(format!(
            "task_join expects JoinHandle receiver, got `{}`",
            other.render()
        ))),
    }
}

fn task_block_on_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    if args.len() != 1 {
        return Err(RuntimeError::new("task_block_on expects one argument"));
    }
    execute_callable_value(mir, def_names, functions, state, &args[0], &[])
}

fn task_sleep_ms_builtin(args: &[Value]) -> Result<Value, RuntimeError> {
    if args.len() != 1 {
        return Err(RuntimeError::new("task_sleep_ms expects one argument"));
    }
    let millis = scalar_to_usize(&args[0])? as u64;
    thread::sleep(Duration::from_millis(millis));
    Ok(Value::Unit)
}

fn http_headers_get_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    locals: &[Value],
    headers: &Value,
    name: &Value,
) -> Result<Value, RuntimeError> {
    let header_name = string_arg(locals, name)?.to_ascii_lowercase();
    let header_entries = runtime_headers_map(mir, locals, headers)?;
    let value = header_entries
        .into_iter()
        .find_map(|(key, value)| (key == header_name).then_some(Value::Str(value)));
    option_value(def_names, value)
}

fn http_body_json_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    locals: &[Value],
    response: &Value,
) -> Result<Value, RuntimeError> {
    let response = materialize_value(locals, response)?;
    let body = string_arg(locals, &struct_field_value(mir, &response, "body")?)?;
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(value) => result_ok(def_names, json_runtime_value(def_names, value)?),
        Err(error) => result_err(
            def_names,
            Value::HeapString(Rc::new(RefCell::new(error.to_string()))),
        ),
    }
}

fn http_method_string(
    mir: &MirModule,
    locals: &[Value],
    value: &Value,
) -> Result<String, RuntimeError> {
    match materialize_value(locals, value)? {
        Value::Enum {
            variant_idx,
            fields,
            ..
        } => Ok(match variant_idx {
            0 => "GET".to_string(),
            1 => "POST".to_string(),
            2 => "PUT".to_string(),
            3 => "PATCH".to_string(),
            4 => "DELETE".to_string(),
            5 => "HEAD".to_string(),
            6 => "OPTIONS".to_string(),
            7 => "TRACE".to_string(),
            8 => "CONNECT".to_string(),
            9 => match fields.first() {
                Some(value) => string_arg(locals, value)?,
                None => return Err(RuntimeError::new("malformed Method::Custom payload")),
            },
            _ => return Err(RuntimeError::new("unknown http method variant")),
        }),
        Value::Struct { .. } => string_arg(locals, &struct_field_value(mir, value, "method")?),
        other => Err(RuntimeError::new(format!(
            "expected http method value, got `{}`",
            other.render()
        ))),
    }
}

fn runtime_headers_map(
    mir: &MirModule,
    locals: &[Value],
    value: &Value,
) -> Result<Vec<(String, String)>, RuntimeError> {
    let headers = materialize_value(locals, value)?;
    let map_value = struct_field_value(mir, &headers, "_map")?;
    match materialize_value(locals, &map_value)? {
        Value::HashMap(entries) => {
            let mut rendered = Vec::new();
            for (key, value) in entries.borrow().iter() {
                let HashableValue::String(key) = key else {
                    continue;
                };
                rendered.push((key.clone(), string_arg(locals, value)?));
            }
            Ok(rendered)
        }
        other => Err(RuntimeError::new(format!(
            "expected Headers map payload, got `{}`",
            other.render()
        ))),
    }
}

fn parse_http_url(url: &str) -> Result<(String, u16, String), RuntimeError> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(RuntimeError::new(
            "http client currently supports only plain http:// URLs",
        ));
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{}", path)),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') => {
            let port = port
                .parse::<u16>()
                .map_err(|_| RuntimeError::new("invalid http URL port"))?;
            (host.to_string(), port)
        }
        _ => (authority.to_string(), 80),
    };
    Ok((host, port, path))
}

fn parse_http_response(
    def_names: &HashMap<DefId, String>,
    response_text: &str,
) -> Result<Value, RuntimeError> {
    let Some((head, body)) = response_text.split_once("\r\n\r\n") else {
        return Err(RuntimeError::new("malformed HTTP response"));
    };
    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| RuntimeError::new("missing HTTP status line"))?;
    let mut parts = status_line.split_whitespace();
    let _version = parts.next();
    let status = parts
        .next()
        .ok_or_else(|| RuntimeError::new("missing HTTP status code"))?
        .parse::<u16>()
        .map_err(|_| RuntimeError::new("invalid HTTP status code"))?;

    let headers_def = find_def(def_names, &["std::http::Headers", "Headers"])
        .ok_or_else(|| RuntimeError::new("cannot find std::http::Headers definition"))?;
    let response_def = find_def(def_names, &["std::http::Response", "Response"])
        .ok_or_else(|| RuntimeError::new("cannot find std::http::Response definition"))?;
    let status_def = find_def(def_names, &["std::http::StatusCode", "StatusCode"])
        .ok_or_else(|| RuntimeError::new("cannot find std::http::StatusCode definition"))?;

    let mut header_entries = std::collections::HashMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        header_entries.insert(
            HashableValue::String(name.trim().to_ascii_lowercase()),
            Value::HeapString(Rc::new(RefCell::new(value.trim().to_string()))),
        );
    }

    Ok(Value::Struct {
        def: response_def,
        fields: vec![
            Value::Struct {
                def: status_def,
                fields: vec![Value::Uint(status as u128)],
            },
            Value::Struct {
                def: headers_def,
                fields: vec![Value::HashMap(Rc::new(RefCell::new(header_entries)))],
            },
            Value::HeapString(Rc::new(RefCell::new(body.to_string()))),
        ],
    })
}

fn split_string_parts(input: &str, delimiter: &str) -> Vec<String> {
    if delimiter.is_empty() {
        return input.chars().map(|ch| ch.to_string()).collect();
    }
    input
        .split(delimiter)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
}

fn hashable_arg(locals: &[Value], value: &Value) -> Result<HashableValue, RuntimeError> {
    materialize_hashable_value(materialize_value(locals, value)?)
}

fn materialize_hashable_value(value: Value) -> Result<HashableValue, RuntimeError> {
    match value {
        Value::Unit => Ok(HashableValue::Unit),
        Value::Bool(value) => Ok(HashableValue::Bool(value)),
        Value::Int(value) => Ok(HashableValue::Int(value)),
        Value::Uint(value) => Ok(HashableValue::Uint(value)),
        Value::Char(value) => Ok(HashableValue::Char(value)),
        Value::Str(value) => Ok(HashableValue::String(value)),
        Value::HeapString(value) => Ok(HashableValue::String(value.borrow().clone())),
        Value::Tuple(values) => Ok(HashableValue::Tuple(
            values
                .into_iter()
                .map(materialize_hashable_value)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Array(values) => Ok(HashableValue::Array(
            values
                .into_iter()
                .map(materialize_hashable_value)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Struct { def, fields } => Ok(HashableValue::Struct {
            def,
            fields: fields
                .into_iter()
                .map(materialize_hashable_value)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        Value::Enum {
            def,
            variant_idx,
            fields,
        } => Ok(HashableValue::Enum {
            def,
            variant_idx,
            fields: fields
                .into_iter()
                .map(materialize_hashable_value)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        other => Err(RuntimeError::new(format!(
            "value `{}` cannot be used as a HashMap key",
            other.render()
        ))),
    }
}

fn io_error_value(
    def_names: &HashMap<DefId, String>,
    message: String,
) -> Result<Value, RuntimeError> {
    let error_def = find_def(def_names, &["std::io::Error", "Error"])
        .ok_or_else(|| RuntimeError::new("cannot find std::io::Error definition"))?;
    let error_kind_def = find_def(def_names, &["std::io::ErrorKind", "ErrorKind"])
        .ok_or_else(|| RuntimeError::new("cannot find std::io::ErrorKind definition"))?;
    Ok(Value::Struct {
        def: error_def,
        fields: vec![
            Value::Enum {
                def: error_kind_def,
                variant_idx: 15,
                fields: Vec::new(),
            },
            Value::HeapString(Rc::new(RefCell::new(message))),
        ],
    })
}

fn json_parse_error_value(
    def_names: &HashMap<DefId, String>,
    message: String,
    offset: u128,
) -> Result<Value, RuntimeError> {
    let error_def = find_def(def_names, &["std::json::ParseError", "ParseError"])
        .ok_or_else(|| RuntimeError::new("cannot find std::json::ParseError definition"))?;
    Ok(Value::Struct {
        def: error_def,
        fields: vec![
            Value::HeapString(Rc::new(RefCell::new(message))),
            Value::Uint(offset),
        ],
    })
}

fn json_runtime_value(
    def_names: &HashMap<DefId, String>,
    value: serde_json::Value,
) -> Result<Value, RuntimeError> {
    let json_def = find_def(def_names, &["std::json::Value", "Value"])
        .ok_or_else(|| RuntimeError::new("cannot find std::json::Value definition"))?;
    Ok(match value {
        serde_json::Value::Null => Value::Enum {
            def: json_def,
            variant_idx: 0,
            fields: Vec::new(),
        },
        serde_json::Value::Bool(flag) => Value::Enum {
            def: json_def,
            variant_idx: 1,
            fields: vec![Value::Bool(flag)],
        },
        serde_json::Value::Number(number) => Value::Enum {
            def: json_def,
            variant_idx: 2,
            fields: vec![Value::Float(number.as_f64().unwrap_or(0.0))],
        },
        serde_json::Value::String(text) => Value::Enum {
            def: json_def,
            variant_idx: 3,
            fields: vec![Value::HeapString(Rc::new(RefCell::new(text)))],
        },
        serde_json::Value::Array(items) => Value::Enum {
            def: json_def,
            variant_idx: 4,
            fields: vec![Value::Vec(Rc::new(RefCell::new(
                items
                    .into_iter()
                    .map(|item| json_runtime_value(def_names, item))
                    .collect::<Result<Vec<_>, _>>()?,
            )))],
        },
        serde_json::Value::Object(items) => {
            let mut map = std::collections::HashMap::new();
            for (key, value) in items {
                map.insert(
                    HashableValue::String(key),
                    json_runtime_value(def_names, value)?,
                );
            }
            Value::Enum {
                def: json_def,
                variant_idx: 5,
                fields: vec![Value::HashMap(Rc::new(RefCell::new(map)))],
            }
        }
    })
}

fn runtime_to_json(locals: &[Value], value: &Value) -> Result<serde_json::Value, RuntimeError> {
    match materialize_value(locals, value)? {
        Value::Enum { variant_idx: 0, .. } => Ok(serde_json::Value::Null),
        Value::Enum {
            variant_idx: 1,
            fields,
            ..
        } => match fields.as_slice() {
            [Value::Bool(flag)] => Ok(serde_json::Value::Bool(*flag)),
            _ => Err(RuntimeError::new("json bool payload is malformed")),
        },
        Value::Enum {
            variant_idx: 2,
            fields,
            ..
        } => match fields.as_slice() {
            [Value::Float(number)] => serde_json::Number::from_f64(*number)
                .map(serde_json::Value::Number)
                .ok_or_else(|| RuntimeError::new("json number is not finite")),
            _ => Err(RuntimeError::new("json number payload is malformed")),
        },
        Value::Enum {
            variant_idx: 3,
            fields,
            ..
        } => match fields.as_slice() {
            [value] => Ok(serde_json::Value::String(string_arg(locals, value)?)),
            _ => Err(RuntimeError::new("json string payload is malformed")),
        },
        Value::Enum {
            variant_idx: 4,
            fields,
            ..
        } => match fields.as_slice() {
            [Value::Vec(values)] => values
                .borrow()
                .iter()
                .map(|item| runtime_to_json(locals, item))
                .collect::<Result<Vec<_>, _>>()
                .map(serde_json::Value::Array),
            _ => Err(RuntimeError::new("json array payload is malformed")),
        },
        Value::Enum {
            variant_idx: 5,
            fields,
            ..
        } => match fields.as_slice() {
            [Value::HashMap(entries)] => {
                let mut map = serde_json::Map::new();
                for (key, value) in entries.borrow().iter() {
                    let HashableValue::String(key) = key else {
                        return Err(RuntimeError::new(
                            "json object keys must be strings at runtime",
                        ));
                    };
                    map.insert(key.clone(), runtime_to_json(locals, value)?);
                }
                Ok(serde_json::Value::Object(map))
            }
            _ => Err(RuntimeError::new("json object payload is malformed")),
        },
        other => Err(RuntimeError::new(format!(
            "expected std::json::Value, got `{}`",
            other.render()
        ))),
    }
}

fn format_builtin(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    locals: &[Value],
    args: &[Value],
) -> Result<String, RuntimeError> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(String::new());
    };
    let Ok(template) = string_arg(locals, first) else {
        let mut parts = Vec::with_capacity(args.len());
        for arg in args {
            parts.push(render_value(mir, def_names, functions, state, locals, arg)?);
        }
        return Ok(parts.join(" "));
    };
    if !template.contains("{}") {
        let mut parts = vec![template];
        for value in rest {
            parts.push(render_value(
                mir, def_names, functions, state, locals, value,
            )?);
        }
        return Ok(parts.join(" "));
    }

    let mut rendered = String::new();
    let mut remainder = template.as_str();
    let mut values = rest.iter();
    while let Some(index) = remainder.find("{}") {
        rendered.push_str(&remainder[..index]);
        if let Some(value) = values.next() {
            rendered.push_str(&render_value(
                mir, def_names, functions, state, locals, value,
            )?);
        } else {
            rendered.push_str("{}");
        }
        remainder = &remainder[index + 2..];
    }
    rendered.push_str(remainder);

    let mut extras = Vec::new();
    for value in values {
        extras.push(render_value(
            mir, def_names, functions, state, locals, value,
        )?);
    }
    if !extras.is_empty() {
        if !rendered.is_empty() {
            rendered.push(' ');
        }
        rendered.push_str(&extras.join(" "));
    }
    Ok(rendered)
}

fn render_value(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    locals: &[Value],
    value: &Value,
) -> Result<String, RuntimeError> {
    let materialized = materialize_value(locals, value)?;
    render_display_value(mir, def_names, functions, state, &materialized)
}

fn render_display_value(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    functions: &HashMap<DefId, &crate::mir::MirFn>,
    state: &mut ExecutionState,
    value: &Value,
) -> Result<String, RuntimeError> {
    match value {
        Value::Unit
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Uint(_)
        | Value::Float(_)
        | Value::Char(_)
        | Value::Str(_)
        | Value::HeapString(_) => Ok(value.render()),
        Value::Struct { def, .. } | Value::Enum { def, .. } => {
            if mir.display_impls.contains(def) {
                let Some(fmt_def) = find_display_fmt_def(def_names, *def) else {
                    return render_materialized_value(mir, def_names, value);
                };
                let formatter_def = find_def(def_names, &["std::fmt::Formatter", "Formatter"])
                    .ok_or_else(|| {
                        RuntimeError::new("cannot find std::fmt::Formatter definition")
                    })?;
                let buffer = Rc::new(RefCell::new(String::new()));
                let formatter = Value::Struct {
                    def: formatter_def,
                    fields: vec![Value::HeapString(buffer.clone())],
                };
                let _ = execute_def(
                    mir,
                    def_names,
                    functions,
                    state,
                    fmt_def,
                    &[value.clone(), Value::ConstRef(Box::new(formatter))],
                )?;
                let rendered = buffer.borrow().clone();
                Ok(rendered)
            } else {
                render_materialized_value(mir, def_names, value)
            }
        }
        _ => render_materialized_value(mir, def_names, value),
    }
}

fn render_materialized_value(
    mir: &MirModule,
    def_names: &HashMap<DefId, String>,
    value: &Value,
) -> Result<String, RuntimeError> {
    Ok(match value {
        Value::Unit
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Uint(_)
        | Value::Float(_)
        | Value::Char(_)
        | Value::Str(_)
        | Value::HeapString(_)
        | Value::Ref(_)
        | Value::ConstRef(_)
        | Value::Closure { .. }
        | Value::Def(_)
        | Value::JoinHandle(_)
        | Value::TcpStream(_)
        | Value::TcpListener(_)
        | Value::UdpSocket(_)
        | Value::Undef => value.render(),
        Value::Vec(values) => {
            let mut rendered = Vec::new();
            for item in values.borrow().iter() {
                rendered.push(render_materialized_value(mir, def_names, item)?);
            }
            format!("[{}]", rendered.join(", "))
        }
        Value::StringSplit { .. }
        | Value::VecIter { .. }
        | Value::MapIter { .. }
        | Value::FilterIter { .. }
        | Value::HashMapIter { .. } => value.render(),
        Value::HashMap(entries) => {
            let mut rendered = Vec::new();
            for (key, value) in entries.borrow().iter() {
                rendered.push(format!(
                    "{}: {}",
                    key.render(),
                    render_materialized_value(mir, def_names, value)?
                ));
            }
            format!("{{{}}}", rendered.join(", "))
        }
        Value::Tuple(values) | Value::Array(values) => {
            let mut rendered = Vec::new();
            for item in values {
                rendered.push(render_materialized_value(mir, def_names, item)?);
            }
            if matches!(value, Value::Tuple(_)) {
                format!("({})", rendered.join(", "))
            } else {
                format!("[{}]", rendered.join(", "))
            }
        }
        Value::Struct { def, fields } => {
            let type_name = short_def_name(def_names, *def);
            let field_names = mir.struct_field_names.get(def);
            let mut rendered = Vec::new();
            for (index, field) in fields.iter().enumerate() {
                let label = field_names
                    .and_then(|names| names.get(index))
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| index.to_string());
                rendered.push(format!(
                    "{label}: {}",
                    render_materialized_value(mir, def_names, field)?
                ));
            }
            format!("{type_name} {{ {} }}", rendered.join(", "))
        }
        Value::Enum {
            def,
            variant_idx,
            fields,
        } => {
            let type_name = short_def_name(def_names, *def);
            let variant_name = mir
                .enum_variant_names
                .get(def)
                .and_then(|names| names.get(*variant_idx))
                .cloned()
                .unwrap_or_else(|| variant_idx.to_string());
            if fields.is_empty() {
                format!("{type_name}::{variant_name}")
            } else {
                let mut rendered = Vec::new();
                for field in fields {
                    rendered.push(render_materialized_value(mir, def_names, field)?);
                }
                format!("{type_name}::{variant_name}({})", rendered.join(", "))
            }
        }
    })
}

fn short_def_name(def_names: &HashMap<DefId, String>, def: DefId) -> String {
    def_names
        .get(&def)
        .and_then(|name| name.rsplit("::").next())
        .unwrap_or("_")
        .to_string()
}

fn find_display_fmt_def(def_names: &HashMap<DefId, String>, owner_def: DefId) -> Option<DefId> {
    let short_name = short_def_name(def_names, owner_def);
    let candidate = format!("{short_name}::fmt");
    def_names
        .iter()
        .find_map(|(def, name)| (name == &candidate).then_some(*def))
}
