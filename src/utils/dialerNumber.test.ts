import { describe, expect, it } from "vitest";
import { numberFontSize } from "./dialerNumber";

describe("numberFontSize", () => {
  it("is the largest size for nothing or a short number", () => {
    expect(numberFontSize(0, 348)).toBe(35);
    expect(numberFontSize(4, 348)).toBe(35);
  });

  it("shrinks a long number to fit the row rather than cutting it", () => {
    // 348 px row, 72 px for the clear button and its spacer: 276 px of text.
    expect(numberFontSize(14, 348)).toBe(31);
    expect(numberFontSize(20, 348)).toBe(22);
    expect(numberFontSize(40, 348)).toBe(16);
  });

  it("falls back to the largest size before the row has been measured", () => {
    expect(numberFontSize(14, 0)).toBe(35);
  });
});
