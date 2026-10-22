use std::collections::HashMap;

use domain::{OrderId, Price, Qty, Side};

use crate::book::{
    book_side::{BestFill, BookSide},
    level::Node,
    slab::{Slab, SlotKey},
};

/// Where a resting order lives: the side and price find its level, and the key finds its node.
#[derive(Debug)]
struct IndexValue {
    side: Side,
    price: Price,
    slot_key: SlotKey,
}

/// The resting orders of one market: both sides, the node storage they share, and an index
/// from `OrderId` to each order's location.
///
/// Invariant: the index and the book hold the same orders. Every resting order has exactly
/// one index entry, and every entry points at that order's live node. Every mutating method
/// keeps the two in sync.
///
/// This is the only book layer that takes an `OrderId`, so an unknown id is handled here as
/// normal input. Below this layer, a bad location is a bug and panics.
#[derive(Debug)]
pub(crate) struct OrderBook {
    asks: BookSide,
    bids: BookSide,
    index: HashMap<OrderId, IndexValue>,
    slab: Slab<Node>,
}

impl OrderBook {
    pub(crate) fn new() -> Self {
        Self {
            asks: BookSide::new(Side::Ask),
            bids: BookSide::new(Side::Bid),
            index: HashMap::new(),
            slab: Slab::new(),
        }
    }

    /// The lowest ask or the highest bid, or `None` if that side is empty.
    pub(crate) fn best_price(&self, side: Side) -> Option<Price> {
        match side {
            Side::Ask => self.asks.best_price(),
            Side::Bid => self.bids.best_price(),
        }
    }

    /// Removes the resting order `id` from the book and returns it, with its remaining
    /// quantity.
    ///
    /// Returns `None` if `id` isn't resting: it never existed, was already cancelled, or was
    /// fully filled. A cancel racing a fill is normal market traffic, not an error.
    pub(crate) fn cancel(&mut self, id: OrderId) -> Option<Node> {
        let IndexValue {
            side,
            price,
            slot_key,
        } = self.index.remove(&id)?;
        Some(match side {
            Side::Ask => self.asks.cancel(&mut self.slab, price, slot_key),
            Side::Bid => self.bids.cancel(&mut self.slab, price, slot_key),
        })
    }

    /// Adds an order to the back of the queue at `price` on `side` and indexes it.
    ///
    /// The caller must have matched the order first: a resting order must not cross the
    /// opposite best price.
    ///
    /// # Panics
    ///
    /// Panics if `id` is already resting, before anything is changed. Ids come from the
    /// sequencer, so a duplicate is a bug. In debug builds, also panics if the order would
    /// cross the book.
    pub(crate) fn rest(&mut self, side: Side, id: OrderId, price: Price, qty: Qty) {
        assert!(!self.index.contains_key(&id), "duplicate order id {id:?}");
        let slot_key = match side {
            Side::Ask => {
                debug_assert!(
                    self.bids
                        .best_price()
                        .is_none_or(|best_bid| best_bid < price),
                    "resting ask would cross the book",
                );
                self.asks.rest(&mut self.slab, price, id, qty)
            }
            Side::Bid => {
                debug_assert!(
                    self.asks
                        .best_price()
                        .is_none_or(|best_ask| best_ask > price),
                    "resting bid would cross the book",
                );
                self.bids.rest(&mut self.slab, price, id, qty)
            }
        };
        self.index.insert(
            id,
            IndexValue {
                side,
                price,
                slot_key,
            },
        );
    }

    /// Fills the head order of the best level on `side`, up to `wanted`. A fully filled
    /// maker leaves the index. Returns `None` if `side` is empty.
    ///
    /// Fills one maker per call. The caller loops and decides when to stop: at the taker's
    /// limit price, when the taker is filled, or when the side runs out.
    pub(crate) fn fill_best(&mut self, side: Side, wanted: Qty) -> Option<BestFill> {
        let fill = match side {
            Side::Ask => self.asks.fill_best(&mut self.slab, wanted),
            Side::Bid => self.bids.fill_best(&mut self.slab, wanted),
        }?;
        if fill.remaining.is_none() {
            self.index.remove(&fill.maker);
        }
        Some(fill)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};

    use super::*;

    fn price(ticks: u64) -> Price {
        Price::try_new(ticks).expect("test price must be non-zero")
    }

    fn qty(lots: u64) -> Qty {
        Qty::try_new(lots).expect("test qty must be non-zero")
    }

    fn id(raw: u64) -> OrderId {
        OrderId::new(raw)
    }

