#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct OrderId(u64);

impl OrderId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn new(id: u64) -> Self {
        OrderId(id)
    }
}

#[derive(Debug)]
pub struct OrderIdGenerator {
    last_issued: OrderId,
}

impl OrderIdGenerator {
    #[must_use]
    pub fn fresh() -> Self {
        OrderIdGenerator {
            last_issued: OrderId(0),
        }
    }

    #[must_use]
    pub fn resume_after(last_order_id: OrderId) -> Self {
        OrderIdGenerator {
            last_issued: last_order_id,
        }
    }

    /// # Panics
    ///
    /// Will panic when out of order ids (u64 add overflow).
    pub fn next_id(&mut self) -> OrderId {
        let current = self.last_issued.get();
        #[allow(clippy::panic)]
        let Some(next) = current.checked_add(1) else {
            panic!("OrderIdGenerator ran out of ids");
        };
        let next_order_id = OrderId(next);
        self.last_issued = next_order_id;
        next_order_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_generator_issues_one_first() {
        assert_eq!(OrderIdGenerator::fresh().next_id(), OrderId::new(1));
    }

    #[test]
    fn ids_are_sequential() {
        let mut generator = OrderIdGenerator::fresh();
        let ids: Vec<u64> = (0..3).map(|_| generator.next_id().get()).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn resume_continues_after_checkpoint() {
        let mut generator = OrderIdGenerator::resume_after(OrderId::new(41));
        assert_eq!(generator.next_id(), OrderId::new(42));
    }

    #[test]
    fn resume_does_not_reissue_checkpoint() {
        let checkpoint = OrderId::new(41);
        let mut generator = OrderIdGenerator::resume_after(checkpoint);
        assert!(generator.next_id() > checkpoint);
    }

    #[test]
    fn issues_max_as_last_id() {
        let mut generator = OrderIdGenerator::resume_after(OrderId::new(u64::MAX - 1));
        assert_eq!(generator.next_id(), OrderId::new(u64::MAX));
    }

    #[test]
    #[should_panic(expected = "ran out of ids")]
    fn panics_after_max_is_issued() {
        let mut generator = OrderIdGenerator::resume_after(OrderId::new(u64::MAX));
        let _ = generator.next_id();
    }
}
