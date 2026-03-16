//! High-level Intermediate Representation (HIR).
//!
//! The HIR is produced from the AST after name resolution.
//! It desugars surface syntax (e.g. `for` loops → iterator method calls,
//! `?` operator → explicit `match` on `Result`), and assigns stable
//! node IDs to every expression, statement, and item.

use crate::source::{FileId, Span};
use std::collections::HashMap;

// ─── Node IDs ─────────────────────────────────────────────────────────────────

/// A stable, unique identifier for any HIR node within a compilation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HirId(pub u32);

/// Counter used to allocate fresh HIR node IDs during AST → HIR lowering.
#[derive(Default, Clone)]
pub struct HirIdAlloc {
    next: u32,
}

impl HirIdAlloc {
    pub fn fresh(&mut self) -> HirId {
        let id = self.next;
        self.next += 1;
        HirId(id)
    }
}

// ─── Resolved definition IDs ─────────────────────────────────────────────────

/// A resolved reference to a named definition (function, struct, enum, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefId {
    pub file: FileId,
    /// Index within the file's definition table.
    pub index: u32,
}

// ─── HIR types ────────────────────────────────────────────────────────────────

/// A fully-resolved type after name resolution.
/// Type variables (`TyVar`) are filled in by the type checker.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Primitive scalar types.
    Bool,
    Char,
    /// Signed integers: i8, i16, i32, i64, i128, isize.
    Int(IntSize),
    /// Unsigned integers: u8, u16, u32, u64, u128, usize.
    Uint(UintSize),
    /// Floating point: f32, f64.
    Float(FloatSize),
    /// The unit type `()`.
    Unit,
    /// The never type `!` — a type that has no values.
    Never,
    /// A reference `&T` or `&mut T`.
    Ref {
        mutable: bool,
        inner: Box<Ty>,
    },
    /// A raw pointer `*const T` or `*mut T`.
    RawPtr {
        mutable: bool,
        inner: Box<Ty>,
    },
    /// A fixed-size array `[T; N]`.
    Array {
        elem: Box<Ty>,
        len: usize,
    },
    /// A slice `[T]`.
    Slice(Box<Ty>),
    /// A tuple `(T1, T2, ..., Tn)`.
    Tuple(Vec<Ty>),
    /// A named type (struct / enum / type alias) with resolved `DefId`.
    Named {
        def: DefId,
        args: Vec<Ty>,
    },
    /// A function pointer `fn(T1, T2) -> T3`.
    FnPtr {
        params: Vec<Ty>,
        ret: Box<Ty>,
    },
    /// A type variable (filled in by the type checker).
    Var(u32),
    /// An opaque `impl Trait` type.
    ImplTrait(Vec<DefId>),
    /// A `dyn Ability` trait-object type.
    DynTrait(DefId),
    /// `str` (unsized).
    Str,
    /// The `String` type (sugar for `std::core::String`).
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntSize {
    I8,
    I16,
    I32,
    I64,
    I128,
    ISize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UintSize {
    U8,
    U16,
    U32,
    U64,
    U128,
    USize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSize {
    F32,
    F64,
}

// ─── HIR expressions ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirExpr {
    pub id: HirId,
    pub kind: HirExprKind,
    pub span: Span,
    /// Type annotation filled in by the type checker (initially `Ty::Var`).
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum HirExprKind {
    Lit(HirLit),
    Var(HirId),
    DefRef(DefId),
    Block(Vec<HirStmt>, Option<Box<HirExpr>>),
    Call {
        callee: Box<HirExpr>,
        args: Vec<HirExpr>,
    },
    MethodCall {
        receiver: Box<HirExpr>,
        method_name: String,
        method_id: DefId,
        args: Vec<HirExpr>,
    },
    Field {
        base: Box<HirExpr>,
        field: String,
        field_index: Option<usize>,
    },
    Index {
        base: Box<HirExpr>,
        index: Box<HirExpr>,
    },
    Tuple(Vec<HirExpr>),
    Array(Vec<HirExpr>),
    Repeat {
        elem: Box<HirExpr>,
        count: usize,
    },
    Struct {
        def: DefId,
        fields: Vec<(String, HirExpr)>,
        rest: Option<Box<HirExpr>>,
    },
    If {
        condition: Box<HirExpr>,
        then_branch: Box<HirExpr>,
        else_branch: Option<Box<HirExpr>>,
    },
    Match {
        scrutinee: Box<HirExpr>,
        arms: Vec<HirArm>,
    },
    BinOp {
        op: HirBinOp,
        lhs: Box<HirExpr>,
        rhs: Box<HirExpr>,
    },
    UnaryOp {
        op: HirUnaryOp,
        operand: Box<HirExpr>,
    },
    Assign {
        target: Box<HirExpr>,
        value: Box<HirExpr>,
    },
    Cast {
        expr: Box<HirExpr>,
        target_ty: Ty,
    },
    Return(Option<Box<HirExpr>>),
    Break(Option<Box<HirExpr>>),
    Continue,
    Ref {
        mutable: bool,
        expr: Box<HirExpr>,
    },
    Deref(Box<HirExpr>),
    /// Desugared `?` operator: `match e { Ok(v) => v, Err(e) => return Err(e.into()) }`
    Try(Box<HirExpr>),
    /// Desugared `for` loop over a runtime iterable such as `Vec`.
    ForDesugared {
        iter: Box<HirExpr>,
        binding: HirId,
        body: Box<HirExpr>,
    },
    /// `while cond { body }`
    While {
        condition: Box<HirExpr>,
        body: Box<HirExpr>,
    },
    Loop(Box<HirExpr>),
    /// `errdefer { ... }` scope guard.
    Errdefer(Box<HirExpr>),
    /// `defer { ... }` scope guard.
    Defer(Box<HirExpr>),
    /// Closure with captured bindings.
    Closure {
        params: Vec<HirParam>,
        ret_ty: Ty,
        body: Box<HirExpr>,
        captures: Vec<HirId>,
    },
    /// Async block.
    AsyncBlock(Box<HirExpr>),
    /// `.await`
    Await(Box<HirExpr>),
    /// `unsafe { ... }`
    Unsafe(Box<HirExpr>),
    /// Compile-time range `lo..hi` or `lo..=hi`
    Range {
        lo: Option<Box<HirExpr>>,
        hi: Option<Box<HirExpr>>,
        inclusive: bool,
    },
}

#[derive(Debug, Clone)]
pub enum HirLit {
    Integer(i128),
    Uint(u128),
    Float(f64),
    String(String),
    Char(char),
    Bool(bool),
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirUnaryOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone)]
pub struct HirArm {
    pub id: HirId,
    pub pattern: HirPattern,
    pub guard: Option<HirExpr>,
    pub body: HirExpr,
}

// ─── HIR patterns ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirPattern {
    pub id: HirId,
    pub kind: HirPatternKind,
    pub span: Span,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum HirPatternKind {
    Wildcard,
    Binding {
        id: HirId,
        mutable: bool,
    },
    Lit(HirLit),
    Tuple(Vec<HirPattern>),
    Struct {
        def: DefId,
        fields: Vec<(String, HirPattern)>,
        rest: bool,
    },
    Variant {
        def: DefId,
        args: Vec<HirPattern>,
    },
    Range {
        lo: Box<HirPattern>,
        hi: Box<HirPattern>,
        inclusive: bool,
    },
    Or(Vec<HirPattern>),
    Ref {
        mutable: bool,
        inner: Box<HirPattern>,
    },
    Slice {
        elems: Vec<HirPattern>,
        rest_index: Option<usize>,
    },
}

// ─── HIR statements ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirStmt {
    pub id: HirId,
    pub kind: HirStmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirStmtKind {
    Let {
        binding: HirId,
        mutable: bool,
        ty: Ty,
        init: Option<HirExpr>,
    },
    Expr(HirExpr),
    Errdefer(HirExpr),
    Defer(HirExpr),
    Use(HirUse),
}

// ─── HIR items ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirParam {
    pub id: HirId,
    pub binding: HirId,
    pub mutable: bool,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub struct HirGenericParam {
    pub name: String,
    pub bounds: Vec<Ty>,
    pub default: Option<Ty>,
}

#[derive(Debug, Clone)]
pub struct HirFn {
    pub id: HirId,
    pub def: DefId,
    pub type_params: Vec<HirGenericParam>,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub params: Vec<HirParam>,
    pub ret_ty: Ty,
    pub body: Option<HirExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirConst {
    pub id: HirId,
    pub def: DefId,
    pub ty: Ty,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirStatic {
    pub id: HirId,
    pub def: DefId,
    pub mutable: bool,
    pub ty: Ty,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirTypeAlias {
    pub id: HirId,
    pub def: DefId,
    pub type_params: Vec<HirGenericParam>,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirUse {
    pub id: HirId,
    pub tree: HirUseTree,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirUseTree {
    pub prefix: Vec<String>,
    pub kind: HirUseKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirUseKind {
    Simple,
    Alias(String),
    Glob,
    Nested(Vec<HirUseTree>),
}

#[derive(Debug, Clone)]
pub enum HirAssocItem {
    Method(HirFn),
    TypeAssoc {
        name: String,
        bounds: Vec<Ty>,
        default: Option<Ty>,
        span: Span,
    },
    Const {
        name: String,
        ty: Ty,
        default: Option<HirExpr>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct HirTrait {
    pub id: HirId,
    pub def: DefId,
    pub type_params: Vec<HirGenericParam>,
    pub super_traits: Vec<Ty>,
    pub items: Vec<HirAssocItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirInterface {
    pub id: HirId,
    pub def: DefId,
    pub type_params: Vec<HirGenericParam>,
    pub super_traits: Vec<Ty>,
    pub items: Vec<HirAssocItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirAbility {
    pub id: HirId,
    pub def: DefId,
    pub super_abilities: Vec<Ty>,
    pub items: Vec<HirAssocItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirImplItem {
    Method(HirFn),
    TypeAssoc {
        name: String,
        ty: Ty,
        span: Span,
    },
    Const {
        name: String,
        ty: Ty,
        value: HirExpr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct HirImpl {
    pub id: HirId,
    pub type_params: Vec<HirGenericParam>,
    pub trait_ref: Option<Ty>,
    pub self_ty: Ty,
    pub items: Vec<HirImplItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirMod {
    pub id: HirId,
    pub def: DefId,
    pub body: Option<Box<HirModule>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirStruct {
    pub id: HirId,
    pub def: DefId,
    pub derives: Vec<String>,
    pub type_params: Vec<HirGenericParam>,
    pub fields: Vec<HirField>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirField {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirEnum {
    pub id: HirId,
    pub def: DefId,
    pub derives: Vec<String>,
    pub type_params: Vec<HirGenericParam>,
    pub variants: Vec<HirVariant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirVariant {
    pub def: DefId,
    pub name: String,
    pub fields: Vec<Ty>,
    pub span: Span,
}

// ─── Extern functions ─────────────────────────────────────────────────────────

/// A foreign function declaration — no body, has an ABI string.
#[derive(Debug, Clone)]
pub struct HirExternFn {
    pub def: DefId,
    pub name: String,
    pub abi: String,
    pub params: Vec<HirParam>,
    pub ret_ty: Ty,
    pub span: Span,
}

// ─── HIR module ───────────────────────────────────────────────────────────────

/// The root HIR node for a source file after name resolution.
#[derive(Debug, Clone)]
pub struct HirModule {
    pub file: FileId,
    pub functions: Vec<HirFn>,
    pub consts: Vec<HirConst>,
    pub statics: Vec<HirStatic>,
    pub type_aliases: Vec<HirTypeAlias>,
    pub uses: Vec<HirUse>,
    pub traits: Vec<HirTrait>,
    pub interfaces: Vec<HirInterface>,
    pub abilities: Vec<HirAbility>,
    pub impls: Vec<HirImpl>,
    pub modules: Vec<HirMod>,
    pub structs: Vec<HirStruct>,
    pub enums: Vec<HirEnum>,
    pub extern_fns: Vec<HirExternFn>,
    pub def_names: HashMap<DefId, String>,
    /// Type environment: maps `HirId` to resolved `Ty`.
    pub ty_env: HashMap<HirId, Ty>,
}

impl HirModule {
    pub fn new(file: FileId) -> Self {
        Self {
            file,
            functions: Vec::new(),
            consts: Vec::new(),
            statics: Vec::new(),
            type_aliases: Vec::new(),
            uses: Vec::new(),
            traits: Vec::new(),
            interfaces: Vec::new(),
            abilities: Vec::new(),
            impls: Vec::new(),
            modules: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            extern_fns: Vec::new(),
            def_names: HashMap::new(),
            ty_env: HashMap::new(),
        }
    }
}
