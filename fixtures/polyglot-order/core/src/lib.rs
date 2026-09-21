//! The shared order model.

/// One filled order.
#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub order_id: String,
    pub symbol: String,
    pub price: f64,
}

impl Order {
    pub fn new(order_id: String, symbol: String, price: f64) -> Self {
        Self {
            order_id,
            symbol,
            price,
        }
    }

    /// The key this order is stored under.
    pub fn key(&self) -> String {
        format!("{}:{}", self.symbol, self.order_id)
    }
}

pub const SELECT_ORDER: &str = "SELECT order_id, symbol, price FROM orders";