    /// Builds a book from `(side, price, id, lots)` orders rested in order.
    fn book_with(orders: &[(Side, u64, u64, u64)]) -> OrderBook {
        let mut ob = OrderBook::new();
        for &(side, ticks, raw_id, lots) in orders {
            ob.rest(side, id(raw_id), price(ticks), qty(lots));
            assert_invariants(&ob);
        }
        ob
    }

    /// Fills against a side that must have liquidity.
    fn fill(ob: &mut OrderBook, side: Side, wanted: u64) -> BestFill {
        let fill = ob.fill_best(side, qty(wanted)).expect("side has liquidity");
        assert_invariants(ob);
        fill
    }

    /// `(price, maker, filled, remaining)` as raw numbers, for compact assertions.
    fn summary(fill: &BestFill) -> (u64, u64, u64, Option<u64>) {
        (
            fill.price.get(),
            fill.maker.get(),
            fill.filled.get(),
            fill.remaining.map(Qty::get),
        )
    }

    fn assert_invariants(ob: &OrderBook) {
        let index_len_u32 = u32::try_from(ob.index.len()).expect("index len conversion failed");
        assert!(
            index_len_u32 == ob.slab.len(),
            "index and slab must hold same amount of items"
        );
        assert!(
            index_len_u32 == ob.asks.len().checked_add(ob.bids.len()).unwrap(),
            "index must hold same amount of items as both sides"
        );
        if let (Some(best_ask), Some(best_bid)) = (ob.asks.best_price(), ob.bids.best_price()) {
            assert!(
                best_bid < best_ask,
                "book crossed: bid {best_bid:?} >= ask {best_ask:?}"
            );
        }

        for (id, entry) in &ob.index {
            let node = ob
                .slab
                .get(entry.slot_key)
                .expect("index entry has a stale slot key");
            assert_eq!(node.id(), *id, "index entry points at another order's node");
        }

        let asks_in_index = ob.index.values().filter(|e| e.side == Side::Ask).count();
        assert_eq!(
            u32::try_from(asks_in_index).unwrap(),
            ob.asks.len(),
            "misplaced order - asks check"
        );
        let bids_in_index = ob.index.values().filter(|e| e.side == Side::Bid).count();
        assert_eq!(
            u32::try_from(bids_in_index).unwrap(),
            ob.bids.len(),
            "misplaced order - bids check"
        );
    }

    // --- Construction ---

    #[test]
    fn new_book_is_empty() {
        let mut ob = OrderBook::new();
        for side in [Side::Ask, Side::Bid] {
            assert_eq!(ob.best_price(side), None);
            assert!(ob.fill_best(side, qty(1)).is_none());
        }
        assert!(ob.cancel(id(1)).is_none());
        assert_invariants(&ob);
    }

    // --- Rest ---

    #[test]
    fn rest_goes_to_its_own_side() {
        let ob = book_with(&[(Side::Ask, 101, 1, 5), (Side::Bid, 99, 2, 5)]);
        assert_eq!(ob.best_price(Side::Ask), Some(price(101)));
        assert_eq!(ob.best_price(Side::Bid), Some(price(99)));
        assert_eq!((ob.asks.len(), ob.bids.len()), (1, 1));
    }

    #[test]
    fn best_price_is_lowest_ask_and_highest_bid() {
        let ob = book_with(&[
            (Side::Ask, 103, 1, 5),
            (Side::Ask, 101, 2, 5),
            (Side::Bid, 97, 3, 5),
            (Side::Bid, 99, 4, 5),
        ]);
        assert_eq!(ob.best_price(Side::Ask), Some(price(101)));
        assert_eq!(ob.best_price(Side::Bid), Some(price(99)));
    }

    #[test]
    fn rest_one_tick_from_the_opposite_best_is_allowed() {
        let ob = book_with(&[(Side::Bid, 100, 1, 5), (Side::Ask, 101, 2, 5)]);
        assert_eq!(ob.best_price(Side::Ask), Some(price(101)));
    }

