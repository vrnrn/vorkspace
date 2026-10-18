# Staff Engineering Reasoning & Operating Guide

Keep this guide timeless and principle-based. Record task plans, progress, completed changes, roadmaps, and point-in-time work logs outside the repository.

Work from observable behavior, explicit ownership, and verifiable evidence. Preserve correctness, user continuity, security boundaries, and durable state while removing unnecessary work. Treat every mechanism—whether a cache, worker, queue, or abstraction—as a hypothesis to test against the problem's constraints, not a pattern to repeat by reflex.

---

## 1. Frame the Problem & Trace the System

Before writing code or choosing a mechanism for nontrivial work, establish the causal chain proportional to the change's blast radius:

1. **The experience:** What user action, workload, or system condition triggers the problem? What must the user see, retain, or be able to do?
2. **The invariant:** What contract must hold before, during, and after the transition?
3. **The authority:** Which boundary owns the authoritative state or effect? Which other representations are derived, delayed, partial, or purely decorative?
4. **The failing transition:** Under what ordering, lifecycle overlap, failure mode, or scale does the invariant break?
5. **The root cause:** Which unnecessary work, hidden coupling, ownership ambiguity, or timing assumption produces the failure?
6. **The intervention:** What is the smallest coherent change at the owning boundary that closes the entire causal gap?
7. **The proof:** What deterministic test or measurement demonstrates success, and what observation would falsify the diagnosis?

### Trace the whole path
Follow an operation end to end: from user intent through admission, state mutation, notification, computation, presentation, persistence, and subprocess or network effects. Trace both the happy path and what happens when the operation is interrupted, repeated, superseded, or recovered after a crash.

### Fix at the owning boundary, not the visible symptom
Distinguish where a defect surfaces from the boundary responsible for it. A rendering glitch can originate in identity, scheduling, storage, or process lifecycle ownership. A short patch that masks the symptom while leaving the race intact is worse than a slightly broader fix with one clear owner; conversely, a broad refactor requires evidence that the existing boundary cannot carry the contract.

---

## 2. Make Ownership & Continuity Precise

### Single clear authority per scope
- **Explicit ownership:** Give every authoritative state or side effect one clear owner within its domain. Separate authorities may exist across runtimes (renderer, native core, remote daemon, provider CLI); define their synchronization protocol rather than collapsing everything into a monolithic coordinator.
- **Atomic publication:** Publish a complete, internally consistent state before notifying subscribers. Coalescing or throttling notifications must never make authoritative reads stale.
- **Freshness vs. cadence:** Decouple data freshness from UI presentation frequency so high-frequency producers (streaming tokens, terminal output, resource polls) wake only the consumers whose relevant slice changed.
- **Orthogonal lifecycle states:** Keep visibility, activity, attachment, warm retention, and disposal distinct. Hiding a surface or parking an idle process does not imply discarding its state.
- **Clean ownership transfer:** Make hand-offs atomic and explicit. Once a successor claims a resource or session, the previous owner must never overwrite or clean it up.

### Continuity of user meaning
- **Preserve semantic identity:** Protect what the user is actually working with—session identity, focus, selection, scroll anchor, and unfinished drafts—rather than incidental coordinates or transient DOM nodes.
- **User intent is authoritative:** Deliberate user input takes precedence over automatic behavior, background refreshes, and delayed programmatic events.
- **Zero-defect intermediate frames:** Inspect the entire visible transition. A momentary collapse, flash, scroll jump, replayed animation, or stale frame is a bug even if the final state converges.
- **Invalidation-aware representations:** If a derived representation depends on geometry, ordering, theme, or configuration, define explicitly when it becomes invalid and how it is reconstructed without losing user context.

---

## 3. Concurrency, Durability & Security Boundaries

### Design asynchronous work as a complete lifecycle
Define admission, concurrency, priority, identity, cancellation, publication, failure, and cleanup together:
- **Admission & priority:** Acquire admission before consuming scarce execution resources. Never trap urgent user control actions behind optional background work.
- **Epoch & identity validation:** Carry enough version, epoch, and owner identity to verify whether an in-flight result still belongs to the current owner and input. Re-validate at every `await` boundary and immediately before publication—being valid when a task started never guarantees it is valid when it finishes.
- **Cancellation semantics:** Distinguish abandonment from continuation. Specify what cancellation guarantees: preventing publication, removing queued jobs, interrupting active subprocesses, or releasing locks. Retain reusable checkpoints only at stable boundaries where both input and interpretation remain compatible.
- **Failure recovery:** Every failure path must release locks, clean up temporary resources, and leave the owner ready for subsequent work.

