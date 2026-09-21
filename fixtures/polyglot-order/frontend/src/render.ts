import { Order } from "./order";

export function describe(order: Order): string {
  return `${order.orderId} ${order.symbol} @ ${order.price.toFixed(2)}`;
}

export function byId(orders: Order[], id: string): Order | undefined {
  return orders.find((o) => o.orderId === id);
}