    #[test]
    #[should_panic(expected = "duplicate order id")]
    fn rest_with_duplicate_id_panics() {
        let mut ob = book_with(&[(Side::Ask, 101, 1, 5)]);
        ob.rest(Side::Bid, id(1), price(99), qty(5));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "resting ask would cross the book")]
    fn rest_ask_at_best_bid_panics() {
        let mut ob = book_with(&[(Side::Bid, 100, 1, 5)]);
        ob.rest(Side::Ask, id(2), price(100), qty(5));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "resting bid would cross the book")]
    fn rest_bid_at_best_ask_panics() {
        let mut ob = book_with(&[(Side::Ask, 100, 1, 5)]);
        ob.rest(Side::Bid, id(2), price(100), qty(5));
    }

    // --- Cancel ---

    #[test]
    fn cancel_returns_the_order_and_removes_it() {
        let mut ob = book_with(&[
            (Side::Ask, 101, 1, 5),
            (Side::Ask, 102, 2, 7),
            (Side::Bid, 99, 3, 4),
        ]);
        let node = ob.cancel(id(1)).expect("order is resting");
        assert_invariants(&ob);
        assert_eq!((node.id(), node.qty()), (id(1), qty(5)));
        assert_eq!(ob.best_price(Side::Ask), Some(price(102)));

        let node = ob.cancel(id(3)).expect("order is resting");
        assert_invariants(&ob);
        assert_eq!((node.id(), node.qty()), (id(3), qty(4)));
        assert_eq!(ob.best_price(Side::Bid), None);
    }

    #[test]
    fn cancel_twice_returns_none() {
        let mut ob = book_with(&[(Side::Bid, 99, 1, 5)]);
        assert!(ob.cancel(id(1)).is_some());
        assert!(ob.cancel(id(1)).is_none());
        assert_invariants(&ob);
    }

    #[test]
    fn cancel_unknown_id_leaves_book_unchanged() {
        let mut ob = book_with(&[(Side::Ask, 101, 1, 5), (Side::Bid, 99, 2, 5)]);
        assert!(ob.cancel(id(42)).is_none());
        assert_invariants(&ob);
        assert_eq!(ob.index.len(), 2);
    }

    #[test]
    fn cancel_after_partial_fill_returns_remaining_qty() {
        let mut ob = book_with(&[(Side::Ask, 101, 1, 10)]);
        let _ = fill(&mut ob, Side::Ask, 4);
        let node = ob
            .cancel(id(1))
            .expect("partially filled order still rests");
        assert_eq!(node.qty(), qty(6));
    }

    #[test]
    fn cancel_after_full_fill_returns_none() {
        let mut ob = book_with(&[(Side::Ask, 101, 1, 5)]);
        assert_eq!(summary(&fill(&mut ob, Side::Ask, 5)), (101, 1, 5, None));
        assert!(ob.cancel(id(1)).is_none());
        assert_invariants(&ob);
    }

    #[test]
    fn cancel_of_filled_order_does_not_touch_the_order_reusing_its_slot() {
        let mut ob = book_with(&[(Side::Ask, 101, 1, 5)]);
        let _ = fill(&mut ob, Side::Ask, 5);
        ob.rest(Side::Ask, id(2), price(101), qty(3));
        assert_invariants(&ob);

        assert!(ob.cancel(id(1)).is_none());
        let node = ob.cancel(id(2)).expect("new order rests");
        assert_eq!((node.id(), node.qty()), (id(2), qty(3)));
        assert_invariants(&ob);
    }

    // --- Fill ---

    #[test]
    fn partial_fill_keeps_maker_in_index() {
        let mut ob = book_with(&[(Side::Bid, 99, 1, 10)]);
        assert_eq!(summary(&fill(&mut ob, Side::Bid, 4)), (99, 1, 4, Some(6)));
        assert!(ob.index.contains_key(&id(1)));
    }

    #[test]
    fn full_fill_drops_maker_from_index() {
        let mut ob = book_with(&[(Side::Bid, 99, 1, 3), (Side::Bid, 98, 2, 5)]);
        assert_eq!(summary(&fill(&mut ob, Side::Bid, 10)), (99, 1, 3, None));
        assert!(!ob.index.contains_key(&id(1)));
        assert_eq!(ob.best_price(Side::Bid), Some(price(98)));
    }

    #[test]
    fn fill_takes_from_the_requested_side_only() {
        let mut ob = book_with(&[(Side::Ask, 101, 1, 5), (Side::Bid, 99, 2, 5)]);
        assert_eq!(summary(&fill(&mut ob, Side::Ask, 5)), (101, 1, 5, None));
        assert!(ob.fill_best(Side::Ask, qty(1)).is_none());
        assert_eq!(ob.best_price(Side::Bid), Some(price(99)));
        assert!(ob.index.contains_key(&id(2)));
    }

    // --- Model-based churn ---

    /// Reference book: per side, price -> FIFO of `(id, remaining lots)`, plus where each
    /// live order rests.
    #[derive(Default)]
    struct Model {
        asks: BTreeMap<u64, VecDeque<(u64, u64)>>,
        bids: BTreeMap<u64, VecDeque<(u64, u64)>>,
        live: BTreeMap<u64, (Side, u64)>,
    }

    impl Model {
        fn levels(&mut self, side: Side) -> &mut BTreeMap<u64, VecDeque<(u64, u64)>> {
            match side {
                Side::Ask => &mut self.asks,
                Side::Bid => &mut self.bids,
            }
        }

        fn best(&self, side: Side) -> Option<u64> {
            match side {
                Side::Ask => self.asks.keys().next().copied(),
                Side::Bid => self.bids.keys().next_back().copied(),
            }
        }

        fn rest(&mut self, side: Side, ticks: u64, raw_id: u64, lots: u64) {
            self.levels(side)
                .entry(ticks)
                .or_default()
                .push_back((raw_id, lots));
            self.live.insert(raw_id, (side, ticks));
        }

        /// Returns the cancelled order's remaining lots.
        fn cancel(&mut self, raw_id: u64) -> Option<u64> {
            let (side, ticks) = self.live.remove(&raw_id)?;
            let levels = self.levels(side);
            let queue = levels.get_mut(&ticks).expect("live order has a level");
            let position = queue
                .iter()
                .position(|&(order, _)| order == raw_id)
                .expect("live order is queued");
            let (_, lots) = queue.remove(position).expect("position is in range");
            if queue.is_empty() {
                levels.remove(&ticks);
            }
            Some(lots)
        }

        /// Returns `(price, maker, filled, remaining)`, like `summary`.
        fn fill(&mut self, side: Side, wanted: u64) -> Option<(u64, u64, u64, Option<u64>)> {
            let mut best = match side {
                Side::Ask => self.asks.first_entry(),
                Side::Bid => self.bids.last_entry(),
            }?;
            let ticks = *best.key();
            let queue = best.get_mut();
            let front = queue.front_mut().expect("model never keeps an empty level");
            let maker = front.0;
            let filled = wanted.min(front.1);
            front.1 = front.1.checked_sub(filled).unwrap();
            let remaining = (front.1 != 0).then_some(front.1);
            if remaining.is_none() {
                queue.pop_front();
                if queue.is_empty() {
                    best.remove();
                }
                self.live.remove(&maker);
            }
            Some((ticks, maker, filled, remaining))
        }
    }

    #[test]
    fn random_churn_matches_a_model() {
        // Deterministic xorshift so failures reproduce
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let pick = |roll: u64, len: usize| -> usize {
            usize::try_from(roll % u64::try_from(len).unwrap()).unwrap()
        };

        let mut ob = OrderBook::new();
        let mut model = Model::default();
        // Ids that were cancelled or fully filled; cancelling them again must be a no-op.
        let mut dead: Vec<u64> = Vec::new();

        for step in 0..10_000_u64 {
            let side = if next() % 2 == 0 {
                Side::Ask
            } else {
                Side::Bid
            };
            match next() % 5 {
                0 | 1 => {
                    // Asks rest at 101..=110 and bids at 90..=100, so a rest never crosses
                    let ticks = match side {
                        Side::Ask => 101 + next() % 10,
                        Side::Bid => 90 + next() % 11,
                    };
                    let lots = next() % 20 + 1;
                    ob.rest(side, id(step), price(ticks), qty(lots));
                    model.rest(side, ticks, step, lots);
                }
                2 if !model.live.is_empty() => {
                    let index = pick(next(), model.live.len());
                    let raw_id = *model.live.keys().nth(index).expect("index is in range");
                    let node = ob.cancel(id(raw_id)).expect("live order cancels");
                    let lots = model.cancel(raw_id).expect("live order cancels");
                    assert_eq!(
                        (node.id(), node.qty()),
                        (id(raw_id), qty(lots)),
                        "wrong cancel at step {step}"
                    );
                    dead.push(raw_id);
                }
                3 if !dead.is_empty() => {
                    let raw_id = dead[pick(next(), dead.len())];
                    assert!(
                        ob.cancel(id(raw_id)).is_none(),
                        "dead order {raw_id} cancelled again at step {step}"
                    );
                }
                4 => {
                    let wanted = next() % 25 + 1;
                    let actual = ob.fill_best(side, qty(wanted)).as_ref().map(summary);
                    let expected = model.fill(side, wanted);
                    assert_eq!(actual, expected, "wrong fill at step {step}");
                    if let Some((_, maker, _, None)) = expected {
                        dead.push(maker);
                    }
                }
                _ => {}
            }

            assert_invariants(&ob);
            for side in [Side::Ask, Side::Bid] {
                assert_eq!(
                    ob.best_price(side).map(Price::get),
                    model.best(side),
                    "{side:?} best price diverged at step {step}"
                );
            }
        }
    }
}
