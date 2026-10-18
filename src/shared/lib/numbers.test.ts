import { describe, expect, it } from "vitest";
import { clamp, formatInteger } from "./numbers";

describe("formatInteger", () => {
  it("groups thousands with commas", () => {
    expect(formatInteger(1250)).toBe("1,250");
    expect(formatInteger(4_222)).toBe("4,222");
    expect(formatInteger(253)).toBe("253");
  });
});

describe("clamp", () => {
  it("constrains values within inclusive bounds", () => {
    expect(clamp(5, 0, 10)).toBe(5);
    expect(clamp(-3, 0, 10)).toBe(0);
    expect(clamp(42, 0, 10)).toBe(10);
  });
});
