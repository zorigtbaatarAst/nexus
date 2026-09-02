import { fetchOrders } from "../../lib/orders";
import { OrderSummary } from "../../components/OrderSummary";

export default async function OrdersPage() {
  const orders = await fetchOrders();
  return <OrderSummary orders={orders} />;
}
