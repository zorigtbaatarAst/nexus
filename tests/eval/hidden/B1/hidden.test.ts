import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * L1 for B1-rename-crosses-the-seam, frontend half.
 *
 * The task: "Rename the `totalAmount` field on Order to `grossAmount` throughout."
 *
 * `tsc --noEmit` keeps the component and the client-side `Order` type agreeing with each
 * other, and that is all it keeps agreeing: the GraphQL query is a template literal, so a
 * rename that never reaches the frontend compiles exactly like one that did. Nothing here is
 * covered by L0, and nothing here can be seen from the Java side either — see `HiddenTest.java`
 * beside this file for the backend half.
 *
 * The assertions are textual because the artefacts are text: a query string and a `.tsx`
 * source. They name the two field names the *prompt* names, and nothing about how the rename
 * was expressed — an interface, a type alias, a destructure and a property access all pass.
 *
 * Comments are stripped before matching. A correct fix may well leave a `// renamed from
 * totalAmount` behind, and grading a note about the old name as the old name would fail it.
 *
 * `web/src/generated/graphql-generated.ts` is deliberately not read. It is codegen output that
 * no build in this fixture regenerates, so it still carries the old name after a correct fix,
 * and grading it would fail every correct fix.
 *
 * Run from `web/`, which is where `npm test` runs it, so these paths are module-relative.
 */
const read = (path: string) =>
  readFileSync(path, "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/[^\n]*/g, " ");

/** The two sites the rename has to reach on this side of the seam. */
const sites = ["src/lib/orders.ts", "src/components/OrderSummary.tsx"];

describe("the rename reached the frontend", () => {
  it("leaves the old field name in neither the query nor the view", () => {
    for (const site of sites) {
      expect(read(site), `${site} still names totalAmount`).not.toMatch(/\btotalAmount\b/);
    }
  });

  it("uses the new field name in both", () => {
    for (const site of sites) {
      expect(read(site), `${site} never names grossAmount`).toMatch(/\bgrossAmount\b/);
    }
  });
});
