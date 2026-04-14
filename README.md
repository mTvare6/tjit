# tjit

A minimal JIT compiled language written in Rust using [Cranelift](https://en.wikipedia.org/wiki/Cranelift).

## Features

- Lexer, parser, and static type checker
- Typed AST compiled using cranelift
- Algebraic data types (struct + enums with payload)
- Pattern matching (ranges, destructuring, wildcards)
- Fixed size arrays
- Arbitrary width integers and struct bitfields (`u13`, `i42`)
- Pipeline operator (`|>`)
- Libc FFI for I/O

## Example

**Functions**
```rs
fn add(a: i64, b: i64) -> i64 {
  a + b
}

add(2, 3)
```

**ADTs**
```rs
struct Point {
  x: i64,
  y: i64,
}

enum Event {
  Click(Point),
  Quit,
}
```

**Pattern matching**
```rs
let val = 15
let r = match val {
  0..10 => 0,
  10..=20 => 1,
  _ => 2,
}
```

**Arrays**
```rs
let arr: [i64; 4] = [10, 20, 30, 40]
let x = arr[2]
```

**Arbitrary width integers**
```rs
struct BitPack {
  is_active: u1,
  day_of_week: u3,
  count: i17,
}
```

**UFCS**
```rs
fn add(a: i64, b: i64) -> i64 {
  a + b
}

let x = 10 |> add(5)
```

# Building

```sh
cargo build --release
```

# Running

```sh
cargo run --release -- <file.tjit>
```

# Usage

```sh
tjit <filename.tjit>
```
