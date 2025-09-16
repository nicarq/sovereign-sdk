## LIGETRON_UNSAFE_JOURNAL_FALLBACK

* **Why that name?**
  `LIGETRON_UNSAFE_JOURNAL_FALLBACK` is an **opt‑in escape hatch** for bring‑up/demos. It’s *unsafe* because it lets the host **guess** a “journal” from program output when the guest didn’t explicitly emit one via the secure contract. That guess is **not enforced inside the circuit**, so the proof may attest to something other than the value you think it does.

* **Why is it unsafe?**
  It breaks the essential “**binding**” between (a) what the program actually proves inside the zk circuit and (b) the **public journal** we publish and verify against. With the fallback enabled, the journal can come from string scraping or even a host‑generated hash of stdout, which the circuit has no obligation to compute or check. This opens correctness and potential confidentiality issues.

* **How does it work (mechanically)?**
  The secure path requires the guest to print exactly:

  ```
  SOV_JOURNAL_HEX:<lowercase-hex>
  ```

  The host reads those bytes as the journal, computes `sha256(journal)`, passes that digest as **arg\[1]**, and the *guest checks inside the circuit* that its own `sha256(journal)` equals **arg\[1]** (second pass).
  With the fallback **enabled**, if the line above is **missing**, the host tries a series of **heuristics** to fabricate a journal:

  * look for lines like `Prover root: <64-hex>`,
  * scan for any 64‑char hex/`0x…` token,
  * parse `Result: <number>`, JSON‑ish arrays, etc.,
  * and if none found, **hash a slice of stdout/stderr plus some context** to synthesize a “journal”.

  The host then computes a digest of that fabricated journal and reruns the prover with that digest. If the zk program **doesn’t** check the digest inside the circuit, the second pass still succeeds—despite the digest having no relation to the real computation.

---

## Deeper dive

### What “binding” means in this design

The secure design has a crisp statement:

> *There exists an execution trace of program `P` such that the circuit verifies and the guest computed `sha256(journal)` and proved it equals the public input `arg[1]`; therefore the published `journal` is exactly what `P` computed.*

This holds only when the guest **calls the journal API and enforces the digest in‑circuit** (e.g., via `sov_commit_and_check` in `sov_journal.h`).

### What goes wrong with the fallback

When you enable the fallback, you’re changing the statement to:

> *There exists an execution trace of `P` with **some** public input `arg[1]` for which the circuit verifies. Separately, the host scraped some bytes from stdout/stderr or even synthesized bytes and called that the “journal”.*

Those bytes may **not be equal** to anything computed in the circuit, and often can be **chosen by the program author** (or influenced by environment), which defeats the meaning of the “journal”.

#### Concrete failure modes

1. **Arbitrary hex injection**
   A program prints two hex strings; the heuristic grabs the first one (which could be arbitrary), while the real result is the second. The proof verifies, but the journal you publish is **not** the computed result.

2. **Unconstrained digest**
   If the guest/circuit never checks `arg[1] == sha256(journal)` internally, the second pass is vacuous: any digest works, so the host can staple any “journal” to a valid proof.

3. **Stale/irrelevant roots**
   “Prover root”/“Merkle root” lines might be **intermediate** or **diagnostic** values, not the semantic result you intend to expose.

4. **Confidentiality leaks**
   Heuristics might capture numbers or hex that inadvertently reveal **private hints** printed during debugging, turning secrets into public journal data.

5. **Synthesis path**
   The “execution‑context” fallback hashes slices of stdout/stderr, part of the WASM, and the **length** of hints. That yields a deterministic value but has **no semantic meaning** and can be influenced by trivial print order changes.

---

## Why keep it at all?

* **Developer ergonomics during bring‑up:**
  It lets you test the plumbing with third‑party WASM that hasn’t been instrumented with `sov_journal.h` yet.
* **Demos:**
  You can show an end‑to‑end flow before the guest adopts the contract.

But it should be **off by default** and never used in production or in any test that aims to check soundness.

---

## Recommended operating policy

* **Default:** **disabled**. Require `SOV_JOURNAL_HEX:`.
* **Enable only** by setting the env var **explicitly** for local demos/tests:

  ```
  LIGETRON_UNSAFE_JOURNAL_FALLBACK=1
  ```
* **Loud logging when used:** emit a WARNING so it’s obvious in CI logs if someone accidentally relied on it.
* **Security reviews/tests** must run with it **off**.

---

## Mental model: secure vs. fallback

| Aspect         | Secure path (default)                                | Fallback path (opt‑in)                             |
| -------------- | ---------------------------------------------------- | -------------------------------------------------- |
| Journal source | Exact `SOV_JOURNAL_HEX:<hex>` line                   | Heuristics / host‑synthesized                      |
| Circuit check  | Guest recomputes `sha256(journal)` & equals `arg[1]` | Often **no** in‑circuit relation                   |
| Property       | **Binding**: published journal is what was proved    | **No binding**: published journal may be arbitrary |
| Safety         | **Sound**                                            | **Unsafe** (correctness + possible leaks)          |

---

## One‑line justification for the name

* **`LIGETRON_`** — scoped to this adapter.
* **`UNSAFE`** — it weakens the proof’s security semantics.
* **`JOURNAL_FALLBACK`** — only kicks in when the real journal is missing.

That’s why we chose a name that’s impossible to miss in code reviews and CI logs.
