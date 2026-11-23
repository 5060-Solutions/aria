import { describe, expect, it } from "vitest";
import { blfDiff, blfExtension, blfExtensions, isNotRegistered } from "./blf";

describe("blfExtension", () => {
  it("takes short numeric users only", () => {
    expect(blfExtension("sip:1002@pbx.example")).toBe("1002");
    expect(blfExtension("sips:42@pbx.example")).toBe("42");
    expect(blfExtension("1002")).toBe("1002");
    expect(blfExtension("sip:+15551234567@carrier")).toBeNull();
    expect(blfExtension("sip:alice@pbx.example")).toBeNull();
    expect(blfExtension("5551234567")).toBeNull();
  });
});

describe("blfExtensions", () => {
  it("dedupes", () => {
    expect(blfExtensions([{ uri: "1002" }, { uri: "sip:1002@x" }, { uri: "alice" }])).toEqual(["1002"]);
  });
});

describe("blfDiff", () => {
  it("subscribes a newly added extension", () => {
    expect(blfDiff([{ uri: "1002" }], [{ uri: "1002" }, { uri: "1003" }])).toEqual({ add: ["1003"], remove: [] });
  });

  it("unsubscribes a deleted one", () => {
    expect(blfDiff([{ uri: "1002" }, { uri: "1003" }], [{ uri: "1002" }])).toEqual({ add: [], remove: ["1003"] });
  });

  it("follows an edited number", () => {
    expect(blfDiff([{ uri: "1002" }], [{ uri: "1004" }])).toEqual({ add: ["1004"], remove: ["1002"] });
  });

  it("keeps an extension another contact still has", () => {
    expect(blfDiff([{ uri: "1002" }, { uri: "sip:1002@x" }], [{ uri: "1002" }])).toEqual({ add: [], remove: [] });
  });

  it("does nothing for a rename", () => {
    expect(blfDiff([{ uri: "1002" }], [{ uri: "1002" }])).toEqual({ add: [], remove: [] });
  });
});

describe("isNotRegistered", () => {
  it("recognises the backend's error", () => {
    expect(isNotRegistered("Not registered")).toBe(true);
    expect(isNotRegistered("No account configured")).toBe(false);
  });
});
