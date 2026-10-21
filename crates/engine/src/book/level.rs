use domain::{OrderId, Qty};

use crate::book::slab::{Slab, SlotKey};

#[derive(Debug)]
pub(crate) struct Node {
    id: OrderId,
    qty: Qty,
    prev: Option<SlotKey>,
    next: Option<SlotKey>,
}

#[derive(Debug)]
pub(crate) struct Level {
    head: Option<SlotKey>,
    tail: Option<SlotKey>,
    total_qty: u128,
    count: u32,
}

#[must_use = "a maker with `remaining: None` must be removed from the order index"]
#[derive(Debug)]
pub(crate) struct FrontFill {
    pub(crate) maker: OrderId,
    pub(crate) filled: Qty,
    pub(crate) remaining: Option<Qty>,
}

impl Node {
    pub(crate) fn id(&self) -> OrderId {
        self.id
    }

    pub(crate) fn qty(&self) -> Qty {
        self.qty
    }
}

impl Default for Level {
    fn default() -> Self {
        Self::new()
    }
}

fn node_mut(slab: &mut Slab<Node>, key: SlotKey) -> &mut Node {
    let Ok(node) = slab.get_mut(key) else {
        unreachable!("slab holds nodes for every existing key");
    };
    node
}

fn peek_node(slab: &Slab<Node>, key: SlotKey) -> &Node {
    let Ok(node) = slab.get(key) else {
        unreachable!("slab holds every existing key")
    };
    node
}

