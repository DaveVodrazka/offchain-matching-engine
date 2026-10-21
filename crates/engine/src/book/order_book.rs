use std::collections::HashMap;

use domain::{OrderId, Price, Qty, Side};

use crate::book::{
    book_side::{BestFill, BookSide},
    level::Node,
    slab::{Slab, SlotKey},
};

#[derive(Debug)]
struct IndexValue {
    side: Side,
    price: Price,
    slot_key: SlotKey,
}

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

    pub(crate) fn best_price(&self, side: Side) -> Option<Price> {
        match side {
            Side::Ask => self.asks.best_price(),
            Side::Bid => self.bids.best_price(),
        }
    }

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
