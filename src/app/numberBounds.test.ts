import { expect, it } from "vitest";
import { decimalCompare, numberInBounds, boundsProblem } from "./numberBounds";
it("compares precise signed decimals without floating conversion and includes both bounds", () => {
  expect(
    decimalCompare(
      "999999999999999999999.00000000000001",
      "999999999999999999999.00000000000002",
    ),
  ).toBe(-1);
  expect(decimalCompare("-0.00000000000001", "0")).toBe(-1);
  expect(numberInBounds("-1", "-1", "1")).toBe(true);
  expect(numberInBounds("1", "-1", "1")).toBe(true);
  expect(numberInBounds("1.00000000000001", "-1", "1")).toBe(false);
  expect(boundsProblem("1", "-1")).toBe("maximum");
  for (const value of ["-", "1.", "1.0", "-0", "01", "1e2"])
    expect(numberInBounds(value)).toBe(false);
});
