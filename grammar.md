# Daram Grammar

This document describes the current source grammar accepted by the Daram lexer and parser in this repository.

It is written from the implementation, not from aspirational design notes. If this file disagrees with the parser, the parser wins.

## Quick Tour

If you just want to read Daram code without parsing the full formal grammar, start here.

### 1. Functions look like this

```daram
fun add(a: i32, b: i32): i32 {
    a + b
}
```

- `fun` declares a function.
- Parameters use `name: Type`.
- The last expression can be the return value.
- `fn` is also accepted, but `fun` is the preferred style.

### 2. Variables and constants

```daram
let value = 10;
const limit: i32 = 100;
```

- `let` creates a local binding.
- `const` creates an immutable local or top-level constant.
- Types can be written explicitly or inferred.

### 3. Structs and enums

```daram
struct User {
    name: string,
    age: i32,
}

enum Option<T> {
    Some(T),
    None,
}
```

- `struct` defines record-like data.
- `enum` defines tagged variants.
- Generics use `<T>`.

### 4. Control flow

```daram
if score > 90 {
    "A"
} else {
    "B"
}
```

```daram
match value {
    0 => "zero",
    1..=9 => "small",
    _ => "many",
}
```

- `if`, `while`, `for`, and `loop` all use block syntax.
- `match` is the main pattern-matching form.

### 5. Imports

```daram
import println from "std/io";
import { read_file, write_file as write } from "std/fs";
```

- Modern Daram style uses `import ... from ...`.
- Legacy `use foo::bar` syntax is still accepted.

### 6. What the language feels like

Daram reads roughly like this:

- TypeScript-style surface readability
- Rust-style type and ownership ambitions
- Expression-oriented blocks and pattern matching
- File-based modules instead of inline `mod { ... }`

## Most Common Syntax

These are the forms most people will hit first.

### Functions

```daram
fun greet(name: string): string {
    "Hello, " + name
}
```

### Methods

```daram
extend User {
    fun is_adult(self): bool {
        self.age >= 18
    }
}
```

### Arrays and tuples

```daram
let numbers = [1, 2, 3];
let pair = ("x", 10);
```

### Struct literals

```daram
let user = User {
    name: "Ari",
    age: 20,
};
```

Shorthand field init is also accepted:

```daram
let user = User { name, age };
```

### References

```daram
fun inspect(value: &i32): i32 {
    *value
}
```

### Closures

```daram
let double = fun(x: i32): i32 {
    x * 2
};
```

### Defer

```daram
defer {
    println("leaving scope");
}
```

`defer` always runs at scope exit. `errdefer` only runs on error paths.

## Conventions

- `A?` means "optional".
- `A*` means "zero or more".
- `A+` means "one or more".
- Alternatives are written with `|`.
- Literal tokens are written in quotes.
- Some lexer-level details are described in prose where EBNF would be noisy.

## Lexical Structure

### Whitespace and Comments

- Whitespace is ignored.
- Line comments use `//`.
- Block comments use `/* ... */`.
- Block comments may nest.

### Identifiers

```ebnf
identifier = ( letter | "_" ) , { letter | digit | "_" } ;
```

The parser also accepts a few contextual path segments in path position:

- `self`
- `super`
- `crate`
- `from` in a few import-related positions

### Literals

```ebnf
integer-literal =
    decimal-integer
  | "0x" , hex-digit , { hex-digit | "_" }
  | "0o" , oct-digit , { oct-digit | "_" }
  | "0b" , bin-digit , { bin-digit | "_" } ;

float-literal =
    decimal-integer , "." , digit , { digit | "_" } , exponent?
  | decimal-integer , exponent ;

exponent = ( "e" | "E" ) , ( "+" | "-" )? , digit , { digit | "_" } ;

string-literal = '"' , { string-char } , '"' ;
char-literal = "'" , char-char , "'" ;
bool-literal = "true" | "false" ;
unit-literal = "(" , ")" ;
```

Supported string escapes:

- `\\`
- `\"`
- `\'`
- `\n`
- `\r`
- `\t`
- `\0`
- `\u{...}`

### Keywords

The lexer recognizes these keywords:

