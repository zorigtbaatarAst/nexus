import { expect, it, vi } from "vitest";
import { fetchOrders, ORDERS_QUERY } from "./orders";

/**
 * The frontend's collateral test: it exists so `npm test` runs something, and so a change
 * that breaks the transport is caught rather than merely type-checked.
 *
 * It asserts on the request and on what comes back, never on a field name — Family B renames
 * `totalAmount`, and a test that named it would fail on a *correct* rename, which the grader
 * would score as collateral damage.
 */
it("posts the orders query to /graphql and returns the rows the server sent", async () => {
  const rows = [{ id: "o-1", reference: "REF-1", status: "PAID" }];
  const fetchMock = vi.fn(async () => ({ json: async () => ({ data: { orders: rows } }) }));
  vi.stubGlobal("fetch", fetchMock);

  const got = await fetchOrders();

  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
  expect(url).toBe("/graphql");
  expect(init.method).toBe("POST");
  expect(JSON.parse(String(init.body)).query).toBe(ORDERS_QUERY);
  expect(got).toEqual(rows);

  vi.unstubAllGlobals();
});
