import type { Order } from "../lib/orders";

export function OrderSummary({ orders }: { orders: Order[] }) {
  const total = orders.reduce((sum, o) => sum + o.totalAmount, 0);
  return (
    <section aria-label="Order summary">
      <h2>Orders</h2>
      <ul>
        {orders.map((o) => (
          <li key={o.id}>
            <span>{o.reference}</span>
            <span>{o.totalAmount.toFixed(2)}</span>
            <span>{o.status}</span>
          </li>
        ))}
      </ul>
      <p>Total: {total.toFixed(2)}</p>
    </section>
  );
}
