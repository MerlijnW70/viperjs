# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) — with the pre-1.0 caveat that the
public API is not stable and may change in any release.

## [Unreleased]

Working towards **M2 — the parser**. The engine does not execute JavaScript yet; what exists is
a complete lexer and a parser covering most of the modern grammar.

### Added

- **Lexer (M1, complete).** Every token form ECMA-262 §12 defines: identifiers over the real
  Unicode `ID_Start`/`ID_Continue` sets, all numeric literals including Annex B's legacy forms
  and BigInt, strings and templates as UTF-16 code units, regular-expression literals under a
  goal symbol supplied by the parser, and all four line terminators. Every token knows its exact
  span, and the token spans plus the trivia between them reconstruct the source byte for byte.
- **Parser.** Statements and declarations, the full expression grammar with correct precedence
  and associativity, automatic semicolon insertion, functions, classes with fields, static blocks
  and private names, generators, `async`/`await`, arrow functions, destructuring and the three
  cover grammars, template literals, optional chaining, `import()`/`import.meta`, and modules
  with their `import` and `export` declarations — each with the early errors the spec demands.
- **Static semantics** over the finished tree: declared names and their collisions, label
  resolution, and the strict-mode rules.
- `examples/parse.rs` — parses a single file or sweeps a repository, grouping failures by kind.

### Guarantees held throughout

- Zero runtime dependencies; the dependency table is empty and checked in CI.
- `#![forbid(unsafe_code)]`, crate-wide.
- No input panics. Nesting is bounded by an explicit count rather than by hitting the OS stack
  guard, and a full-depth parse is asserted to survive one mebibyte of stack.

[Unreleased]: https://github.com/MerlijnW70/praxis/commits/master
