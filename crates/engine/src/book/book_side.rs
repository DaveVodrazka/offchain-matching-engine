use std::collections::{
    BTreeMap,
    btree_map::{Entry, OccupiedEntry},
};

use domain::{OrderId, Price, Qty, Side};

use crate::book::{
    level::{FrontFill, Level, Node},
    slab::{Slab, SlotKey},
};

#[must_use]
#[derive(Debug)]
pub(crate) struct BestFill {
    pub(crate) maker: OrderId,
    pub(crate) filled: Qty,
    pub(crate) remaining: Option<Qty>,
    pub(crate) price: Price,
}

/// One side of the order book: price levels in price priority.
/// Invariant: the map never holds an empty level.
/// every mutating method removes a level the moment it empties.
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

    /// The caller must make sure price doesn't cross the opposite side
    /// and must record the returned `SlotKey` in the index.
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

    /// # Panics
    ///
    /// Panics if no level exists at price or key is stale. Both mean the index and the book disagree.
    /// Key must belong to the level at price.
    /// A key from a different price corrupts both levels.
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

    /// Fills the head order of the best level, up to wanted.
    /// Removes the level if it empties.
    /// Returns None if the side is empty.
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
            maker,
            filled,
            remaining,
            price,
        })
    }
}

#[cfg(test)]
mod tests {
    use domain::Side;

    use crate::book::book_side::BookSide;

    #[test]
    fn test_new() {
        let side = BookSide::new(Side::Ask);
        assert!(side.best_price().is_none());
    }
}
