---
id: DR-0001
title: The engine has zero runtime dependencies, forever
status: enforced
invariant: zero-deps
---

The `[dependencies]` table of the engine crate stays empty. Permanently. This is not
minimalism as an aesthetic — it is the product.

An embeddable engine is chosen by people who are putting someone else's code inside their own
binary. Every crate we depend on is a crate they now depend on, in their audit, their supply
chain, their build times, their `cargo-deny` policy, and their next CVE notification. "Zero
dependencies" is a sentence that ends procurement conversations, and no convenience crate is
worth re-opening them.

The cost is real and accepted: we write our own regex engine, our own float formatting
(Ryū/Grisu by hand), our own Unicode tables. Those are weeks of work each. They are also,
every one of them, work that a conformance suite can verify objectively — which is exactly the
kind of work this project is set up to absorb.

`lab/` is explicitly exempt: experiments may depend on anything, because nothing there ships
and the engine may never import it.

If you believe the engine needs a crate, that belief is the signal to stop and write the
argument down here as a superseding record — not to add the line and see if anyone notices.
