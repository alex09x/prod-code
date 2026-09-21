/** One filled order as the API returns it. */
export interface Order {
  orderId: string;
  symbol: string;
  price: number;
}

export function makeOrder(orderId: string, symbol: string, price: number): Order {
  return { orderId, symbol, price };
}
