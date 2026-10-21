use std::collections::{
    BTreeMap,
    btree_map::{Entry, OccupiedEntry},
};

use domain::{OrderId, Price, Qty, Side};

use crate::book::{
    level::{FrontFill, Level, Node},
    slab::{Slab, SlotKey},
};

/// The result of filling the head order of a side's best level.
///
/// `price` is the maker's price, which is the trade price.
#[must_use = "a maker with `remaining: None` must be removed from the order index"]
#[derive(Debug)]
pub(crate) struct BestFill {
    pub(crate) price: Price,
    pub(crate) maker: OrderId,
    pub(crate) filled: Qty,
    pub(crate) remaining: Option<Qty>,
}

/// One side of the order book: price levels in price priority.
///
/// Invariant: the map never holds an empty level. Every mutating method removes a level
/// the moment it empties.
pub(crate) struct BookSide {
    side: Side,
    levels: BTreeMap<Price, Level>,
}

impl BookSide {
    pub(crate) fn new(side: Side) -> Self {
        Self {
            side,
            levels: BTreeMap::new(),
        }
    }

    /// The lowest ask or the highest bid, or `None` if the side is empty.
    pub(crate) fn best_price(&self) -> Option<Price> {
        match self.side {
            Side::Ask => self.levels.first_key_value().map(|(&p, _)| p),
            Side::Bid => self.levels.last_key_value().map(|(&p, _)| p),
        }
    }

