package backend

// Order is the wire form of a filled order.
type Order struct {
	OrderID string  `json:"order_id"`
	Symbol  string  `json:"symbol"`
	Price   float64 `json:"price"`
}

// NewOrder builds an order.
func NewOrder(id string, symbol string, price float64) Order {
	return Order{OrderID: id, Symbol: symbol, Price: price}
}
