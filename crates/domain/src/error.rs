use thiserror::Error;

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("price set to 0")]
    ZeroPrice,
    #[error("quantity set to 0")]
    ZeroQuantity,
    #[error("quantity underflow")]
    QuantityUnderflow,
    #[error("tick size cannot be 0")]
    ZeroTickSize,
    #[error("lot size cannot be 0")]
    ZeroLotSize,
    #[error("tick * lot must be integer")]
    NonIntegralTickLot,
    #[error("too large base decimal")]
    DecimalsTooLarge,
    #[error("tick * lot is too large")]
    TickLotProductTooLarge,
    #[error("calculating notional overflowed")]
    NotionalOverflow,
}
