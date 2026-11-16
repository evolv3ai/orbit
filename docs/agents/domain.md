# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

Layout: **single-context**. This is a Bun workspace (`apps/server`, `apps/web`, `apps/mobile`,
`packages/*`), but it is one product with one vocabulary.

## Before exploring, read these

- **`.ai/`**: Orbit's existing agent context. Start at `.ai/README.md`, which gives the read order
  (`PROJECT_OVERVIEW.md`, `ARCHITECTURE.md`, `FEATURES.md`, `DEVELOPMENT.md`, `UI_CONVENTIONS.md`,
  `lessons.md`).
- **`GLOSSARY.md`** at the repo root.
- **`docs/adr/`**: read ADRs that touch the area you're about to work in.
- **`docs/specs/`**: dated design specs. For the ONLYOFFICE work, read
  `docs/specs/2026-11-16-onlyoffice-integration-design.md`.

If `GLOSSARY.md` or `docs/adr/` doesn't exist, **proceed silently**. Don't flag their absence; don't
suggest creating them upfront. The `/domain-modeling` skill (reached via `/grill-with-docs` and
`/improve-codebase-architecture`) creates them lazily when terms or decisions actually get resolved.

## File structure

```
/
├── .ai/                 ← existing agent context (upstream)
├── GLOSSARY.md          ← created lazily
├── docs/
│   ├── adr/             ← created lazily
│   ├── agents/          ← this setup
│   └── specs/           ← dated design specs
├── apps/  (server, web, mobile)
└── packages/  (api, database, editor)
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test
name), use the term as defined in `GLOSSARY.md`, or in `.ai/` where the glossary is silent. Don't drift
to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal: either you're inventing language
the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR or spec, surface it explicitly rather than silently
overriding:

> _Contradicts ADR-0007 (event-sourced orders), but worth reopening because…_

## Fork note

`.ai/`, `AGENTS.md` and most of `docs/` come from upstream `coollabsio/orbit`. Put fork-only material
(`docs/agents/`, specs, ADRs for fork features) in new files where possible, so merges from upstream
stay clean.
