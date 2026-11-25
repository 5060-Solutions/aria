import { describe, expect, it } from "vitest";
import { daySeparator, formatDelta, formatLogTime, formatLogTimestamp } from "./diagnosticsTime";

// 2026-11-24T15:30:12.345Z
const T = Date.UTC(2026, 10, 24, 15, 30, 12, 345);

describe("formatLogTime", () => {
  it("shows the time with milliseconds, 24-hour, in the given zone", () => {
    expect(formatLogTime(T, "en-US", "UTC")).toBe("15:30:12.345");
    expect(formatLogTime(T + 5, "de-DE", "UTC")).toBe("15:30:12.350");
    expect(formatLogTime(Date.UTC(2026, 10, 24, 0, 0, 0, 7), "en-US", "UTC")).toBe("00:00:00.007");
  });
});

describe("formatLogTimestamp", () => {
  it("is the full date and time with milliseconds, for a tooltip or a copy", () => {
    expect(formatLogTimestamp(T, "UTC")).toBe("2026-11-24 15:30:12.345 UTC");
  });
});

describe("formatDelta", () => {
  it("says how long after the previous message this one came", () => {
    expect(formatDelta(0)).toBe("+0 ms");
    expect(formatDelta(123)).toBe("+123 ms");
    expect(formatDelta(999)).toBe("+999 ms");
    expect(formatDelta(1000)).toBe("+1.00 s");
    expect(formatDelta(1234)).toBe("+1.23 s");
    expect(formatDelta(59_999)).toBe("+60.0 s");
    expect(formatDelta(61_000)).toBe("+1:01 min");
    expect(formatDelta(3_600_000)).toBe("+60:00 min");
  });

  it("is empty for the first message", () => {
    expect(formatDelta(null)).toBe("");
  });
});

describe("daySeparator", () => {
  it("marks the first message and every change of day", () => {
    const monday = Date.UTC(2026, 10, 23, 23, 59, 0);
    const tuesday = Date.UTC(2026, 10, 24, 0, 1, 0);
    expect(daySeparator(null, tuesday, "en-US", "UTC")).toBe("Nov 24, 2026");
    expect(daySeparator(monday, tuesday, "en-US", "UTC")).toBe("Nov 24, 2026");
    expect(daySeparator(tuesday, tuesday + 60_000, "en-US", "UTC")).toBeNull();
  });
});
