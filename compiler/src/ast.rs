//! Abstract Syntax Tree (AST) for the Daram language.
//!
//! This is the output of the parser and the input to name resolution.
//! It closely mirrors the source syntax.

use crate::source::{FileId, Span};

// ─── Identifiers ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Self {
            name: name.into(),
            span,
        }
    }
}

/// A path like `std::collections::Vec` or just `foo`.
#[derive(Debug, Clone)]
pub struct Path {
    pub segments: Vec<Ident>,
    pub span: Span,
}

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// Named type: `i32`, `String`, `Vec<T>`
    Named {
        path: Path,
        generics: Vec<TypeExpr>,
        span: Span,
    },
    /// Reference: `&T` or `&mut T`
    Ref {
        mutable: bool,
        inner: Box<TypeExpr>,
        span: Span,
    },
    /// Tuple: `(A, B, C)`
    Tuple { elems: Vec<TypeExpr>, span: Span },
    /// Array: `[T; N]`
    Array {
        elem: Box<TypeExpr>,
        len: Box<Expr>,
        span: Span,
    },
    /// Slice: `[T]`
    Slice { elem: Box<TypeExpr>, span: Span },
    /// Function type: `fn(A, B) -> C`
    FnPtr {
        params: Vec<TypeExpr>,
        ret: Option<Box<TypeExpr>>,
        span: Span,
    },
    /// Never type `!`
    Never { span: Span },
    /// Inferred / wildcard `_`
    Infer { span: Span },
    /// `Self`
    SelfType { span: Span },
    /// `dyn Ability`
    DynTrait { ability: Path, span: Span },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. }
            | TypeExpr::Ref { span, .. }
            | TypeExpr::Tuple { span, .. }
            | TypeExpr::Array { span, .. }
            | TypeExpr::Slice { span, .. }
            | TypeExpr::FnPtr { span, .. }
            | TypeExpr::Never { span }
            | TypeExpr::Infer { span }
            | TypeExpr::SelfType { span }
            | TypeExpr::DynTrait { span, .. } => *span,
        }
    }
}

// ─── Patterns ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Pattern {
    /// Wildcard `_`
    Wildcard { span: Span },
    /// Binding `x` or `mut x`
    Ident { mutable: bool, name: Ident },
    /// Literal: `42`, `"hello"`, `true`
    Literal { lit: Literal, span: Span },
    /// Tuple `(a, b, c)`
    Tuple { elems: Vec<Pattern>, span: Span },
    /// Struct destructure `Foo { x, y }`
    Struct {
        path: Path,
        fields: Vec<FieldPattern>,
        rest: bool,
        span: Span,
    },
    /// Enum variant `Some(x)` or `None`
    Variant {
        path: Path,
        args: Vec<Pattern>,
        span: Span,
    },
    /// Range `1..=5`
    Range {
        lo: Box<Pattern>,
        hi: Box<Pattern>,
        inclusive: bool,
        span: Span,
    },
    /// Or-pattern `A | B`
    Or {
        alternatives: Vec<Pattern>,
        span: Span,
    },
    /// Reference `&p` or `&mut p`
    Ref {
        mutable: bool,
        inner: Box<Pattern>,
        span: Span,
    },
    /// Slice pattern `[first, .., last]`
    Slice {
        elems: Vec<Pattern>,
        rest_index: Option<usize>,
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard { span }
            | Pattern::Literal { span, .. }
            | Pattern::Tuple { span, .. }
            | Pattern::Struct { span, .. }
            | Pattern::Variant { span, .. }
            | Pattern::Range { span, .. }
            | Pattern::Or { span, .. }
            | Pattern::Ref { span, .. }
            | Pattern::Slice { span, .. } => *span,
            Pattern::Ident { name, .. } => name.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldPattern {
    pub name: Ident,
    /// If `None`, the field value is bound to a variable with the same name.
    pub pattern: Option<Pattern>,
    pub span: Span,
}

// ─── Literals ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Integer(u128),
    Float(f64),
    String(String),
    Char(char),
    Bool(bool),
    Unit,
}

