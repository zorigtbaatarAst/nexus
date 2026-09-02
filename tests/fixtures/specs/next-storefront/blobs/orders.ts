export const ORDERS_QUERY = /* GraphQL */ `
  query Orders {
    orders {
      id
      reference
      totalAmount
      status
    }
  }
`;

export async function fetchOrders(): Promise<Order[]> {
  const res = await fetch("/graphql", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query: ORDERS_QUERY }),
  });
  const { data } = await res.json();
  return data.orders;
}

export interface Order {
  id: string;
  reference: string;
  totalAmount: number;
  status: string;
}