`let`, `mut`, `fn`, `fun`, `return`, `if`, `else`, `for`, `in`, `while`, `loop`, `break`, `continue`, `struct`, `enum`, `const`, `static`, `impl`, `extend`, `trait`, `interface`, `implements`, `match`, `as`, `use`, `import`, `export`, `from`, `mod`, `pub`, `self`, `super`, `crate`, `type`, `async`, `await`, `unsafe`, `errdefer`, `defer`, `where`, `ability`, `capability`, `move`, `extern`, `dyn`

`capability` is reserved by the lexer but is not a standalone parsed item in the current frontend grammar.

### Punctuation and Operators

```text
+  -  *  /  %  ^  &  |  ~
&& || ! ? = == != < <= > >=
<< >> += -= *= /= %= &= |= ^= <<= >>=
-> => .. ..= . ::
( ) { } [ ] , ; : @ #
```

## File Grammar

```ebnf
module = { item } ;
```

Inline `mod { ... }` items are rejected. Module structure is file-based.

## Attributes and Visibility

```ebnf
derive-attr = "@derive" , "(" , path , { "," , path } , ")" ;

visibility = "pub" | "export" ;
```

Only `@derive(...)` is accepted today, and only on `struct` and `enum` items.

## Items

```ebnf
item =
    function-item
  | struct-item
  | enum-item
  | const-item
  | static-item
  | trait-item
  | interface-item
  | impl-item
  | type-alias-item
  | ability-item
  | use-item
  | import-item
  | extern-block ;
```

### Functions

```ebnf
function-item =
    derive-attr* ,
    visibility? ,
    "async"? ,
    "unsafe"? ,
    ( "fun" | "fn" ) ,
    identifier ,
    generic-params? ,
    "(" , function-param-list? , ")" ,
    return-type? ,
    where-clause? ,
    ( block-expr | ";" ) ;

function-param-list = function-param , { "," , function-param } , ","? ;

function-param =
    pattern , ":" , type-expr , default-value? ;

default-value = "=" , expr ;

return-type = ( ":" | "->" ) , type-expr ;
```

Notes:

- `fun` is the preferred spelling, but `fn` is still accepted.
- A method receiver may omit its type when written as `self`.
- Default argument values are accepted in item-level function declarations.

### Structs

```ebnf
struct-item =
    derive-attr* ,
    visibility? ,
    "struct" ,
    identifier ,
    generic-params? ,
    where-clause? ,
    (
        ";"
      | tuple-struct-fields , ";"?
      | struct-fields
      | unit-struct-elision
    ) ;

tuple-struct-fields =
    "(" , tuple-struct-field , { "," , tuple-struct-field } , ","? , ")" ;

tuple-struct-field = visibility? , type-expr ;

struct-fields =
    "{" , struct-field , { ","? , struct-field } , ","? , "}" ;

struct-field = visibility? , identifier , ":" , type-expr ;

unit-struct-elision = empty ;
```

A unit struct may be written either as `struct Name;` or simply `struct Name` before the next item or EOF.

### Enums

```ebnf
enum-item =
    derive-attr* ,
    visibility? ,
    "enum" ,
    identifier ,
    generic-params? ,
    where-clause? ,
    "{" , enum-variant , { ","? , enum-variant } , ","? , "}" ;

enum-variant =
    identifier ,
    (
        tuple-variant
      | struct-variant
      | empty
    ) ;

tuple-variant =
    "(" , type-expr , { "," , type-expr } , ","? , ")" ;

struct-variant =
    "{" , variant-field , { ","? , variant-field } , ","? , "}" ;

variant-field = identifier , ":" , type-expr ;
```

### Constants and Statics

```ebnf
const-item =
    visibility? ,
    "const" ,
    identifier ,
    ":" , type-expr ,
    "=" , expr ,
    ";" ;

static-item =
    visibility? ,
    "static" ,
    "mut"? ,
    identifier ,
    ":" , type-expr ,
    "=" , expr ,
    ";" ;
```

### Traits and Interfaces

