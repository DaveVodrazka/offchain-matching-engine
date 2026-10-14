#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ask,
    Bid,
}

impl Side {
    #[must_use]
    pub fn opposite(self) -> Side {
        match self {
            Side::Ask => Side::Bid,
            Side::Bid => Side::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_of_ask_is_bid() {
        assert_eq!(Side::Ask.opposite(), Side::Bid);
    }

    #[test]
    fn opposite_of_bid_is_ask() {
        assert_eq!(Side::Bid.opposite(), Side::Ask);
    }

    #[test]
    fn opposite_twice_is_identity() {
        for side in [Side::Ask, Side::Bid] {
            assert_eq!(side.opposite().opposite(), side);
        }
    }
}
