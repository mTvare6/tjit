# tjit

An experimental strongly typed systems language with a JIT compiler written in Rust using [Cranelift](https://en.wikipedia.org/wiki/Cranelift).

## Features

- Lexer, parser, and static type checker lowering to Cranelift IR with a typed AST HIR
- Algebraic data types (struct + enums with payload)
- Pattern matching (ranges, destructuring, wildcards)
- Bit-packing for arbitrary-width integers (`u13`, `i42`) packed to maximize L1 cache density and promoted during execution.
- Fixed size arrays
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

**Pipeline operator**
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


## TODO
- [ ] Heap FFI (`alloc` / `free`)
- [ ] Affine type system (move semantics)
- [ ] RAII (Drop heap allocation at the end of scope)
- [ ] Borrow checker with mutability XOR aliasing via custom MIR (non-cranelift one) lowering and non-lexical lifetimes.
