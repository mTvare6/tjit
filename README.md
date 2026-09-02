# tjit

An experimental strongly typed systems language with a JIT compiler written in Rust using [Cranelift](https://en.wikipedia.org/wiki/Cranelift) with a static type checker.

## Features

- ADTs and Arrays
- Pattern matching (ranges, destructuring, wildcards)
- Bit-packing for arbitrary-width (<64) integers (`u13`, `i42`) packed to maximise cache efficiency.
- Pipeline operator (`|>`) as a form of UFCS.
- Libc FFI for IO

## Showcase

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
  - [ ] RAII (and drop traits?)
  - [ ] Borrow checker with mutability ^ aliasing (and a MIR for that) and [NLLs](https://rust-lang.github.io/rfcs/2094-nll.html).
