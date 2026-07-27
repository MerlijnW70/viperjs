---
id: DR-0005
title: AST nodes own their data; spans are diagnostics, not data
status: prose-only
---

Every AST node carries a [`Span`], and every node owns whatever it means. Those are two separate
decisions that are easy to conflate, and conflating them is how a syntax tree ends up unable to
outlive the text it came from.

**Nodes own their data.** A numeric literal holds an `f64`, a string literal holds the `Vec<u16>`
its code units make up, a regular expression holds its pattern and flags as `String`s. None of it
borrows the source. The alternative — storing spans and slicing the source whenever a value is
wanted — is tempting because the lexer already computed the extents and the copies look
redundant. It has two costs that are not obvious until later. The tree becomes lifetime-bound to
a `&str`, which means the compiler, any cache, and any tooling that wants to hand a tree around
all inherit that constraint. And the value would be *recomputed* at each use, so a literal's
meaning would depend on code running twice and agreeing — which is exactly the property the
lexer's own value functions exist to guarantee once.

**Spans are diagnostics.** They are on every node so that an error can point at the construct
that caused it, and for nothing else. No stage may recover a node's meaning from its span; a tree
whose spans were all zeroed must still compile to the same bytecode, only with worse error
messages. That rule is what keeps the ownership decision honest — the moment a compiler reads a
span to find out what a node *is*, the tree is borrowing the source again by another name.

The cost is allocation: a program with ten thousand string literals allocates ten thousand
vectors. That is a performance problem, which is the kind this project prefers to have
(GOAL.md §1). Interning, small-string optimisation, and an arena for the tree itself are all M8
experiments with benchmarks in front of them, and none of them changes this decision — they
change where the bytes live, not who owns them.

Recursive parts of the grammar will box their children when they arrive. An index-based arena
would pack better and is the usual answer at scale, but it makes every traversal indirect and
every early-error rule harder to read, and the tree is discarded as soon as bytecode exists. If a
benchmark later says the allocation matters, that is the point at which to change it, and the
lab is where to find out.