### Protect durable truth from partial views
- **Distinct absence states:** Missing, unknown, unavailable, empty, and explicitly deleted carry different meanings; never conflate them in storage or wire schemas.
- **Safe partial reads:** A displayed or memory-bounded view may intentionally omit large payloads. Protect omitted data at the persistence boundary so a partial read followed by a write merges against durable state rather than clobbering unloaded fields.
- **Root-last atomic commits:** When persisting or migrating multi-file or multi-table state (transcripts, rollouts, subagent edges, checkpoints), write through flushed temporary files (`fsync` + atomic `rename`) and publish the resumable root marker last so readers never observe a half-written state.
- **Honest external uncertainty:** A local call, timeout, or disconnect does not prove whether an external process or remote host executed an effect once, zero times, or partially. Use durable submission receipts and idempotency keys rather than blind replay.

### Enforce trust and isolation boundaries
- **Boundary validation:** Validate untrusted identifiers, paths, and external payloads at the trust boundary before constructing filesystem paths, SQL queries, or subprocess arguments.
- **Safe filesystem & process primitives:** Enforce project-root containment on file mutations where applicable, create temporary or credential files exclusively with owner-only permissions (`O_EXCL` / `0600`), isolate child processes in dedicated process groups or job objects, and keep untrusted HTML/artifact previews inside opaque sandboxed origins without IPC access.

---

## 4. Performance & Resource Economics

### Remove work before moving it
First determine whether work is necessary at all. Only then narrow its scope, reuse compatible results, defer it, or move it off the interactive thread:
- Update only subscribers whose observed slice changed.
- Avoid rebuilding unchanged history, re-flattening static trees, or copying whole transcripts for incremental appends.
- Separate essential interactive readiness from optional decorative enrichment (such as syntax highlighting or link previews) while preserving recovery ordering.
- Account for the full cost of any cache, worker, or queue—including serialization, coordination, invalidation, and retained memory. Moving work changes *where* it runs; it does not automatically reduce total cost.

### Budget the resource that actually matters
- Bound the real bottleneck: retained bytes, DOM/layout weight, active child processes, IPC broadcast fan-out, queued payloads, and reconstruction latency—not merely entry counts. Label estimates clearly as estimates.
- Retain warm state when measured latency gains justify its footprint, and evict inactive, reconstructible state through its owner while protecting active turns and unsaved user input. Prevent a single oversized item from evicting the entire hot working set.
- Separate resource observation from the authority to act: a pressure sample can surface diagnostics or park idle caches, but destructive termination requires re-validating current ownership at the point of action.

---

## 5. Verification, Edge Transitions & Documentation Hygiene

### Match evidence to the claim
Exercise the changed contract directly with deterministic tests and representative measurements:
- **Reuse & caching:** Compare against a straightforward reference across mutated inputs, invalidations, and configuration switches; verify both output equivalence and work skipped.
- **Notifications & state:** Verify both reduced wakeups and immediately consistent authoritative reads.
- **Async lifecycles:** Deterministically control completion order, cancellation, stale publication, replacement, failure, and crash recovery.
- **Partial data & durability:** Exercise the full read → modify → save → reopen/resume cycle, including interrupted writes.
- **Performance:** Measure against a clear baseline with repeated samples across every process or thread that pays the cost.

Control scheduling whenever timing is not the property under test. Never "fix" an unexplained test failure by weakening an assertion or inflating a timeout. Keep claims strictly within the evidence: functional correctness does not prove latency; fewer renders do not prove lower memory; component microbenchmarks do not prove end-to-end responsiveness.

### Stress the transitions between states
Explicitly review overlapping transitions derived from the ownership model: active ↔ idle, current ↔ superseded, warm ↔ reconstructed, normal ↔ resource-pressured, partial ↔ complete, local ↔ remote, and steady operation overlapping with navigation, resize, cancellation, or save. Extend an existing owner before introducing a competing mechanism, and preserve the rationale behind non-obvious invariants so future simplification does not silently remove a safety guarantee.

### Keep documentation grounded and paired with ELI5 companions
- **Ground in live truth:** Every document in `docs/` must reflect the actual features, commands, flags, file paths, and architecture of the current codebase.
- **No changelogs or work logs:** Keep only timeless product and architecture documentation in the repository. Never commit `CHANGELOG.md`, upstream import logs, QA notes, or point-in-time work logs; fold lasting architectural or product changes directly into the relevant docs.
- **1-to-1 ELI5 companions:** Every technical document at `docs/<name>.md` must have a matching plain-English companion at `docs/eli5/<name>.md`. Whenever you add, update, or remove a document in `docs/`, create, update, or remove its `docs/eli5/<name>.md` counterpart in the same change.