```ebnf
trait-item =
    visibility? ,
    "trait" ,
    identifier ,
    generic-params? ,
    super-traits? ,
    where-clause? ,
    "{" , { trait-member } , "}" ;

interface-item =
    visibility? ,
    "interface" ,
    identifier ,
    generic-params? ,
    super-traits? ,
    where-clause? ,
    "{" , { trait-member } , "}" ;

super-traits = ":" , type-expr , { "+" , type-expr } ;

trait-member =
    visibility? ,
    (
        function-item-without-leading-attrs
      | assoc-type-decl
      | assoc-const-decl
    ) ;

function-item-without-leading-attrs =
    "async"? ,
    "unsafe"? ,
    ( "fun" | "fn" ) ,
    identifier ,
    generic-params? ,
    "(" , function-param-list? , ")" ,
    return-type? ,
    where-clause? ,
    ( block-expr | ";" ) ;

assoc-type-decl =
    "type" ,
    identifier ,
    assoc-type-bounds? ,
    assoc-type-default? ,
    ";" ;

assoc-type-bounds = ":" , type-expr , { "+" , type-expr } ;
assoc-type-default = "=" , type-expr ;

assoc-const-decl =
    "const" ,
    identifier ,
    ":" , type-expr ,
    ( "=" , expr )? ,
    ";" ;
```

### `impl` / `extend`

```ebnf
impl-item =
    ( "impl" | "extend" ) ,
    generic-params? ,
    type-expr ,
    (
        "implements" , type-expr
      | "for" , type-expr
      | empty
    ) ,
    where-clause? ,
    "{" , { impl-member } , "}" ;

impl-member =
    visibility? ,
    (
        function-item-without-leading-attrs
      | impl-type-item
      | const-item
    ) ;

impl-type-item =
    "type" ,
    identifier ,
    "=" , type-expr ,
    ";" ;
```

Interpretation:

- `extend SelfType { ... }` is an inherent impl.
- `extend SelfType implements Trait { ... }` is the preferred trait impl spelling.
- `impl Trait for SelfType { ... }` is also accepted.

### Type Aliases

```ebnf
type-alias-item =
    visibility? ,
    "type" ,
    identifier ,
    generic-params? ,
    "=" , type-expr ,
    ";" ;
```

### Abilities

```ebnf
ability-item =
    visibility? ,
    "ability" ,
    identifier ,
    ability-supers? ,
    (
        ";" |
        "{" , { trait-member } , "}"
    ) ;

ability-supers = ":" , path , { "+" , path } ;
```

### Imports

Two import families are accepted.

Legacy `use` grammar:

```ebnf
use-item = "use" , use-tree , ";" ;

use-tree =
    path ,
    (
        "::" , "*"
      | "::" , "{" , use-tree , { "," , use-tree } , ","? , "}"
      | "as" , identifier
      | empty
    ) ;
```

Modern `import` grammar:

```ebnf
import-item = "import" , import-tree , ";" ;

import-tree =
    "*" , "as" , identifier , "from" , import-source
  | "{" , import-binding , { "," , import-binding } , ","? , "}" , "from" , import-source
  | identifier , "from" , import-source ;

import-binding = identifier , ( "as" , identifier )? ;

import-source = string-literal | path ;
```

Examples:

```daram
use std::io;
use std::fmt::{Debug, Display as Show};
use std::collections::*;

import println from "std/io";
import { read_file, write_file as write } from "std/fs";
import * as json from json_extra;
```

### Extern Blocks

```ebnf
extern-block =
    "extern" ,
    string-literal? ,
    "{" , { extern-function } , "}" ;

extern-function =
    visibility? ,
    ( "fun" | "fn" ) ,
    identifier ,
    "(" , extern-param-list? , ")" ,
    return-type? ,
    ";" ;

extern-param-list =
    extern-param , { "," , extern-param } , ","? ;

extern-param = pattern , ":" , type-expr ;
```

If the ABI string is omitted, the parser defaults it to `"C"`.

## Generic Parameters and Where Clauses

```ebnf
generic-params =
    "<" , generic-param , { "," , generic-param } , ","? , ">" ;

generic-param =
    identifier ,
    generic-bounds? ,
    generic-default? ;

generic-bounds = ":" , type-expr , { "+" , type-expr } ;
generic-default = "=" , type-expr ;

where-clause =
    "where" , where-predicate , { "," , where-predicate } , ","? ;

where-predicate =
    type-expr ,
    ( ":" , type-expr , { "+" , type-expr } )? ;
```

## Types