impl Level {
    #[must_use]
    pub(crate) fn new() -> Self {
        Level {
            head: None,
            tail: None,
            total_qty: 0,
            count: 0,
        }
    }
    fn peek_head<'a>(&self, slab: &'a Slab<Node>) -> Option<&'a Node> {
        self.head.map(|key| peek_node(slab, key))
    }
    fn increment_count(&mut self) {
        let Some(new_count) = self.count.checked_add(1) else {
            unreachable!("slab overflows before a level will");
        };
        self.count = new_count;
    }
    fn decrement_count(&mut self) {
        let Some(new_count) = self.count.checked_sub(1) else {
            unreachable!("count includes the node being removed");
        };
        self.count = new_count;
    }
    fn increase_qty(&mut self, qty: Qty) {
        let Some(new_qty) = self.total_qty.checked_add(u128::from(qty.get())) else {
            unreachable!("u128 cannot overflow from adding u64 u32 times");
        };
        self.total_qty = new_qty;
    }
    fn decrease_qty(&mut self, qty: Qty) {
        let Some(new_qty) = self.total_qty.checked_sub(u128::from(qty.get())) else {
            unreachable!("total_qty includes the qty being removed");
        };
        self.total_qty = new_qty;
    }
    pub(crate) fn len(&self) -> u32 {
        self.count
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(crate) fn total_qty(&self) -> u128 {
        self.total_qty
    }
    /// Removes the order at `key` from anywhere in the queue in O(1) and returns it.
    ///
    /// `key` must belong to this level. A key from another level corrupts both levels;
    /// debug builds catch it only when the node is a head or tail.
    ///
    /// # Panics
    ///
    /// Panics if `key` is stale or the level's links point at a freed node. Both mean
    /// the book's state is already corrupted.
    pub(crate) fn unlink(&mut self, slab: &mut Slab<Node>, key: SlotKey) -> Node {
        let Ok(node_to_be_unlinked) = slab.remove(key) else {
            unreachable!("slab holds nodes for every existing key");
        };

        match (node_to_be_unlinked.prev, node_to_be_unlinked.next) {
            // node is in the middle
            (Some(prev), Some(next)) => {
                node_mut(slab, prev).next = Some(next);
                node_mut(slab, next).prev = Some(prev);
            }
            // node is the head
            (None, Some(next)) => {
                debug_assert_eq!(self.head, Some(key), "wrong price level");
                self.head = Some(next);
                node_mut(slab, next).prev = None;
            }
            // node is the tail
            (Some(prev), None) => {
                debug_assert_eq!(self.tail, Some(key), "wrong price level");
                self.tail = Some(prev);
                node_mut(slab, prev).next = None;
            }
            // node is the only node - both head and tail
            (None, None) => {
                debug_assert_eq!(self.head, Some(key), "wrong price level");
                debug_assert_eq!(self.tail, Some(key), "wrong price level");
                self.head = None;
                self.tail = None;
            }
        }

        self.decrement_count();
        self.decrease_qty(node_to_be_unlinked.qty);

        node_to_be_unlinked
    }
    pub(crate) fn push_back(&mut self, slab: &mut Slab<Node>, id: OrderId, qty: Qty) -> SlotKey {
        let current_tail = self.tail;
        let key = slab.insert(Node {
            id,
            qty,
            prev: current_tail,
            next: None,
        });
        if let Some(current_tail_key) = current_tail {
            node_mut(slab, current_tail_key).next = Some(key);
        } else {
            // no tail -> no node -> this is the only node now
            self.head = Some(key);
        }
        self.tail = Some(key);
        self.increment_count();
        self.increase_qty(qty);
        key
    }

    fn partial_fill(&mut self, slab: &mut Slab<Node>, filled: Qty, new_qty: Qty) {
        let Some(head_key) = self.head else {
            unreachable!("fill is never called on empty level");
        };
        let head_node = node_mut(slab, head_key);
        head_node.qty = new_qty;
        self.decrease_qty(filled);
    }

    fn full_fill(&mut self, slab: &mut Slab<Node>) {
        let Some(head_key) = self.head else {
            unreachable!("fill is never called on empty level");
        };
        self.unlink(slab, head_key);
    }

    pub(crate) fn fill_front(&mut self, slab: &mut Slab<Node>, qty: Qty) -> FrontFill {
        let Some(head_node) = self.peek_head(slab) else {
            unreachable!("cannot call fill_front on empty level");
        };
        let maker = head_node.id;
        let filled = qty.min(head_node.qty); // Qty derives Ord
        let Ok(new_qty_option) = head_node.qty.checked_sub(filled) else {
            unreachable!("filled is clamped to the head node qty");
        };
        if let Some(new_qty) = new_qty_option {
            self.partial_fill(slab, filled, new_qty);
        } else {
            self.full_fill(slab);
        }
        FrontFill {
            maker,
            filled,
            remaining: new_qty_option,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qty(lots: u64) -> Qty {
        Qty::try_new(lots).expect("test qty must be non-zero")
    }

    fn id(raw: u64) -> OrderId {
        OrderId::new(raw)
    }

    /// Builds a level from `(id, qty)` pairs pushed in order. Returns the keys in push order.
    fn level_with(slab: &mut Slab<Node>, orders: &[(u64, u64)]) -> (Level, Vec<SlotKey>) {
        let mut level = Level::new();
        let keys = orders
            .iter()
            .map(|&(raw_id, lots)| level.push_back(slab, id(raw_id), qty(lots)))
            .collect();
        assert_invariants(slab, &level);
        (level, keys)
    }

    /// Order ids from head to tail.
    fn ids(slab: &Slab<Node>, level: &Level) -> Vec<u64> {
        let mut out = Vec::new();
        let mut cursor = level.head;
        while let Some(key) = cursor {
            let node = slab.get(key).expect("level links must point to live nodes");
            out.push(node.id.get());
            cursor = node.next;
        }
        out
    }

    fn assert_invariants(slab: &Slab<Node>, level: &Level) {
        let mut walked = 0_u32;
        let mut walked_qty = 0_u128;
        let mut key_option = level.head;
        let mut prev_key = None;

        while let Some(key) = key_option {
            assert!(walked <= level.count, "cycle in next links");
            let node = slab.get(key).expect("level links must point to live nodes");
            assert_eq!(node.prev, prev_key, "broken back link");
            walked = walked.checked_add(1).expect("walked count overflow");
            walked_qty = walked_qty
                .checked_add(u128::from(node.qty.get()))
                .expect("walked_qty overflow");
            prev_key = key_option;
            key_option = node.next;
        }
        assert_eq!(level.tail, prev_key, "broken tail back link");
        assert_eq!(walked, level.count, "walked does not match count");
        assert_eq!(
            walked_qty, level.total_qty,
            "walked_qty does not match total_qty"
        );
    }

    // --- Construction ---

    #[test]
    fn new_level_is_empty() {
        let level = Level::new();
        let slab = Slab::<Node>::new();
        assert!(level.is_empty());
        assert_eq!(level.len(), 0);
        assert_eq!(level.total_qty(), 0);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn default_level_is_empty() {
        let level = Level::default();
        assert!(level.is_empty());
        assert_invariants(&Slab::new(), &level);
    }

    // --- push_back ---

    #[test]
    fn push_back_onto_empty_level_makes_it_head_and_tail() {
        let mut level = Level::new();
        let mut slab = Slab::<Node>::new();
        let key = level.push_back(&mut slab, id(123), qty(456));
        assert_invariants(&slab, &level);
        assert_eq!(level.head, Some(key), "returned key must be the head");
        assert_eq!(level.tail, Some(key), "returned key must be the tail");
        assert_eq!(level.len(), 1, "count does not match");
        assert_eq!(level.total_qty(), 456, "total_qty does not match");
    }

    #[test]
    fn push_back_appends_after_current_tail() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5), (2, 7)]);
        let new = level.push_back(&mut slab, id(3), qty(9));

        assert_eq!(level.head, Some(keys[0]), "head must not move");
        assert_eq!(level.tail, Some(new), "new order must become the tail");
        assert_eq!(ids(&slab, &level), [1, 2, 3]);
        assert_eq!(level.len(), 3);
        assert_eq!(level.total_qty(), 21);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn total_qty_does_not_overflow_u64() {
        let mut slab = Slab::new();
        let (level, _) = level_with(&mut slab, &[(1, u64::MAX), (2, u64::MAX)]);
        assert_eq!(level.total_qty(), u128::from(u64::MAX) * 2);
    }

    // --- unlink ---

    #[test]
    fn unlink_head_promotes_next_to_head() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5), (2, 7), (3, 9)]);
        level.unlink(&mut slab, keys[0]);

        assert_eq!(level.head, Some(keys[1]));
        assert_eq!(ids(&slab, &level), [2, 3]);
        assert_eq!(level.len(), 2);
        assert_eq!(level.total_qty(), 16);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn unlink_tail_promotes_prev_to_tail() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5), (2, 7), (3, 9)]);
        level.unlink(&mut slab, keys[2]);

        assert_eq!(level.tail, Some(keys[1]));
        assert_eq!(ids(&slab, &level), [1, 2]);
        assert_eq!(level.total_qty(), 12);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn unlink_middle_joins_its_neighbours() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5), (2, 7), (3, 9)]);
        level.unlink(&mut slab, keys[1]);

        assert_eq!(level.head, Some(keys[0]), "head must not move");
        assert_eq!(level.tail, Some(keys[2]), "tail must not move");
        assert_eq!(ids(&slab, &level), [1, 3]);
        assert_eq!(level.total_qty(), 14);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn unlink_only_node_empties_level() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5)]);
        level.unlink(&mut slab, keys[0]);

        assert!(level.is_empty());
        assert_eq!(level.head, None);
        assert_eq!(level.tail, None);
        assert_eq!(level.total_qty(), 0);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn unlink_returns_the_removed_order_and_frees_its_slot() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5), (2, 7)]);
        let node = level.unlink(&mut slab, keys[1]);

        assert_eq!(node.id, id(2));
        assert_eq!(node.qty, qty(7));
        assert!(!slab.contains(keys[1]), "removed node must leave the slab");
        assert_eq!(slab.len(), 1);
    }

    #[test]
    fn push_then_unlink_all_in_any_order_restores_empty_level() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
        for &i in &[2, 0, 4, 1, 3] {
            level.unlink(&mut slab, keys[i]);
            assert_invariants(&slab, &level);
        }
        assert!(level.is_empty());
        assert_eq!(level.total_qty(), 0);
        assert!(slab.is_empty(), "every node must be freed");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "wrong price level")]
    fn unlink_with_head_key_of_another_level_is_caught_in_debug() {
        let mut slab = Slab::new();
        let (mut level_a, _) = level_with(&mut slab, &[(1, 5), (2, 5)]);
        let (_, keys_b) = level_with(&mut slab, &[(3, 5), (4, 5)]);
        level_a.unlink(&mut slab, keys_b[0]);
    }

    // --- fill_front ---

    #[test]
    fn fifo_ordering() {
        let mut slab = Slab::new();
        let (mut level, _) = level_with(&mut slab, &[(1, 5), (2, 5), (3, 5)]);
        assert_eq!(level.len(), 3, "count does not match");
        assert_eq!(level.total_qty(), 15, "total_qty does not match");

        for expected in [1, 2, 3] {
            let fill = level.fill_front(&mut slab, qty(5));
            assert_eq!(
                fill.maker,
                id(expected),
                "orders must fill in arrival order"
            );
            assert_eq!(fill.filled, qty(5));
            assert_eq!(fill.remaining, None);
            assert_invariants(&slab, &level);
        }
        assert!(level.is_empty());
    }

    #[test]
    fn partial_fill_keeps_head_in_place() {
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 10), (2, 7)]);
        let fill = level.fill_front(&mut slab, qty(3));

        assert_eq!(fill.maker, id(1));
        assert_eq!(fill.filled, qty(3));
        assert_eq!(fill.remaining, Some(qty(7)));
        assert_eq!(
            level.head,
            Some(keys[0]),
            "partially filled head stays first"
        );
        assert_eq!(slab.get(keys[0]).unwrap().qty, qty(7));
        assert_eq!(level.len(), 2, "partial fill must not change count");
        assert_eq!(level.total_qty(), 14);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn fill_one_lot_short_leaves_one_lot() {
        let mut slab = Slab::new();
        let (mut level, _) = level_with(&mut slab, &[(1, 10)]);
        let fill = level.fill_front(&mut slab, qty(9));

        assert_eq!(fill.remaining, Some(qty(1)));
        assert_eq!(level.len(), 1);
        assert_eq!(level.total_qty(), 1);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn repeated_partial_fills_then_exact_fill_removes_order() {
        let mut slab = Slab::new();
        let (mut level, _) = level_with(&mut slab, &[(1, 10), (2, 4)]);

        assert_eq!(level.fill_front(&mut slab, qty(3)).remaining, Some(qty(7)));
        assert_eq!(level.fill_front(&mut slab, qty(3)).remaining, Some(qty(4)));
        let last = level.fill_front(&mut slab, qty(4));
        assert_eq!(
            (last.maker, last.filled, last.remaining),
            (id(1), qty(4), None)
        );

        assert_eq!(ids(&slab, &level), [2]);
        assert_eq!(level.total_qty(), 4);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn over_asked_fill_is_clamped_to_head_qty() {
        let mut slab = Slab::new();
        let (mut level, _) = level_with(&mut slab, &[(1, 4), (2, 6)]);
        let fill = level.fill_front(&mut slab, qty(10));

        assert_eq!(fill.maker, id(1));
        assert_eq!(fill.filled, qty(4), "fill must stop at the head order");
        assert_eq!(fill.remaining, None);
        assert_eq!(ids(&slab, &level), [2], "next order must be untouched");
        assert_eq!(level.total_qty(), 6);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn full_fill_of_last_order_empties_level() {
        let mut slab = Slab::new();
        let (mut level, _) = level_with(&mut slab, &[(1, 5)]);
        let fill = level.fill_front(&mut slab, qty(5));

        assert_eq!(fill.remaining, None);
        assert!(level.is_empty());
        assert!(slab.is_empty());
        assert_invariants(&slab, &level);
    }

    #[test]
    #[should_panic(expected = "fill_front on empty level")]
    fn fill_front_on_empty_level_panics() {
        let mut slab = Slab::new();
        let mut level = Level::new();
        let _ = level.fill_front(&mut slab, qty(1));
    }

    // --- Regressions ---

    #[test]
    fn push_back_after_full_fill_of_only_order() {
        // Regression: full_fill once left `tail` pointing at the removed node.
        let mut slab = Slab::new();
        let (mut level, _) = level_with(&mut slab, &[(1, 5)]);
        let _ = level.fill_front(&mut slab, qty(5));

        let key = level.push_back(&mut slab, id(2), qty(3));
        assert_eq!(level.head, Some(key));
        assert_eq!(level.tail, Some(key));
        assert_invariants(&slab, &level);
    }

    #[test]
    fn unlink_new_head_after_full_fill() {
        // Regression: full_fill once left the new head's `prev` pointing at the removed node.
        let mut slab = Slab::new();
        let (mut level, keys) = level_with(&mut slab, &[(1, 5), (2, 5), (3, 5)]);
        let _ = level.fill_front(&mut slab, qty(5));

        level.unlink(&mut slab, keys[1]);
        assert_eq!(ids(&slab, &level), [3]);
        assert_invariants(&slab, &level);
    }

    // --- Shared slab ---

    #[test]
    fn levels_sharing_one_slab_stay_independent() {
        let mut slab = Slab::new();
        let (mut bids_100, keys_a) = level_with(&mut slab, &[(1, 5), (2, 6)]);
        let (asks_101, _) = level_with(&mut slab, &[(3, 7), (4, 8)]);

        bids_100.unlink(&mut slab, keys_a[0]);
        let _ = bids_100.fill_front(&mut slab, qty(6));

        assert!(bids_100.is_empty());
        assert_eq!(
            ids(&slab, &asks_101),
            [3, 4],
            "other level must be untouched"
        );
        assert_eq!(asks_101.total_qty(), 15);
        assert_invariants(&slab, &bids_100);
        assert_invariants(&slab, &asks_101);
    }

    // --- Model-based churn ---

    #[test]
    fn random_churn_matches_a_vecdeque_model() {
        use std::collections::VecDeque;

        // Deterministic xorshift so failures reproduce
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
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
        let mut level = Level::new();
        // Model: (key, id, remaining lots), head first.
        let mut model: VecDeque<(SlotKey, u64, u64)> = VecDeque::new();

        for step in 0..10_000_u64 {
            match next() % 4 {
                0 | 1 => {
                    let lots = next() % 20 + 1;
                    let key = level.push_back(&mut slab, id(step), qty(lots));
                    model.push_back((key, step, lots));
                }
                2 if !model.is_empty() => {
                    let (key, raw_id, lots) = model
                        .remove(pick(next(), model.len()))
                        .expect("index is in range");
                    let node = level.unlink(&mut slab, key);
                    assert_eq!((node.id, node.qty), (id(raw_id), qty(lots)));
                }
                3 if !model.is_empty() => {
                    let wanted = next() % 25 + 1;
                    let fill = level.fill_front(&mut slab, qty(wanted));
                    let front = model.front_mut().expect("model is non-empty");
                    let filled = wanted.min(front.2);
                    assert_eq!(fill.maker, id(front.1), "wrong maker at step {step}");
                    assert_eq!(fill.filled, qty(filled));
                    front.2 -= filled;
                    if front.2 == 0 {
                        assert_eq!(fill.remaining, None);
                        model.pop_front();
                    } else {
                        assert_eq!(fill.remaining, Some(qty(front.2)));
                    }
                }
                _ => {}
            }

            assert_eq!(level.len(), u32::try_from(model.len()).unwrap());
            assert_eq!(
                level.total_qty(),
                model.iter().map(|o| u128::from(o.2)).sum::<u128>()
            );
            if step % 250 == 0 {
                assert_invariants(&slab, &level);
                let expected: Vec<u64> = model.iter().map(|o| o.1).collect();
                assert_eq!(
                    ids(&slab, &level),
                    expected,
                    "queue order diverged at step {step}"
                );
            }
        }
        assert_invariants(&slab, &level);
    }
}