// ─── Expressions ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    Literal {
        lit: Literal,
        span: Span,
    },
    Path {
        path: Path,
        span: Span,
    },
    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        args: Vec<Expr>,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: Ident,
        span: Span,
    },
    Tuple {
        elems: Vec<Expr>,
        span: Span,
    },
    Array {
        elems: Vec<Expr>,
        span: Span,
    },
    Repeat {
        elem: Box<Expr>,
        count: Box<Expr>,
        span: Span,
    },
    Struct {
        path: Path,
        fields: Vec<FieldInit>,
        rest: Option<Box<Expr>>,
        span: Span,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    While {
        condition: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    WhileLet {
        pattern: Pattern,
        scrutinee: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    For {
        pattern: Pattern,
        iterable: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    Loop {
        body: Box<Expr>,
        span: Span,
    },
    Break {
        value: Option<Box<Expr>>,
        span: Span,
    },
    Continue {
        span: Span,
    },
    Return {
        value: Option<Box<Expr>>,
        span: Span,
    },
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    CompoundAssign {
        op: CompoundOp,
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    Cast {
        expr: Box<Expr>,
        ty: TypeExpr,
        span: Span,
    },
    Try {
        expr: Box<Expr>,
        span: Span,
    },
    Await {
        expr: Box<Expr>,
        span: Span,
    },
    Closure {
        params: Vec<ClosureParam>,
        ret_ty: Option<TypeExpr>,
        body: Box<Expr>,
        span: Span,
    },
    Ref {
        mutable: bool,
        expr: Box<Expr>,
        span: Span,
    },
    Deref {
        expr: Box<Expr>,
        span: Span,
    },
    Range {
        lo: Option<Box<Expr>>,
        hi: Option<Box<Expr>>,
        inclusive: bool,
        span: Span,
    },
    /// `unsafe { ... }`
    Unsafe {
        body: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal { span, .. }
            | Expr::Path { span, .. }
            | Expr::Block { span, .. }
            | Expr::Call { span, .. }
            | Expr::MethodCall { span, .. }
            | Expr::Index { span, .. }
            | Expr::Field { span, .. }
            | Expr::Tuple { span, .. }
            | Expr::Array { span, .. }
            | Expr::Repeat { span, .. }
            | Expr::Struct { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::While { span, .. }
            | Expr::WhileLet { span, .. }
            | Expr::For { span, .. }
            | Expr::Loop { span, .. }
            | Expr::Break { span, .. }
            | Expr::Continue { span }
            | Expr::Return { span, .. }
            | Expr::BinOp { span, .. }
            | Expr::UnaryOp { span, .. }
            | Expr::Assign { span, .. }
            | Expr::CompoundAssign { span, .. }
            | Expr::Cast { span, .. }
            | Expr::Try { span, .. }
            | Expr::Await { span, .. }
            | Expr::Closure { span, .. }
            | Expr::Ref { span, .. }
            | Expr::Deref { span, .. }
            | Expr::Range { span, .. }
            | Expr::Unsafe { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ClosureParam {
    pub pattern: Pattern,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
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
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
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
}

// ─── Statements ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let x: T = expr;` or `let mut x = expr;`
    Let {
        pattern: Pattern,
        ty: Option<TypeExpr>,
        init: Option<Expr>,
        span: Span,
    },
    /// `errdefer { ... }` — runs cleanup only on error paths.
    Errdefer { body: Expr, span: Span },
    /// `defer { ... }` — always runs cleanup at end of scope.
    Defer { body: Expr, span: Span },
    /// Expression statement (with optional semicolon).
    Expr { expr: Expr, has_semi: bool },
    /// `use` declaration inside a block.
    Use { tree: UseTree, span: Span },
}

// ─── Extern block ─────────────────────────────────────────────────────────────

/// `extern "C" { fn foo(x: i32): i32; }`
#[derive(Debug, Clone)]
pub struct ExternBlock {
    pub abi: String,
    pub functions: Vec<ExternFn>,
    pub span: Span,
}

/// A single function declaration inside an `extern` block (no body).
#[derive(Debug, Clone)]
pub struct ExternFn {
    pub visibility: Visibility,
    pub name: Ident,
    pub params: Vec<FnParam>,
    pub ret_ty: Option<TypeExpr>,
    pub span: Span,
}

// ─── Items ────────────────────────────────────────────────────────────────────

/// Top-level or module-level item.
#[derive(Debug, Clone)]
pub enum Item {
    Function(FnDef),
    Struct(StructDef),
    Enum(EnumDef),
    Trait(TraitDef),
    Interface(InterfaceDef),
    Impl(ImplBlock),
    TypeAlias(TypeAlias),
    Ability(AbilityDef),
    Use(UseTree, Span),
    Module(ModuleDef),
    Const(ConstDef),
    Static(StaticDef),
    ExternBlock(ExternBlock),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Function(item) => item.span,
            Item::Struct(item) => item.span,
            Item::Enum(item) => item.span,
            Item::Trait(item) => item.span,
            Item::Interface(item) => item.span,
            Item::Impl(item) => item.span,
            Item::TypeAlias(item) => item.span,
            Item::Ability(item) => item.span,
            Item::Use(_, span) => *span,
            Item::Module(item) => item.span,
            Item::Const(item) => item.span,
            Item::Static(item) => item.span,
            Item::ExternBlock(item) => item.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Visibility {
    pub is_pub: bool,
    pub span: Option<Span>,
}

impl Visibility {
    pub fn private() -> Self {
        Visibility {
            is_pub: false,
            span: None,
        }
    }
    pub fn public(span: Span) -> Self {
        Visibility {
            is_pub: true,
            span: Some(span),
        }
    }
}

// ─── Function definition ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FnDef {
    pub visibility: Visibility,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub name: Ident,
    pub generics: GenericParams,
    pub params: Vec<FnParam>,
    pub ret_ty: Option<TypeExpr>,
    pub where_clause: Vec<WherePredicate>,
    pub body: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnParam {
    pub pattern: Pattern,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

// ─── Struct ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StructDef {
    pub derives: Vec<Path>,
    pub visibility: Visibility,
    pub name: Ident,
    pub generics: GenericParams,
    pub where_clause: Vec<WherePredicate>,
    pub kind: StructKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StructKind {
    /// `struct Foo { x: i32, y: f64 }`
    Fields(Vec<StructField>),
    /// `struct Foo(i32, f64)`
    Tuple(Vec<TupleField>),
    /// `struct Foo;`
    Unit,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub visibility: Visibility,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TupleField {
    pub visibility: Visibility,
    pub ty: TypeExpr,
    pub span: Span,
}

// ─── Enum ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub derives: Vec<Path>,
    pub visibility: Visibility,
    pub name: Ident,
    pub generics: GenericParams,
    pub where_clause: Vec<WherePredicate>,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: Ident,
    pub kind: VariantKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum VariantKind {
    Unit,
    Tuple(Vec<TypeExpr>),
    Struct(Vec<StructField>),
}

// ─── Trait / Interface ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub visibility: Visibility,
    pub name: Ident,
    pub generics: GenericParams,
    pub super_traits: Vec<TypeExpr>,
    pub where_clause: Vec<WherePredicate>,
    pub items: Vec<TraitItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct InterfaceDef {
    pub visibility: Visibility,
    pub name: Ident,
    pub generics: GenericParams,
    pub super_traits: Vec<TypeExpr>,
    pub where_clause: Vec<WherePredicate>,
    pub items: Vec<TraitItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TraitItem {
    Method(FnDef),
    TypeAssoc {
        name: Ident,
        bounds: Vec<TypeExpr>,
        default: Option<TypeExpr>,
        span: Span,
    },
    Const {
        name: Ident,
        ty: TypeExpr,
        default: Option<Expr>,
        span: Span,
    },
}

// ─── Impl block ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ImplBlock {
    pub generics: GenericParams,
    pub trait_ref: Option<TypeExpr>,
    pub self_ty: TypeExpr,
    pub where_clause: Vec<WherePredicate>,
    pub items: Vec<ImplItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ImplItem {
    Method(FnDef),
    TypeAssoc {
        name: Ident,
        ty: TypeExpr,
        span: Span,
    },
    Const(ConstDef),
}

// ─── Ability (Move-style) ─────────────────────────────────────────────────────

/// `ability Copy {}` — declares a type-level permission marker.
#[derive(Debug, Clone)]
pub struct AbilityDef {
    pub visibility: Visibility,
    pub name: Ident,
    pub super_abilities: Vec<Path>,
    pub items: Vec<TraitItem>,
    pub span: Span,
}

// ─── Type alias ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypeAlias {
    pub visibility: Visibility,
    pub name: Ident,
    pub generics: GenericParams,
    pub ty: TypeExpr,
    pub span: Span,
}

// ─── Constants / statics ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConstDef {
    pub visibility: Visibility,
    pub name: Ident,
    pub ty: TypeExpr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StaticDef {
    pub visibility: Visibility,
    pub mutable: bool,
    pub name: Ident,
    pub ty: TypeExpr,
    pub value: Expr,
    pub span: Span,
}

// ─── Module ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ModuleDef {
    pub visibility: Visibility,
    pub name: Ident,
    pub body: Option<Vec<Item>>,
    pub span: Span,
}

// ─── Use declarations ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UseTree {
    pub prefix: Path,
    pub kind: UseTreeKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum UseTreeKind {
    /// `use foo::bar;` — imports the last segment.
    Simple,
    /// `use foo::bar as baz;`
    Alias(Ident),
    /// `use foo::*;`
    Glob,
    /// `use foo::{a, b, c};`
    Nested(Vec<UseTree>),
}

// ─── Generics ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GenericParams {
    pub params: Vec<GenericParam>,
    pub span: Option<Span>,
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Ident,
    pub bounds: Vec<TypeExpr>,
    pub default: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WherePredicate {
    pub ty: TypeExpr,
    pub bounds: Vec<TypeExpr>,
    pub span: Span,
}

// ─── Module (root) ────────────────────────────────────────────────────────────

/// The top-level parse result for a source file.
#[derive(Debug, Clone)]
pub struct Module {
    pub file: FileId,
    pub items: Vec<Item>,
    pub span: Span,
}
