use domain::{FillId, FillIdGenerator, OrderId, Price, Qty, Side};

use crate::{
    book::{
        book_side::BestFill,
        order_book::{CancelledOrder, OrderBook},
    },
    command::Command,
    event::{Event, RejectionReason},
};

pub struct Engine {
    order_book: OrderBook,
    fill_id_generator: FillIdGenerator,
}

fn price_crosses(side: Side, price_wanted: Price, best_price: Price) -> bool {
    match side {
        Side::Ask => price_wanted <= best_price,
        Side::Bid => price_wanted >= best_price,
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            order_book: OrderBook::new(),
            fill_id_generator: FillIdGenerator::fresh(),
        }
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn cancel(&mut self, id: OrderId) -> Vec<Event> {
        let Some(CancelledOrder { price, qty, side }) = self.order_book.cancel(id) else {
            return vec![Event::Rejected {
                id,
                reason: RejectionReason::UnknownOrder,
            }];
        };
        vec![Event::Cancelled {
            id,
            qty,
            side,
            price,
        }]
    }

    fn rest(&mut self, id: OrderId, side: Side, price: Price, qty: Qty) -> Event {
        self.order_book.rest(side, id, price, qty);
        Event::Rested {
            id,
            side,
            price,
            qty,
        }
    }

    /// # Panics
    ///
    /// Panics after `u64::MAX` fills
    fn increment_fill_id(&mut self) -> FillId {
        self.fill_id_generator.next_id()
    }

    fn limit(&mut self, id: OrderId, qty: Qty, side: Side, price: Price) -> Vec<Event> {
        let mut events = vec![];
        let mut qty_left_option = Some(qty);
        while let Some(qty_left) = qty_left_option {
            let Some(best_price) = self.order_book.best_price(side.opposite()) else {
                // other side is empty, we are done
                break;
            };
            if !price_crosses(side, price, best_price) {
                // price is not good enough, we are done
                break;
            }
            let Some(BestFill {
                price: price_fill,
                maker,
                filled,
                remaining: _,
            }) = self.order_book.fill_best(side.opposite(), qty_left)
            else {
                unreachable!("we checked that good price exists");
            };
            let Ok(new_qty_left) = qty_left.checked_sub(filled) else {
                unreachable!("fill_best fills at most qty_left");
            };
            qty_left_option = new_qty_left;
            let fill_id = self.increment_fill_id();
            events.push(Event::Fill {
                maker,
                taker: id,
                taker_side: side,
                filled,
                price: price_fill,
                fill_id,
            });
        }
        if let Some(qty_left) = qty_left_option {
            events.push(self.rest(id, side, price, qty_left));
        }
        events
    }

    #[must_use]
    pub fn apply(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::Cancel { id } => self.cancel(id),
            Command::Limit {
                id,
                qty,
                side,
                price,
            } => self.limit(id, qty, side, price),
            Command::Market => todo!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order_id(n: u64) -> OrderId {
        OrderId::new(n)
    }

    fn qty(n: u64) -> Qty {
        Qty::try_new(n).unwrap()
    }

    fn price(n: u64) -> Price {
        Price::try_new(n).unwrap()
    }

    #[test]
    fn test_create_cancel() {
        let mut engine = Engine::new();
        let limit_events = engine.apply(Command::Limit {
            id: order_id(1),
            qty: qty(10),
            side: Side::Ask,
            price: price(100),
        });
        assert_eq!(
            limit_events,
            vec![Event::Rested {
                id: order_id(1),
                qty: qty(10),
                side: Side::Ask,
                price: price(100),
            }]
        );
        let cancel_events = engine.apply(Command::Cancel { id: order_id(1) });
        assert_eq!(
            cancel_events,
            vec![Event::Cancelled {
                id: order_id(1),
                qty: qty(10),
                side: Side::Ask,
                price: price(100),
            }]
        );
    }

    #[test]
    fn test_limit_match() {
        let mut engine = Engine::new();
        let ask_events = engine.apply(Command::Limit {
            id: order_id(1),
            qty: qty(10),
            side: Side::Ask,
            price: price(100),
        });
        assert_eq!(
            ask_events,
            vec![Event::Rested {
                id: order_id(1),
                qty: qty(10),
                side: Side::Ask,
                price: price(100),
            }]
        );
        let bid_events = engine.apply(Command::Limit {
            id: order_id(2),
            qty: qty(15),
            side: Side::Bid,
            price: price(102),
        });
        assert_eq!(
            bid_events,
            vec![
                Event::Fill {
                    maker: order_id(1),
                    taker: order_id(2),
                    taker_side: Side::Bid,
                    filled: qty(10),
                    price: price(100),
                    fill_id: FillId::new(1)
                },
                Event::Rested {
                    id: order_id(2),
                    qty: qty(5),
                    side: Side::Bid,
                    price: price(102),
                }
            ]
        );
    }
}
