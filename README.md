# Off-chain Matching Engine

A deterministic limit order book matching engine for a single market, written in Rust.

The engine takes commands (limit order, market order, cancel), matches them against a
price-time priority order book, and reports every outcome as an event. It has no clock,
no randomness and no IO: replaying the same command log on a fresh engine always produces
the same events and the same book.

## Features

- **Limit orders**: match against the opposite side up to the limit price; any unfilled
  remainder rests on the book.
- **Market orders**: match at any price; any unfilled remainder expires and never rests.
- **Cancel**: removes a resting order. Cancelling an unknown, already cancelled or fully
  filled order is rejected without changing anything.
- **Price-time priority**: best price first, oldest order first within a price level.
  Trades execute at the maker's price.
- **Sequential fill ids**: each fill gets a unique, increasing id so downstream settlement
  can deduplicate fills.

## Usage

```rust
use domain::{OrderId, Price, Qty, Side};
use engine::{Command, Engine, Event};

let mut engine = Engine::new();

let _ = engine.apply(Command::Limit {
    id: OrderId::new(1),
    side: Side::Ask,
    price: Price::try_new(100)?,
    qty: Qty::try_new(5)?,
});

let events = engine.apply(Command::Market {
    id: OrderId::new(2),
    side: Side::Bid,
    qty: Qty::try_new(3)?,
});
// [Event::Fill { maker: 1, taker: 2, taker_side: Bid, filled: 3, price: 100, fill_id: 1 }]
```

Order ids are assigned upstream (by a sequencer) and passed in with each command.

## Commands and events

`Engine::apply(Command) -> Vec<Event>` is the whole public interface. Every command
produces at least one event, in this order: fills (best price first, oldest maker first),
then the taker's remainder, if any.

| Event       | Meaning                                                       | Book change        |
|-------------|---------------------------------------------------------------|--------------------|
| `Fill`      | Trade between one resting maker and the incoming taker        | maker qty reduced  |
| `Rested`    | Unfilled part of a limit order added to the book              | `+qty` at price    |
| `Cancelled` | Resting order removed; carries its unfilled qty               | `-qty` at price    |
| `Expired`   | Unfilled remainder of a taker that never rested (market order) | none               |
| `Rejected`  | Command not applied (e.g. unknown order on cancel)            | none               |

Each taker order ends in exactly one way: fully filled, filled plus a `Rested` remainder,
or filled plus an `Expired` remainder. Its fills plus the remainder always equal its
quantity. Events are self-contained, so a consumer never needs to query the engine.

## Architecture

The project is a Cargo workspace with two crates.

```
crates/
├── domain/   value types shared by the system
└── engine/   matching engine and order book
```

### `domain`

Validated, integer-only value types. No floating point anywhere.

- `Price` and `Qty`: non-zero `u64` values in ticks and lots.
- `Side` (`Bid` / `Ask`), `OrderId`, `FillId` and their sequential id generators
  (which can resume from a checkpoint).
- `MarketSpec`: market parameters (base/quote decimals, tick size, lot size). Validates
  that `tick × lot` is an integer in quote units, and converts grid values into token
  amounts (`QuoteAmount` notional, `BaseAmount`) using `u128` with overflow checks.

### `engine`

```
Engine
 ├── FillIdGenerator
 └── OrderBook
      ├── index: HashMap<OrderId, (side, price, slot key)>
      ├── slab:  Slab<Node>          shared storage for all resting orders
      ├── asks:  BookSide            BTreeMap<Price, Level>, lowest first
      └── bids:  BookSide            BTreeMap<Price, Level>, highest first
```

- **`Engine`**: applies commands. Runs the matching loop (one maker per step until the
  taker is filled, the opposite side is empty, or the next price is past the limit) and
  emits events.
- **`OrderBook`**: both sides plus an index from `OrderId` to each order's location, so
  cancel is a direct lookup. The index and the book are kept in sync by every mutation.
- **`BookSide`**: price levels in a `BTreeMap`. Best price is the first/last key. Empty
  levels are removed immediately.
- **`Level`**: a FIFO queue of orders at one price, implemented as an intrusive doubly
  linked list whose nodes live in the slab. Supports O(1) push to back, fill from front,
  and unlink from anywhere (for cancel). Tracks order count and total quantity.
- **`Slab`**: a generational arena (`Vec` of slots with a free list). Keys carry a
  generation counter, so a stale key to a reused slot is detected instead of silently
  reading the wrong order.

### Safety

- `unsafe` code is forbidden workspace-wide.
- All arithmetic that could overflow is explicit (`checked_*`), enforced by
  `clippy::arithmetic_side_effects`; `unwrap`, `expect` and `panic` are linted.
- Overflow checks stay on in release builds.
- Invalid input (unknown order id on cancel) is a normal `Rejected` event. Internal
  inconsistencies (e.g. duplicate order id from upstream, corrupted book state) panic
  rather than continue with bad state.

## Development

Requires Rust 1.91.1 (pinned in `rust-toolchain.toml`).

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

CI runs fmt, clippy, tests and docs on every push.
