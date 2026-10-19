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

#[derive(Debug)]
pub(crate) struct FrontFill {
    pub(crate) maker: OrderId,
    pub(crate) filled: Qty,
    pub(crate) remaining: Option<Qty>,
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

    #[must_use]
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

    #[test]
    fn new_level_is_empty() {
        let level = Level::new();
        let slab = Slab::<Node>::new();
        assert!(level.is_empty());
        assert_eq!(level.len(), 0);
        assert_invariants(&slab, &level);
    }

    #[test]
    fn insert() {
        let mut level = Level::new();
        let mut slab = Slab::<Node>::new();
        let id = OrderId::new(123);
        let qty = Qty::try_new(456).expect("failed creating Qty");
        level.push_back(&mut slab, id, qty);
        assert_invariants(&slab, &level);
        assert_eq!(level.len(), 1, "count does not match");
        assert_eq!(
            level.total_qty(),
            u128::from(qty.get()),
            "total_qty does not match"
        );
    }

    #[test]
    fn fifo_ordering() {
        let mut level = Level::new();
        let mut slab = Slab::<Node>::new();
        let id1 = OrderId::new(1);
        let id2 = OrderId::new(2);
        let id3 = OrderId::new(3);
        level.push_back(&mut slab, id1, qty(5));
        assert_invariants(&slab, &level);
        level.push_back(&mut slab, id2, qty(5));
        assert_invariants(&slab, &level);
        level.push_back(&mut slab, id3, qty(5));
        assert_invariants(&slab, &level);
        assert_eq!(level.len(), 3, "count does not match");
        assert_eq!(
            level.total_qty(),
            qty(15).get().into(),
            "total_qty does not match"
        );
        let res1 = level.fill_front(&mut slab, qty(5));
        assert_eq!(res1.maker, id1);
        assert_eq!(res1.filled, qty(5));
        assert_eq!(res1.remaining, None);
        assert_invariants(&slab, &level);
        let res2 = level.fill_front(&mut slab, qty(5));
        assert_eq!(res2.maker, id2);
        assert_eq!(res2.filled, qty(5));
        assert_eq!(res2.remaining, None);
        assert_invariants(&slab, &level);
        let res3 = level.fill_front(&mut slab, qty(5));
        assert_eq!(res3.maker, id3);
        assert_eq!(res3.filled, qty(5));
        assert_eq!(res3.remaining, None);
        assert_invariants(&slab, &level);
    }
}
