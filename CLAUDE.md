# Penguin — agent guide

Fast, local-first, keyboard-first email client. Gmail/Workspace first, macOS first, no server. Read `docs/ARCHITECTURE.md` before touching code: it holds the command contract and the file-ownership map.

## Layout
- `crates/penguin-core`: types, SQLite+FTS5 store, search query parser. No networking, no Tauri.
- `crates/penguin-provider`: the `MailProvider`/`Backend` seam every mail provider implements (compose, outbox, snooze live here). Provider agents start from `docs/PROVIDERS-IMPL.md`.
- `crates/penguin-gmail`: OAuth (loopback + PKCE), Keychain tokens, Gmail REST, sync engine, MIME compose.
- `crates/penguin-render`: email HTML sanitizer.
- `crates/penguin-eval`: dev-only search relevance harness (synthetic mailbox, 570 judged queries incl. edge cases in docs/SEARCH-CASES.md, before/after diffs). Judge every ranking change with it: `docs/SEARCH-EVAL.md`.
- `apps/desktop`: Tauri 2 app. `src-tauri/` is thin command glue; `src/` is React + Vite. The UI talks to the backend only through `src/lib/api.ts`.
- `design/`: static mockups and the `penguin.css` design system (source of visual truth).

## Commands
- Rust: `source ~/.cargo/env && cargo check --workspace`, `cargo test --workspace`
- On a shared Linux build box, run cargo through `scripts/box-cargo.sh` (at most two builds at once; first run `scripts/box-tauri-sysroot.sh` for the desktop crate), and delete a worktree's `target/` once its branch is merged.
- UI alone with mock data: `cd apps/desktop && npm run dev:mock` → http://localhost:1420
- Full app: `cd apps/desktop && npm run tauri dev`

## Scope
- Penguin is a macOS app. There is no mobile app.

## Rules
- Never commit OAuth client JSON, tokens, `.env`, or mailbox data. Never log tokens or message bodies.
- Fixtures and mock data use fictional people and `.example` domains only.
- Keep `types.rs` ⇄ `types.ts` in lockstep.
- Treat email HTML as hostile. It is rendered only through penguin-render plus the sandboxed iframe.
- Speed budget: interactions read local state only. Never block the UI on the network.
