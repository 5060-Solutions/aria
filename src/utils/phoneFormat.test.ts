import { describe, expect, it } from "vitest";
import { deviceCountry, dialDigits, dialHint, formatAsYouType, formatDisplayNumber } from "./phoneFormat";

describe("formatAsYouType", () => {
  it("formats a US national number as it is typed", () => {
    expect(formatAsYouType("8004444444", "US")).toBe("(800) 444-4444");
    expect(formatAsYouType("2128402440", "US")).toBe("(212) 840-2440");
    expect(formatAsYouType("8004", "US")).toBe("8004");
  });

  it("formats every step of typing without ever dropping a digit", () => {
    let typed = "";
    for (const d of "8004444444") {
      typed += d;
      expect(formatAsYouType(typed, "US").replace(/\D/g, "")).toBe(typed);
    }
  });

  it("formats an international number as the country its code names", () => {
    expect(formatAsYouType("+442079460958", "US")).toBe("+44 20 7946 0958");
    expect(formatAsYouType("+12128402440", "GB")).toBe("+1 212 840 2440");
    expect(formatAsYouType("+", "US")).toBe("+");
  });

  it("formats a national number for a non-US default country", () => {
    expect(formatAsYouType("02079460958", "GB")).toBe("020 7946 0958");
  });

  it("never touches extensions and short codes", () => {
    for (const short of ["1", "10", "1001", "9998", "911", "933", "711", "123456"]) {
      expect(formatAsYouType(short, "US")).toBe(short);
    }
  });

  it("never touches star codes, pound codes or names", () => {
    for (const code of ["*97", "*80101", "*98 1001", "#", "*67 8004444444", "#21#", "Pat Lee"]) {
      expect(formatAsYouType(code, "US")).toBe(code);
    }
  });

  it("reads a pasted number with spaces, dashes and dots", () => {
    expect(formatAsYouType("800 444 4444", "US")).toBe("(800) 444-4444");
    expect(formatAsYouType("800-444-4444", "US")).toBe("(800) 444-4444");
    expect(formatAsYouType("(800) 444-4444", "US")).toBe("(800) 444-4444");
    expect(formatAsYouType("800.444.4444", "US")).toBe("(800) 444-4444");
  });

  it("is empty for an empty box", () => {
    expect(formatAsYouType("", "US")).toBe("");
  });
});

describe("dialDigits", () => {
  it("dials the digits as typed, minus the formatting", () => {
    expect(dialDigits("(800) 444-4444")).toBe("8004444444");
    expect(dialDigits("800-444-4444")).toBe("8004444444");
    expect(dialDigits("800 444 4444")).toBe("8004444444");
    expect(dialDigits("+44 20 7946 0958")).toBe("+442079460958");
  });

  it("adds no country code and leaves extensions and star codes alone", () => {
    expect(dialDigits("1001")).toBe("1001");
    expect(dialDigits("*97")).toBe("*97");
    expect(dialDigits("*80101")).toBe("*80101");
    expect(dialDigits("911")).toBe("911");
    expect(dialDigits("#21#")).toBe("#21#");
  });

  it("leaves an address and a name as they are", () => {
    expect(dialDigits("sip:bob@pbx.example")).toBe("sip:bob@pbx.example");
    expect(dialDigits("Pat Lee")).toBe("Pat Lee");
  });
});

describe("dialHint", () => {
  it("shows the international reading of a valid national number", () => {
    expect(dialHint("8004444444", "US")).toBe("+1 (800) 444-4444");
    expect(dialHint("(212) 840-2440", "US")).toBe("+1 (212) 840-2440");
    expect(dialHint("02079460958", "GB")).toBe("+44 20 7946 0958");
  });

  it("shows nothing for extensions, star codes, partial or international input", () => {
    for (const none of ["", "1001", "*97", "911", "800444", "+12128402440", "800444444", "Pat"]) {
      expect(dialHint(none, "US")).toBeNull();
    }
  });
});

describe("formatDisplayNumber", () => {
  it("shows the default country's numbers nationally", () => {
    expect(formatDisplayNumber("+12128402440", "US")).toBe("(212) 840-2440");
    expect(formatDisplayNumber("+18004444444", "US")).toBe("(800) 444-4444");
    expect(formatDisplayNumber("8004444444", "US")).toBe("(800) 444-4444");
  });

  it("shows everyone else's internationally", () => {
    expect(formatDisplayNumber("+442079460958", "US")).toBe("+44 20 7946 0958");
    expect(formatDisplayNumber("+12128402440", "GB")).toBe("+1 212 840 2440");
    expect(formatDisplayNumber("+442079460958", "GB")).toBe("020 7946 0958");
  });

  it("leaves extensions, star codes, short codes, names and invalid numbers as they are", () => {
    for (const same of ["1001", "*97", "911", "Pat Lee", "5551234", "+1555", "anonymous"]) {
      expect(formatDisplayNumber(same, "US")).toBe(same);
    }
  });
});

describe("deviceCountry", () => {
  it("takes the region of the language tag, else US", () => {
    expect(deviceCountry("en-GB")).toBe("GB");
    expect(deviceCountry("de-DE")).toBe("DE");
    expect(deviceCountry("en")).toBe("US");
    expect(deviceCountry("en-ZZ")).toBe("US");
    expect(deviceCountry(undefined)).toBe("US");
  });
});
