import { beforeEach, describe, expect, it } from "vitest";
import { errorText, notifyError, useToastStore } from "./toastStore";

const state = () => useToastStore.getState();

describe("toastStore", () => {
  beforeEach(() => useToastStore.setState({ current: null, queue: [] }));

  it("shows the first toast at once and queues the rest", () => {
    notifyError("Transfer failed");
    state().show("Saved", "success");
    expect(state().current?.message).toBe("Transfer failed");
    expect(state().current?.severity).toBe("error");
    expect(state().queue.map((t) => t.message)).toEqual(["Saved"]);

    state().dismiss();
    expect(state().current?.message).toBe("Saved");
    state().dismiss();
    expect(state().current).toBeNull();
  });

  it("collapses the same message repeated", () => {
    notifyError("Not registered");
    notifyError("Not registered");
    expect(state().queue).toHaveLength(0);
    notifyError("Other");
    notifyError("Other");
    expect(state().queue).toHaveLength(1);
  });

  it("gives each toast its own id so a repeat after dismissal re-opens", () => {
    notifyError("Hold failed");
    const first = state().current?.id;
    state().dismiss();
    notifyError("Hold failed");
    expect(state().current?.id).not.toBe(first);
  });
});

describe("errorText", () => {
  it("reads backend strings and Errors", () => {
    expect(errorText("Not registered")).toBe("Not registered");
    expect(errorText(new Error("boom"))).toBe("boom");
    expect(errorText(42)).toBe("42");
  });
});