    fn best_entry(&mut self) -> Option<OccupiedEntry<'_, Price, Level>> {
        match self.side {
            Side::Ask => self.levels.first_entry(),
            Side::Bid => self.levels.last_entry(),
        }
    }

    /// Appends an order to the back of the level at `price`, creating the level if needed.
    ///
    /// The caller must make sure `price` doesn't cross the opposite side and must record
    /// the returned `SlotKey` in the index.
    pub(crate) fn rest(
        &mut self,
        slab: &mut Slab<Node>,
        price: Price,
        id: OrderId,
        qty: Qty,
    ) -> SlotKey {
        self.levels
            .entry(price)
            .or_default()
            .push_back(slab, id, qty)
    }

    /// Removes the order at `key` from the level at `price` and returns it. Removes the level
    /// if it empties.
    ///
    /// `key` must belong to the level at `price`. A key from another price corrupts both
    /// levels; debug builds catch it only when the node is a head or tail.
    ///
    /// # Panics
    ///
    /// Panics if no level exists at `price` or `key` is stale. Both mean the index and the
    /// book disagree.
    pub(crate) fn cancel(&mut self, slab: &mut Slab<Node>, price: Price, key: SlotKey) -> Node {
        match self.levels.entry(price) {
            Entry::Occupied(mut e) => {
                let level = e.get_mut();
                let node = level.unlink(slab, key);
                if level.is_empty() {
                    e.remove();
                }
                node
            }
            Entry::Vacant(_) => unreachable!("index points at a price with no level"),
        }
    }

    /// Fills the head order of the best level, up to `wanted`, and removes the level if it
    /// empties. Returns `None` if the side is empty.
    #[must_use]
    pub(crate) fn fill_best(&mut self, slab: &mut Slab<Node>, wanted: Qty) -> Option<BestFill> {
        let mut best_entry = self.best_entry()?;
        let price = *best_entry.key();
        let level = best_entry.get_mut();
        let FrontFill {
            maker,
            filled,
            remaining,
        } = level.fill_front(slab, wanted);
        if level.is_empty() {
            best_entry.remove();
        }
        Some(BestFill {
            price,
            maker,
            filled,
            remaining,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};

    use domain::{OrderId, Price, Qty, Side};

    use super::{BestFill, BookSide};
    use crate::book::{level::Node, slab::Slab, slab::SlotKey};

    fn price(ticks: u64) -> Price {
        Price::try_new(ticks).expect("test price must be non-zero")
    }

    fn qty(lots: u64) -> Qty {
        Qty::try_new(lots).expect("test qty must be non-zero")
    }

    fn id(raw: u64) -> OrderId {
        OrderId::new(raw)
    }

    /// Builds a side from `(price, id, lots)` orders rested in order. Returns the keys in
    /// rest order.
    fn side_with(
        direction: Side,
        orders: &[(u64, u64, u64)],
    ) -> (Slab<Node>, BookSide, Vec<SlotKey>) {
        let mut slab = Slab::new();
        let mut side = BookSide::new(direction);
        let keys = orders
            .iter()
            .map(|&(ticks, raw_id, lots)| side.rest(&mut slab, price(ticks), id(raw_id), qty(lots)))
            .collect();
        assert_invariants(&slab, &mut side);
        (slab, side, keys)
    }

    /// Fills against a side that must have liquidity.
    fn fill(slab: &mut Slab<Node>, side: &mut BookSide, wanted: u64) -> BestFill {
        let fill = side
            .fill_best(slab, qty(wanted))
            .expect("side has liquidity");
        assert_invariants(slab, side);
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

    /// `(price, order count, total lots)` per level, lowest price first.
    fn levels(side: &BookSide) -> Vec<(u64, u32, u128)> {
        side.levels
            .iter()
            .map(|(p, level)| (p.get(), level.len(), level.total_qty()))
            .collect()
    }

    /// Checks every invariant `BookSide` owns. Assumes `side` is the only user of `slab`.
    ///
    /// Takes `&mut` only because `best_entry` needs it; nothing is mutated.
    fn assert_invariants(slab: &Slab<Node>, side: &mut BookSide) {
        let mut resting_orders = 0_u32;
        for (price, level) in &side.levels {
            assert!(
                !level.is_empty(),
                "empty level left in the map at {price:?}"
            );
            resting_orders = resting_orders
                .checked_add(level.len())
                .expect("resting order count overflow");
        }
        assert_eq!(
            slab.len(),
            resting_orders,
            "slab holds nodes that no level links to, or levels count freed nodes"
        );

        let expected_best = match side.side {
            Side::Ask => side.levels.keys().next().copied(),
            Side::Bid => side.levels.keys().next_back().copied(),
        };
        assert_eq!(
            side.best_price(),
            expected_best,
            "best_price picks the wrong end"
        );
        assert_eq!(
            side.best_entry().map(|entry| *entry.key()),
            expected_best,
            "best_entry picks the wrong end"
        );
    }

    // --- Construction ---

    #[test]
    fn new_side_is_empty() {
        for direction in [Side::Ask, Side::Bid] {
            let mut slab = Slab::new();
            let mut side = BookSide::new(direction);
            assert_eq!(side.best_price(), None);
            assert!(side.fill_best(&mut slab, qty(1)).is_none());
            assert_invariants(&slab, &mut side);
        }
    }

    // --- Price priority ---

    #[test]
    fn ask_best_is_lowest_price() {
        let (_, side, _) = side_with(
            Side::Ask,
            &[(100, 1, 5), (102, 2, 5), (98, 3, 5), (101, 4, 5)],
        );
        assert_eq!(side.best_price(), Some(price(98)));
    }

    #[test]
    fn bid_best_is_highest_price() {
        let (_, side, _) = side_with(
            Side::Bid,
            &[(100, 1, 5), (98, 2, 5), (102, 3, 5), (101, 4, 5)],
        );
        assert_eq!(side.best_price(), Some(price(102)));
    }

    #[test]
    fn ask_fills_walk_up_from_lowest_price() {
        let (mut slab, mut side, _) =
            side_with(Side::Ask, &[(101, 1, 2), (100, 2, 2), (102, 3, 2)]);
        let prices: Vec<u64> = (0..3)
            .map(|_| fill(&mut slab, &mut side, 2).price.get())
            .collect();
        assert_eq!(prices, [100, 101, 102]);
        assert_eq!(side.best_price(), None);
    }

    #[test]
    fn bid_fills_walk_down_from_highest_price() {
        let (mut slab, mut side, _) =
            side_with(Side::Bid, &[(101, 1, 2), (100, 2, 2), (102, 3, 2)]);
        let prices: Vec<u64> = (0..3)
            .map(|_| fill(&mut slab, &mut side, 2).price.get())
            .collect();
        assert_eq!(prices, [102, 101, 100]);
        assert_eq!(side.best_price(), None);
    }

    // --- Rest ---

    #[test]
    fn rest_at_existing_price_joins_that_level() {
        let (_, side, _) = side_with(Side::Ask, &[(100, 1, 5), (100, 2, 7)]);
        assert_eq!(levels(&side), [(100, 2, 12)]);
    }

    #[test]
    fn rest_returns_distinct_keys() {
        let (_, _, keys) = side_with(Side::Ask, &[(100, 1, 5), (100, 2, 5), (101, 3, 5)]);
        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[1], keys[2]);
        assert_ne!(keys[0], keys[2]);
    }

    #[test]
    fn rest_after_level_emptied_recreates_it() {
        let (mut slab, mut side, keys) = side_with(Side::Bid, &[(100, 1, 5)]);
        let _ = side.cancel(&mut slab, price(100), keys[0]);
        side.rest(&mut slab, price(100), id(2), qty(3));
        assert_invariants(&slab, &mut side);
        assert_eq!(levels(&side), [(100, 1, 3)]);
        assert_eq!(summary(&fill(&mut slab, &mut side, 3)), (100, 2, 3, None));
    }

    // --- Fill ---

    #[test]
    fn partial_fill_keeps_level_and_head() {
        let (mut slab, mut side, _) = side_with(Side::Ask, &[(100, 1, 10)]);
        assert_eq!(
            summary(&fill(&mut slab, &mut side, 4)),
            (100, 1, 4, Some(6))
        );
        assert_eq!(levels(&side), [(100, 1, 6)]);
        assert_eq!(summary(&fill(&mut slab, &mut side, 6)), (100, 1, 6, None));
    }

    #[test]
    fn full_fill_of_last_order_removes_level_and_moves_best() {
        let (mut slab, mut side, _) = side_with(Side::Ask, &[(100, 1, 3), (101, 2, 20)]);
        assert_eq!(summary(&fill(&mut slab, &mut side, 10)), (100, 1, 3, None));
        assert_eq!(side.best_price(), Some(price(101)));
        assert_eq!(
            summary(&fill(&mut slab, &mut side, 7)),
            (101, 2, 7, Some(13))
        );
        assert_eq!(levels(&side), [(101, 1, 13)]);
    }

    #[test]
    fn fill_takes_one_maker_per_call() {
        let (mut slab, mut side, _) = side_with(Side::Ask, &[(100, 1, 2), (100, 2, 5)]);
        assert_eq!(
            summary(&fill(&mut slab, &mut side, 10)),
            (100, 1, 2, None),
            "over-asked fill must be clamped to the head order"
        );
        assert_eq!(levels(&side), [(100, 1, 5)]);
    }

    #[test]
    fn fills_within_a_level_are_fifo() {
        let (mut slab, mut side, _) =
            side_with(Side::Bid, &[(100, 1, 2), (100, 2, 1), (100, 3, 4)]);
        let makers: Vec<u64> = (0..3)
            .map(|_| fill(&mut slab, &mut side, 4).maker.get())
            .collect();
        assert_eq!(makers, [1, 2, 3]);
    }

    #[test]
    fn sweeping_the_side_empties_it_and_frees_every_slot() {
        let (mut slab, mut side, _) =
            side_with(Side::Ask, &[(100, 1, 2), (101, 2, 3), (101, 3, 4)]);
        for _ in 0..3 {
            let _ = fill(&mut slab, &mut side, 4);
        }
        assert!(side.fill_best(&mut slab, qty(1)).is_none());
        assert!(slab.is_empty());
    }

    // --- Cancel ---

    #[test]
    fn cancel_returns_the_order() {
        let (mut slab, mut side, keys) = side_with(Side::Ask, &[(100, 1, 5), (100, 2, 7)]);
        let node = side.cancel(&mut slab, price(100), keys[1]);
        assert_eq!((node.id(), node.qty()), (id(2), qty(7)));
        assert_invariants(&slab, &mut side);
    }

    #[test]
    fn cancel_after_partial_fill_returns_remaining_qty() {
        let (mut slab, mut side, keys) = side_with(Side::Ask, &[(100, 1, 10)]);
        let _ = fill(&mut slab, &mut side, 4);
        let node = side.cancel(&mut slab, price(100), keys[0]);
        assert_eq!(node.qty(), qty(6));
    }

    #[test]
    fn cancel_only_order_at_best_removes_level_and_moves_best() {
        let (mut slab, mut side, keys) = side_with(Side::Bid, &[(100, 1, 5), (99, 2, 5)]);
        let _ = side.cancel(&mut slab, price(100), keys[0]);
        assert_invariants(&slab, &mut side);
        assert_eq!(side.best_price(), Some(price(99)));
        assert_eq!(levels(&side), [(99, 1, 5)]);
    }

    #[test]
    fn cancel_middle_order_keeps_level_and_queue_order() {
        let (mut slab, mut side, keys) =
            side_with(Side::Ask, &[(100, 1, 1), (100, 2, 1), (100, 3, 1)]);
        let _ = side.cancel(&mut slab, price(100), keys[1]);
        assert_invariants(&slab, &mut side);
        assert_eq!(levels(&side), [(100, 2, 2)]);
        let makers: Vec<u64> = (0..2)
            .map(|_| fill(&mut slab, &mut side, 1).maker.get())
            .collect();
        assert_eq!(makers, [1, 3]);
    }

    #[test]
    fn cancel_behind_best_leaves_best_unchanged() {
        let (mut slab, mut side, keys) = side_with(Side::Ask, &[(100, 1, 5), (105, 2, 5)]);
        let _ = side.cancel(&mut slab, price(105), keys[1]);
        assert_invariants(&slab, &mut side);
        assert_eq!(side.best_price(), Some(price(100)));
        assert_eq!(levels(&side), [(100, 1, 5)]);
    }

    #[test]
    #[should_panic(expected = "index points at a price with no level")]
    fn cancel_at_price_with_no_level_panics() {
        let (mut slab, mut side, keys) = side_with(Side::Ask, &[(100, 1, 5)]);
        let _ = side.cancel(&mut slab, price(101), keys[0]);
    }

    #[test]
    #[should_panic(expected = "slab holds nodes for every existing key")]
    fn cancel_with_stale_key_panics() {
        let (mut slab, mut side, keys) = side_with(Side::Ask, &[(100, 1, 5), (100, 2, 5)]);
        let _ = side.cancel(&mut slab, price(100), keys[0]);
        let _ = side.cancel(&mut slab, price(100), keys[0]);
    }

    // --- Model-based churn ---

    /// One resting order in the model: `(key, id, remaining lots)`.
    type ModelOrder = (SlotKey, u64, u64);

    #[test]
    fn random_churn_matches_a_btreemap_model() {
        for direction in [Side::Ask, Side::Bid] {
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

            let mut slab = Slab::new();
            let mut side = BookSide::new(direction);
            // Model: price -> queue of resting orders, head first.
            let mut model: BTreeMap<u64, VecDeque<ModelOrder>> = BTreeMap::new();

            for step in 0..10_000_u64 {
                match next() % 4 {
                    0 | 1 => {
                        let ticks = 95 + next() % 11;
                        let lots = next() % 20 + 1;
                        let key = side.rest(&mut slab, price(ticks), id(step), qty(lots));
                        model.entry(ticks).or_default().push_back((key, step, lots));
                    }
                    2 if !model.is_empty() => {
                        let total: usize = model.values().map(VecDeque::len).sum();
                        let mut index = pick(next(), total);
                        let ticks = *model
                            .iter()
                            .find(|(_, queue)| {
                                if index < queue.len() {
                                    true
                                } else {
                                    index -= queue.len();
                                    false
                                }
                            })
                            .expect("index is in range")
                            .0;
                        let queue = model.get_mut(&ticks).expect("level exists");
                        let (key, raw_id, lots) = queue.remove(index).expect("index is in range");
                        if queue.is_empty() {
                            model.remove(&ticks);
                        }
                        let node = side.cancel(&mut slab, price(ticks), key);
                        assert_eq!((node.id(), node.qty()), (id(raw_id), qty(lots)));
                    }
                    3 if !model.is_empty() => {
                        let wanted = next() % 25 + 1;
                        let fill = side
                            .fill_best(&mut slab, qty(wanted))
                            .expect("side has liquidity");
                        let mut best = match direction {
                            Side::Ask => model.first_entry(),
                            Side::Bid => model.last_entry(),
                        }
                        .expect("model is non-empty");
                        let ticks = *best.key();
                        let front = best.get_mut().front_mut().expect("level is non-empty");
                        let filled = wanted.min(front.2);
                        assert_eq!(
                            (fill.price.get(), fill.maker.get(), fill.filled.get()),
                            (ticks, front.1, filled),
                            "wrong fill at step {step}"
                        );
                        front.2 -= filled;
                        if front.2 == 0 {
                            assert_eq!(fill.remaining, None);
                            best.get_mut().pop_front();
                            if best.get().is_empty() {
                                best.remove();
                            }
                        } else {
                            assert_eq!(fill.remaining, Some(qty(front.2)));
                        }
                    }
                    _ => {}
                }

                assert_invariants(&slab, &mut side);
                let expected: Vec<(u64, u32, u128)> = model
                    .iter()
                    .map(|(&ticks, queue)| {
                        (
                            ticks,
                            u32::try_from(queue.len()).unwrap(),
                            queue.iter().map(|order| u128::from(order.2)).sum(),
                        )
                    })
                    .collect();
                assert_eq!(levels(&side), expected, "levels diverged at step {step}");
            }
        }
    }
}