```ebnf
type-expr =
    named-type
  | ref-type
  | tuple-type
  | array-type
  | slice-type
  | fn-type
  | never-type
  | infer-type
  | self-type
  | dyn-type ;

named-type = type-path , generic-args? ;
type-path = type-path-segment , { "::" , type-path-segment } ;
type-path-segment = identifier | "crate" | "super" ;
generic-args = "<" , type-expr , { "," , type-expr } , ","? , ">" ;

ref-type = "&" , "mut"? , type-expr ;
tuple-type = "(" , type-expr-list? , ")" ;
type-expr-list = type-expr , { "," , type-expr } , ","? ;
array-type = "[" , type-expr , ";" , expr , "]" ;
slice-type = "[" , type-expr , "]" ;
fn-type = ( "fun" | "fn" ) , "(" , type-expr-list? , ")" , return-type? ;
never-type = "!" ;
infer-type = "_" ;
self-type = "self" ;
dyn-type = "dyn" , path ;
```

Notes:

- Named types may start with an identifier, `crate`, or `super`.

## Patterns

```ebnf
pattern =
    or-pattern ;

or-pattern =
    range-pattern , { "|" , range-pattern } ;

range-pattern =
    atomic-pattern ,
    (
        ".." , atomic-pattern
      | "..=" , atomic-pattern
      | empty
    ) ;

atomic-pattern =
    "_"
  | identifier-pattern
  | literal-pattern
  | ref-pattern
  | tuple-pattern
  | slice-pattern
  | struct-pattern
  | variant-pattern
  | path-pattern ;

identifier-pattern =
    "mut"? , identifier
  | "self"
  | "mut" , "self" ;

literal-pattern =
    integer-literal
  | float-literal
  | string-literal
  | char-literal
  | bool-literal
  | unit-literal ;

ref-pattern = "&" , "mut"? , pattern ;

tuple-pattern = "(" , pattern-list? , ")" ;
pattern-list = pattern , { "," , pattern } , ","? ;

slice-pattern =
    "[" ,
    (
        slice-entry , { "," , slice-entry } , ","?
    )? ,
    "]" ;

slice-entry = pattern | ".." ;

struct-pattern =
    path ,
    "{" ,
    (
        struct-pattern-field , { "," , struct-pattern-field } , ","?
    )? ,
    ( ","? , ".." )? ,
    "}" ;

struct-pattern-field = identifier , ( ":" , pattern )? ;

variant-pattern =
    path ,
    "(" , pattern-list? , ")" ;

path-pattern = path ;
```

Semantics:

- A single-segment path pattern is treated as a binding unless it has tuple/struct destructuring syntax.
- A multi-segment bare path pattern is treated as a variant-like path pattern.

## Statements

```ebnf
stmt =
    let-stmt
  | const-let-stmt
  | defer-stmt
  | errdefer-stmt
  | use-stmt
  | import-stmt
  | expr-stmt ;

let-stmt =
    "let" ,
    "mut"? ,
    pattern ,
    ( ":" , type-expr )? ,
    ( "=" , expr )? ,
    ";" ;

const-let-stmt =
    "const" ,
    pattern ,
    ( ":" , type-expr )? ,
    ( "=" , expr )? ,
    ";" ;

defer-stmt = "defer" , block-expr ;
errdefer-stmt = "errdefer" , block-expr ;

use-stmt = use-item ;
import-stmt = import-item ;

expr-stmt = expr , ";"? ;
```

Notes:

- Inside blocks, `const` behaves like an immutable local binding statement, not a top-level `const` item.
- The parser currently marks `let` bindings as mutable in pattern form, even when `mut` is omitted. That is parser behavior, not a style recommendation.

## Expressions

