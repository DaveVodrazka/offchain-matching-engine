# Off-chain Matching Engine

An off-chain matching engine for a CLOB DEX, written in Rust, with on-chain settlement. An AMM is added later as a second liquidity source.

## Purpose

**This is a learning project, not a product.** The goal is to deeply understand how off-chain matching engines work. That means the data structures, the matching algorithm, sequencing, risk, and the trust boundary between the off-chain operator and the on-chain contract.

What follows from that:

- **Understanding over speed.** Nothing gets built until its purpose and its role in the system are fully understood. Slow progress is fine. Code that nobody understands is not.
- **Architecture and best practices at all times.** Every piece should be done the way a production system would do it, even when a shortcut would work.
- **Full ownership.** The author writes all core code. An AI assistant reviews it, explains concepts, and fills in only non-educational code: scaffolding, boilerplate, tooling.
- **One lesson, one discrete piece.** The project is structured like a university course. Each lesson builds a single component that depends only on earlier lessons.

## Working agreement

Each lesson follows the same loop:

1. **Concept.** What the piece is and why it exists.
2. **Role.** Where it sits in the system and what depends on it.
3. **Decisions.** The design trade-offs, which the author decides.
4. **Build.** The author writes the core code.
5. **Review.** Review rounds until the code is correct and idiomatic.
6. **Filler.** The assistant adds non-educational code, with the author's OK.

## Architecture (target)

```
            ┌──────────── API gateway (REST/WS) ────────────┐
            │  signed orders in, market data + fills out    │
            └──────────────┬────────────────────────────────┘
                           ▼
   Auth (sig verify, nonce, expiry) → Risk/Ledger (avail vs locked)
                           ▼
              Sequencer (single writer, command log/WAL)
                           ▼
           Matching engine (pure state machine, per market)
                           ▼  events: Fill, Cancel, BookDelta
           ┌───────────────┼──────────────────┐
           ▼               ▼                  ▼
     Market data     Settlement batcher   Persistence (snapshots)
                           ▼
                  [SettlementGateway trait]  ◄── FakeChain / devnet / testnet
                           ▲
                  Chain indexer (deposits, withdrawals, settled, reorgs) → Ledger
```

Principles:

- **Hexagonal (ports & adapters).** The core is pure, with no IO, no clock, and no async. Chains, networks and storage sit behind traits.
- **Deterministic engine.** `apply(Command) -> Vec<Event>`, so state equals the replay of the command log.
- **No floats.** Prices and quantities are integer ticks and lots, and amounts use checked `u128` arithmetic.

### Chain mocking strategy

| Tier | What | Use for |
|---|---|---|
| 0 | In-memory `FakeChain` with fault injection | Unit tests, property tests, simulation (the main tool) |
| 1 | Local devnet (anvil / starknet-devnet / katana) | Integration tests against real contracts |
| 2 | Public testnet | Final demo only |

## Decisions log

