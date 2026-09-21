package backend

import "fmt"

// Describe renders one order for a log line.
func Describe(o Order) string {
	return fmt.Sprintf("%s %s @ %.2f", o.OrderID, o.Symbol, o.Price)
}

// Lookup finds an order by its identifier.
func Lookup(all []Order, id string) (Order, bool) {
	for _, o := range all {
		if o.OrderID == id {
			return o, true
		}
	}
	return Order{}, false
}

const selectOrder = "SELECT order_id, symbol, price FROM orders WHERE order_id = $1"
