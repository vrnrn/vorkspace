# Contributing

Read `AGENTS.md` before changing the code. Trace the state owner and preserve conversation identity, unfinished drafts, cancellation, and durable storage.

The root Cargo workspace contains `crates/vorkspace-core` and `crates/vorkspace-gpui`. JavaScript source under `host/` owns the remote host; `embedded/` owns the editor and voice audio helpers. Build and verification commands live under `scripts/`. See the [repository map](docs/repository.md) for ownership and test locations.

```sh
npm ci
npm run check
npm run assets
npm run build
```

Run `npm run dev -- --fixture` against isolated development data for interactive checks. Keep measurements and work logs outside the repository. Pair technical documentation changes with their `docs/eli5/` companions. Preserve dependency copyright notices and licenses.