| Date | Decision | Rationale |
|---|---|---|
| 2026-10-11 | Start with a CLOB, add AMM later as a liquidity source | Off-chain matching is natural for a CLOB, and the AMM extends it cleanly |
| 2026-10-11 | Integer ticks and lots, not `rust_decimal` | The chain has no decimals, comparison is one instruction, and off-grid values can't be represented. `Decimal` is used only at the API edge |
| 2026-10-11 | `u64` ticks and lots, `u128` atomic amounts | `u64 × u64` can exceed `u64` |
| 2026-10-11 | `Price` has no arithmetic | A spread is a tick distance, not a price (YAGNI) |
| 2026-10-11 | `Qty` is non-zero; `checked_sub` returns `Ok(None)` for a full fill and `Err` for underflow | A full fill is a normal outcome, not an error |
| 2026-10-11 | Release builds keep `overflow-checks = true` | Silent wrapping in money code is worse than a panic |
| 2026-10-14 | `OrderId` is a tag newtype with a public `OrderId::new(u64)`, and `0` is allowed | It prevents mixing IDs with other `u64`s, not forgery: an unknown ID fails at the book lookup. The sequencer (resume) and the API (cancel) must build IDs from raw `u64`s. Revisit if `0` causes problems |
| 2026-10-14 | `OrderIdGenerator` has no `Default`/`new()`, only `fresh()` and `resume_after(last_issued)` | Restarting from scratch after a crash would reissue IDs, so the call site has to choose explicitly. Persistence stays outside `domain`, and the state is injected |
| 2026-10-14 | `QuoteAmount`/`BaseAmount` have `pub(crate)` constructors | Only `MarketSpec` mints amounts. Other crates can read them but not create them (revisit in Lesson 8 for deposits) |
| 2026-10-14 | `domain` exposes a flat API through `lib.rs` re-exports, with private modules | The file layout stays an internal detail, and `unreachable_pub` polices the API surface |
| 2026-10-14 | `MarketSpec::new` will take `base: Asset, quote: Asset` (Lesson 9). No `MarketParams` or decimals newtypes now | Swapped decimals silently give a wrong K (10^12×). `Asset` carries its own decimals. Tick and lot can still be swapped (K is symmetric, `base_amount` is wrong), so test the getters |
| 2026-10-14 | CI runs on push to every branch (tags ignored), with no `pull_request` trigger | Solo repo with no forks: branches get tested without opening a PR, and there are no duplicate runs. Add `pull_request` back if outside contributors join |
| 2026-10-14 | `rust-toolchain.toml` is the only place the toolchain version is set; CI reads it via `setup-rust-toolchain` | A version pinned in several places drifts apart |
| 2026-10-14 | CI uses `--locked`, `--all-targets` and `-D warnings` | The lockfile must not change without anyone noticing, lints apply to tests too, and a warning fails the build |
| 2026-10-14 | Cancel in O(1) using a slab with an intrusive doubly linked list per level | 90% or more of venue traffic is cancels. Unlinking a node is O(1) with no tombstones, so walking a level never has to skip dead orders. This is the core learning goal of the project |
| 2026-10-14 | Price levels are kept in a `BTreeMap<Price, Level>` per side; the `OrderId` index is a `HashMap` used for lookups only | Deterministic ordered iteration with O(log L), where the number of levels L is small. The index is never iterated, so `HashMap`'s random order can't break replay |
| 2026-10-14 | The book lives in `crates/engine` as a `book` module | Matching (Lesson 4) goes in the same crate, so the book's internals can stay `pub(crate)` |
| 2026-10-14 | `rest()` checks that the order doesn't cross the spread with `debug_assert!` only | Crossing is the engine's job. Tests catch engine bugs, release builds pay nothing, and the Lesson 5 property tests are the real guarantee |
| 2026-10-14 | Write our own generic `Slab<T>` with generational keys `SlotKey { index: u32, generation: u32 }`. Generations use `wrapping_add`. `get`/`get_mut`/`remove` return `Result<_, SlabError>` | It's for learning, and `slotmap` serves as the reference. Generations turn a stale key from silent cross-user corruption into an error. `u32` keeps the key at 8 bytes and doesn't depend on the platform. A false match after wrapping needs a stale key that survives 2³² reuses of one slot. The slab knows nothing about orders |
| 2026-10-18 | Slab lookups return `Result` with a single `SlabError::StaleSlotKey`, not `Option` | Inside the engine every key comes from the `OrderId` index or level links, so a stale key always means a bug (an unknown `OrderId` fails earlier, at the `HashMap`). The error documents the failure. Genuine stale keys only ever fail the generation check, so one variant is enough |
| 2026-10-18 | `insert` is infallible: broken internal invariants use `unreachable!`, and index-space exhaustion (`len == u32::MAX`) panics | The caller can't cause or fix slab self-corruption, so it shouldn't have to handle it. Exhaustion is a capacity limit, and fail-stop matches `overflow-checks = true`. A `debug_assert!` on the push path (empty free list ⇒ `len == vec.len()`) is the proof that `to_u32` never truncates |
| 2026-10-18 | The slab tracks `len` as a counter, and `len`/`is_empty`/`contains` exist even though the book can use `index.len()` | It's a generic container, and tests and invariant checks need it. O(1) |
| — | Chain choice (EVM vs Starknet): **deferred to Lesson 9** | Not needed until signed orders |

## Lessons