```ebnf
expr = assign-expr ;

assign-expr =
    range-expr ,
    (
        "="
      | "+=" | "-=" | "*=" | "/=" | "%="
      | "&=" | "|=" | "^=" | "<<=" | ">>="
    ) , assign-expr
  | range-expr ;

range-expr =
    or-expr ,
    (
        ".." , or-expr?
      | "..=" , or-expr?
      | empty
    ) ;

or-expr = and-expr , { "||" , and-expr } ;
and-expr = cmp-expr , { "&&" , cmp-expr } ;
cmp-expr = bitor-expr , ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) , bitor-expr | bitor-expr ;
bitor-expr = bitxor-expr , { "|" , bitxor-expr } ;
bitxor-expr = bitand-expr , { "^" , bitand-expr } ;
bitand-expr = shift-expr , { "&" , shift-expr } ;
shift-expr = add-expr , { ( "<<" | ">>" ) , add-expr } ;
add-expr = mul-expr , { ( "+" | "-" ) , mul-expr } ;
mul-expr = cast-expr , { ( "*" | "/" | "%" ) , cast-expr } ;
cast-expr = unary-expr , { "as" , type-expr } ;

unary-expr =
    ( "-" | "!" | "~" ) , unary-expr
  | "&" , "mut"? , unary-expr
  | "*" , unary-expr
  | postfix-expr ;

postfix-expr =
    primary-expr ,
    {
        "." , identifier
      | "." , integer-literal
      | "(" , call-args? , ")"
      | "[" , expr , "]"
      | "?"
      | "await"
    } ;

call-args = expr , { "," , expr } , ","? ;
```

### Primary Expressions

```ebnf
primary-expr =
    literal-expr
  | path-expr
  | tuple-or-group-expr
  | array-expr
  | block-expr
  | if-expr
  | match-expr
  | while-expr
  | for-expr
  | loop-expr
  | closure-expr
  | return-expr
  | break-expr
  | continue-expr
  | unsafe-expr
  | async-block-expr
  | struct-literal ;
```

#### Blocks

```ebnf
block-expr = "{" , { stmt } , tail-expr? , "}" ;
tail-expr = expr ;
```

#### Grouping, Tuples, and Arrays

```ebnf
tuple-or-group-expr =
    "(" , ")"
  | "(" , expr , ")"
  | "(" , expr , "," , expr , { "," , expr } , ","? , ")" ;

array-expr =
    "[" , "]"
  | "[" , expr , ";" , expr , "]"
  | "[" , expr , { "," , expr } , ","? , "]" ;
```

#### Paths and Struct Literals

```ebnf
path-expr = path ;

struct-literal =
    path ,
    "{" ,
    (
        field-init , { "," , field-init } , ","?
    )? ,
    ( ","? , ".." , expr )? ,
    "}" ;

field-init = identifier , ( ":" , expr )? ;
```

Shorthand field init is accepted:

```daram
Point { x, y }
```

#### Control Flow

```ebnf
if-expr =
    "if" , expr , block-expr ,
    ( "else" , ( if-expr | block-expr ) )? ;

match-expr =
    "match" , expr ,
    "{" , match-arm , { ","? , match-arm } , ","? , "}" ;

match-arm =
    pattern ,
    ( "if" , expr )? ,
    "=>" , expr ;

while-expr =
    "while" ,
    (
        "let" , pattern , "=" , expr
      | expr
    ) ,
    block-expr ;

for-expr =
    "for" , pattern , "in" , expr , block-expr ;

loop-expr =
    "loop" , block-expr ;
```

#### Closures

```ebnf
closure-expr =
    ( "fun" | "fn" ) ,
    "(" , closure-param-list? , ")" ,
    return-type? ,
    block-expr ;

closure-param-list =
    closure-param , { "," , closure-param } , ","? ;

closure-param =
    pattern ,
    ( ":" , type-expr )? ;
```

#### Early Exit and Unsafe

```ebnf
return-expr = "return" , expr? ;
break-expr = "break" , expr? ;
continue-expr = "continue" ;
unsafe-expr = "unsafe" , block-expr ;
async-block-expr = "async" , block-expr ;
```

## Paths

```ebnf
path = path-segment , { "::" , path-segment } ;

path-segment =
    identifier
  | "self"
  | "super"
  | "crate"
  | "from" ;
```

## Current Parser Notes

- `mod { ... }` blocks are intentionally rejected.
- `fun` and `fn` are both accepted in declarations, function types, and closure syntax.
- Both `:` and `->` are accepted for return types in several places.
- `import` is the modern import surface; `use` is still accepted.
- `extend ... implements ...` is the modern impl surface; `impl Trait for Self` is still accepted.
- `async { ... }` currently parses as a block-shaped primary expression. Its runtime meaning is narrower than the syntax may suggest.
- `capability` is reserved but not a complete grammar feature in this parser.

## Source of Truth

If you need to verify or update this file, check these implementation files first:

- `compiler/src/lexer.rs`
- `compiler/src/parser.rs`
- `compiler/src/ast.rs`