- [x] **0. What a CLOB DEX is.** Skipped, since the author already knows CLOB basics.
- [x] **1. Domain primitives.** `Side`, `Price`, `Qty`, `OrderId`, `QuoteAmount`, `BaseAmount`, `MarketSpec`, `DomainError`
  - [x] `Side` with `opposite()`
  - [x] `Price` newtype, non-zero, no arithmetic
  - [x] `Qty` newtype, non-zero, `checked_sub` with full-fill semantics
  - [x] Rename `QuantityOverflow` to `QuantityUnderflow`
  - [x] `DomainError` via `thiserror` (`Display`, `Error`, `PartialEq`)
  - [x] Tests: exact fill, underflow, `try_new` boundaries for `Price` and `Qty`; `Side`; `OrderIdGenerator` (sequence, resume, exhaustion)
  - [x] Clean up clippy warnings (`#[must_use]`, `# Errors` docs, semicolons)
  - [x] `OrderId` (sequencer-assigned `u64`)
  - [x] `QuoteAmount` / `BaseAmount` constructors (`pub(crate)`) and accessors
  - [x] `MarketSpec::new`, which rejects zero sizes and a non-integral tick × lot
  - [x] `MarketSpec::notional` with checked `u128` math
  - [x] `MarketSpec::base_amount`
  - [x] Tests: the ETH/USDC worked example, overflow, invalid market config
  - [x] Split `domain` into modules (`error`, `side`, `grid`, `amount`, `order_id`, `market_spec`)
  - [x] Visibility pass: field privacy, `pub(crate)` constructors, `lib.rs` re-exports
  - [x] `OrderIdGenerator`: `fresh()` and `resume_after()` for replay
  - [x] Polish: `const fn` constructors, error message casing, comments, a scoped `#[allow]`
- [x] **2. Workspace and tooling.** Cargo workspace, lints, toolchain pin, git
  - [x] Workspace `Cargo.toml` with shared package settings and dependencies
  - [x] Workspace lints (`pedantic`, `arithmetic_side_effects`, no `unwrap`/`panic`, `forbid(unsafe)`)
  - [x] Release `overflow-checks = true`
  - [x] `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`
  - [x] `git init` at the root
  - [x] Remove the nested `crates/domain/.git`
  - [x] First commit
  - [x] CI (fmt, clippy, test, doc): `.github/workflows/test.yaml`
- [ ] **3. Order book data structure.** Price levels, a FIFO queue per level, O(1) cancel
  - [x] Concept, role, top-level decisions (A–D)
  - [x] 3a. Slab: stable-index storage, free list, stale keys (25 tests, including a model-based churn test)
  - [ ] 3b. Level: intrusive doubly linked FIFO list over the slab, cached total
  - [ ] 3c. Side: `BTreeMap<Price, Level>`, best price, bid/ask ordering
  - [ ] 3d. `OrderBook`: `OrderId` index, `rest`/`cancel`/`peek_best`/`fill_front` API, tests
- [ ] **4. Matching algorithm.** `apply(Command) -> Vec<Event>`, limit and market orders
- [ ] **5. Invariants and property testing.** proptest suite for the book (no crossed book, quantity conservation)
- [ ] **6. Advanced order types.** IOC, FOK, post-only, self-trade prevention
- [ ] **7. Sequencer and event sourcing.** Command log, replay, snapshots
- [ ] **8. Ledger and risk.** Available, locked and pending balances
- [ ] **9. Signed orders.** EIP-712/SNIP-12 verification, nonces, expiry (chain choice made here). `Asset` type (address + decimals) replaces the raw decimals in `MarketSpec::new`
- [ ] **10. Settlement port and FakeChain.** `SettlementGateway` trait, an in-memory chain with fault injection
- [ ] **11. Settlement batcher.** Batching, retries, rollback on revert
- [ ] **12. Smart contract.** Vault, `settle(batch)`, cancel by nonce, forced withdrawal
- [ ] **13. Devnet adapter and indexer.** Real chain adapter, event watching, reorg handling
- [ ] **14. API and market data.** REST and WebSocket (axum), L2 deltas, trades, fills
- [ ] **15. Simulation and benchmarks.** Market maker and taker bots, criterion, flamegraphs
- [ ] **16. AMM hybrid.** Pool as a liquidity source, routing between the book and the pool
- [ ] **17. Testnet deploy.** Final demo

## Current status

**Lessons 1 and 2 are complete** (2026-10-14): 42 tests pass, and CI (fmt, clippy, test, doc) is green on `master`.

**Lesson 3a (slab) is complete** (2026-10-18).

**Next session:**
1. **Lesson 3b: Level.** An intrusive doubly linked FIFO list over the slab, with a cached total. Open question from 3a: does unlinking need `get2_mut`, or is sequential `get_mut` enough? Remove the `#[cfg_attr(not(test), expect(dead_code))]` on `book::slab` once Level uses it.

**Small leftovers (optional):**
- Public items have `# Errors`/`# Panics` sections but no one-line doc summaries.
- Flatten the `let k = if … else { match … }` block in `MarketSpec::new`.
